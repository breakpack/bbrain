pub mod note;
pub mod watch;

use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use crate::db::{paper_repo, settings_repo};
use crate::db::paper_repo::Paper;
use crate::error::{AppError, Result};
use crate::relations;
use crate::state::AppState;
use crate::time::now_iso8601;
use note::{MANAGED_END, MANAGED_START, USER_END, USER_START};

pub const SYNC_STATUS: &str = "sync://status";

/// Writes one paper's note into the vault, merging with whatever the user has
/// changed there (DEVELOPMENT.md §13).
pub async fn sync_paper(app: &AppHandle, paper_id: &str) -> Result<()> {
    let state = app.state::<AppState>();

    let (vault, paper, markdown, related) = {
        let conn = state.db.conn();

        let vault = settings_repo::get(&conn)?
            .obsidian_vault_path
            .ok_or_else(|| AppError::NotFound("no obsidian vault configured".into()))?;

        let paper = paper_repo::get(&conn, paper_id)?;
        let markdown: Option<String> = conn
            .query_row(
                "SELECT markdown FROM analyses WHERE paper_id = ?1",
                params![paper_id],
                |row| row.get(0),
            )
            .optional()?;
        let related = relations::related_titles(&conn, paper_id)?;

        (PathBuf::from(vault), paper, markdown, related)
    };

    let Some(markdown) = markdown else {
        tracing::info!(paper = %paper_id, "no analysis yet; nothing to sync");
        return Ok(());
    };

    let vault_root = vault.join("Bbrain");
    let notes_dir = vault_root.join("Papers");
    let attachments_dir = vault_root.join("Attachments");
    std::fs::create_dir_all(&notes_dir)
        .map_err(|e| AppError::VaultUnavailable(e.to_string()))?;
    std::fs::create_dir_all(&attachments_dir)
        .map_err(|e| AppError::VaultUnavailable(e.to_string()))?;

    let note_path = note_path(&notes_dir, &paper);
    let existing = std::fs::read_to_string(&note_path).ok();

    let merged = match &existing {
        Some(content) => merge(content, &paper, &markdown, &related)?,
        None => render_new(&paper, &markdown, &related),
    };

    write_atomically(&note_path, &merged)?;
    copy_attachment(&state.paths.paper_dir(paper_id), &attachments_dir, paper_id)?;

    {
        let conn = state.db.conn();
        conn.execute(
            "INSERT INTO sync_records
               (paper_id, vault_path, base_hash, app_revision, vault_revision,
                attachment_strategy, status, updated_at)
             VALUES (?1, ?2, ?3, ?3, ?3, 'copy', 'synced', ?4)
             ON CONFLICT(paper_id) DO UPDATE SET
               vault_path = excluded.vault_path,
               base_hash = excluded.base_hash,
               app_revision = excluded.app_revision,
               vault_revision = excluded.vault_revision,
               status = 'synced',
               updated_at = excluded.updated_at",
            params![
                paper_id,
                note_path.to_string_lossy(),
                content_hash(&merged),
                now_iso8601()
            ],
        )?;
    }

    let _ = app.emit(SYNC_STATUS, paper_id);
    Ok(())
}

/// Merge policy (§13.3): Bbrain owns the managed block and its own frontmatter
/// keys. The user owns their block, their groups and tags, and anything Bbrain
/// does not recognize — all of which are carried through untouched.
pub fn merge(existing: &str, paper: &Paper, markdown: &str, related: &[String]) -> Result<String> {
    let parsed = note::parse(existing)?;

    let mut frontmatter = String::from("---\n");
    frontmatter.push_str(&format!("bbrain_id: \"{}\"\n", paper.id));
    frontmatter.push_str(&format!("title: \"{}\"\n", escape(&paper.title)));

    if !paper.authors.is_empty() {
        frontmatter.push_str("authors:\n");
        for author in &paper.authors {
            frontmatter.push_str(&format!("  - \"{}\"\n", escape(author)));
        }
    }
    if let Some(year) = paper.year {
        frontmatter.push_str(&format!("year: {year}\n"));
    }
    if let Some(doi) = &paper.doi {
        frontmatter.push_str(&format!("doi: \"{}\"\n", escape(doi)));
    }

    // Groups and tags are bidirectional: whatever the vault currently says wins
    // here, because the file watcher pulls vault edits back into the app.
    let groups = note::list_field(&parsed.frontmatter_lines, "groups");
    if !groups.is_empty() {
        frontmatter.push_str("groups:\n");
        for group in &groups {
            frontmatter.push_str(&format!("  - \"{}\"\n", escape(group)));
        }
    }

    let vault_tags = note::list_field(&parsed.frontmatter_lines, "tags");
    let tags: Vec<String> = if vault_tags.is_empty() {
        paper.tags.iter().map(|tag| tag.display_name.clone()).collect()
    } else {
        vault_tags
    };
    if !tags.is_empty() {
        frontmatter.push_str("tags:\n");
        for tag in &tags {
            frontmatter.push_str(&format!("  - {}\n", tag));
        }
    }

    frontmatter.push_str(&format!("bbrain_pdf: \"../Attachments/{}.pdf\"\n", paper.id));
    frontmatter.push_str(&format!("bbrain_updated_at: \"{}\"\n", now_iso8601()));

    // Keys Bbrain has never heard of are preserved exactly (§13.3).
    for line in &parsed.frontmatter_lines {
        if let Some((key, _)) = line.split_once(':') {
            let key = key.trim();
            if is_managed_key(key) || line.starts_with(' ') || line.starts_with('-') {
                continue;
            }
            frontmatter.push_str(line);
            frontmatter.push('\n');
        }
    }
    frontmatter.push_str("---\n");

    let managed = managed_block(markdown, related);
    let user = if parsed.user.is_empty() {
        default_user_block()
    } else {
        parsed.user
    };

    Ok(format!(
        "{frontmatter}{prologue}{MANAGED_START}\n{managed}\n{MANAGED_END}\n\n{USER_START}\n{user}\n{USER_END}\n{epilogue}",
        prologue = normalize_gap(&parsed.prologue),
        // Normalized, not passed through: otherwise every merge would add another
        // blank line and the note would drift on each sync.
        epilogue = normalize_gap(&parsed.epilogue),
    ))
}

fn render_new(paper: &Paper, markdown: &str, related: &[String]) -> String {
    let empty = format!("---\nbbrain_id: \"{}\"\n---\n", paper.id);
    // A fresh note is the same merge against an empty document, so there is only
    // one code path that decides note layout.
    merge(&empty, paper, markdown, related).unwrap_or_else(|_| {
        format!(
            "{MANAGED_START}\n{}\n{MANAGED_END}\n\n{USER_START}\n{}\n{USER_END}\n",
            managed_block(markdown, related),
            default_user_block()
        )
    })
}

fn managed_block(markdown: &str, related: &[String]) -> String {
    if related.is_empty() {
        return markdown.trim().to_string();
    }

    // Related papers are plain wiki links, so Obsidian's own graph picks them up
    // without a Bbrain plugin (§13.4).
    let links = related
        .iter()
        .map(|title| format!("- [[{}]]", sanitize_link(title)))
        .collect::<Vec<_>>()
        .join("\n");

    let body = markdown.trim();
    match body.split_once("## 관련 논문") {
        Some((head, _)) => format!("{}## 관련 논문\n\n{links}", head),
        None => format!("{body}\n\n## 관련 논문\n\n{links}"),
    }
}

fn default_user_block() -> String {
    "## My Notes\n".to_string()
}

fn is_managed_key(key: &str) -> bool {
    matches!(
        key,
        "bbrain_id"
            | "title"
            | "authors"
            | "year"
            | "doi"
            | "groups"
            | "tags"
            | "bbrain_pdf"
            | "bbrain_updated_at"
    )
}

fn normalize_gap(prologue: &str) -> String {
    let trimmed = prologue.trim();
    if trimmed.is_empty() {
        "\n".to_string()
    } else {
        format!("\n{trimmed}\n\n")
    }
}

fn escape(value: &str) -> String {
    value.replace('"', "'")
}

/// Wiki links cannot contain the characters Obsidian uses for aliases and
/// embeds.
/// Exports the topic graph ("second brain") to the vault as one note per topic,
/// linking related topics and member papers with `[[wiki links]]` so Obsidian's
/// Graph View renders the concept map. Fully Bbrain-managed and regenerated on
/// each export (§13). Returns how many topic notes were written.
pub fn export_topic_graph(app: &AppHandle) -> Result<usize> {
    use std::collections::HashMap;

    let state = app.state::<AppState>();
    let (vault, graph) = {
        let conn = state.db.conn();
        let vault = settings_repo::get(&conn)?.obsidian_vault_path.ok_or_else(|| {
            AppError::VaultUnavailable("Obsidian 보관함이 설정되지 않았습니다.".into())
        })?;
        (vault, crate::topics::load_topic_graph(&conn)?)
    };

    if graph.nodes.is_empty() {
        return Ok(0);
    }

    let topics_dir = Path::new(&vault).join("Bbrain").join("Topics");
    std::fs::create_dir_all(&topics_dir).map_err(|e| AppError::VaultUnavailable(e.to_string()))?;

    let label_by_id: HashMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.label.as_str()))
        .collect();

    // Undirected adjacency, storing each neighbour's file-safe name for linking.
    let mut related: HashMap<&str, Vec<String>> = HashMap::new();
    for edge in &graph.edges {
        let (Some(a), Some(b)) = (
            label_by_id.get(edge.source.as_str()),
            label_by_id.get(edge.target.as_str()),
        ) else {
            continue;
        };
        related.entry(edge.source.as_str()).or_default().push(safe_file_name(b));
        related.entry(edge.target.as_str()).or_default().push(safe_file_name(a));
    }

    let mut written = 0;
    for node in &graph.nodes {
        let mut body = format!(
            "# {}\n\n_{}편의 논문에서 추출된 개념_\n\n",
            node.label, node.paper_count
        );

        if let Some(rels) = related.get(node.id.as_str()) {
            let mut uniq = rels.clone();
            uniq.sort();
            uniq.dedup();
            if !uniq.is_empty() {
                body.push_str("## 관련 개념\n");
                for name in uniq {
                    body.push_str(&format!("- [[{name}]]\n"));
                }
                body.push('\n');
            }
        }

        if !node.papers.is_empty() {
            body.push_str("## 논문\n");
            for paper in &node.papers {
                let short: String = paper.id.chars().take(8).collect();
                body.push_str(&format!("- [[{}-{}]]\n", safe_file_name(&paper.title), short));
            }
            body.push('\n');
        }

        let note = format!("{MANAGED_START}\n{body}{MANAGED_END}\n");
        let path = topics_dir.join(format!("{}.md", safe_file_name(&node.label)));
        write_atomically(&path, &note)?;
        written += 1;
    }

    let _ = app.emit(SYNC_STATUS, ());
    Ok(written)
}

pub fn sanitize_link(title: &str) -> String {
    title
        .replace(['[', ']', '|', '#', '^'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// File names come from user-controlled titles, so they are sanitized and the
/// result is checked to stay inside the notes directory (§16.3).
pub fn note_path(notes_dir: &Path, paper: &Paper) -> PathBuf {
    let short_id: String = paper.id.chars().take(8).collect();
    let safe_title = safe_file_name(&paper.title);

    notes_dir.join(format!("{safe_title}-{short_id}.md"))
}

pub fn safe_file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();

    // `..` would climb out of the directory even with the separators gone, and a
    // leading dot hides the file.
    let mut collapsed = cleaned;
    while collapsed.contains("..") {
        collapsed = collapsed.replace("..", "-");
    }
    let trimmed = collapsed.trim().trim_matches(['.', '-', ' ']).trim();
    let truncated: String = trimmed.chars().take(80).collect();

    if truncated.is_empty() {
        "제목 없음".to_string()
    } else {
        truncated.trim().to_string()
    }
}

fn write_atomically(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::VaultUnavailable("note has no parent directory".into()))?;

    // A temp file in the same directory, then rename: Obsidian never sees a
    // half-written note (§13.4).
    let temp = parent.join(format!(
        ".{}.bbrain.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("note")
    ));

    std::fs::write(&temp, content).map_err(|e| AppError::VaultUnavailable(e.to_string()))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        AppError::VaultUnavailable(e.to_string())
    })?;

    Ok(())
}

/// Hard links keep one copy of the PDF when the vault is on the same volume;
/// otherwise the file is copied.
fn copy_attachment(paper_dir: &Path, attachments_dir: &Path, paper_id: &str) -> Result<()> {
    let source = paper_dir.join("source.pdf");
    if !source.exists() {
        return Ok(());
    }

    let destination = attachments_dir.join(format!("{paper_id}.pdf"));
    if destination.exists() {
        return Ok(());
    }

    if std::fs::hard_link(&source, &destination).is_ok() {
        return Ok(());
    }

    std::fs::copy(&source, &destination)
        .map(|_| ())
        .map_err(|e| AppError::VaultUnavailable(e.to_string()))
}

pub fn content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::paper_repo::{ImportStatus, Tag};

    fn paper() -> Paper {
        Paper {
            id: "019512b0-0000-7000-8000-000000000000".into(),
            sha256: "hash".into(),
            title: "Attention Is All You Need".into(),
            import_status: ImportStatus::Ready,
            page_count: Some(11),
            is_favorite: false,
            last_opened_at: None,
            created_at: "2026-07-14T00:00:00Z".into(),
            updated_at: "2026-07-14T00:00:00Z".into(),
            authors: vec!["Vaswani".into()],
            year: Some(2017),
            venue: None,
            doi: Some("10.1234/example".into()),
            abstract_text: None,
            group_ids: vec![],
            tags: vec![Tag {
                id: "t1".into(),
                display_name: "transformer".into(),
                source: "ai".into(),
            }],
        }
    }

    const EXISTING: &str = r#"---
bbrain_id: "019512b0-0000-7000-8000-000000000000"
title: "Attention Is All You Need"
zotero_key: "ABCD1234"
groups:
  - "Reading List"
tags:
  - transformer
  - my-own-tag
---

<!-- bbrain:managed:start -->
# 오래된 내용
<!-- bbrain:managed:end -->

<!-- bbrain:user:start -->
## My Notes

직접 쓴 메모. [[다른 노트]]
<!-- bbrain:user:end -->

## 내가 만든 섹션

Bbrain이 모르는 내용.
"#;

    #[test]
    fn the_managed_block_is_replaced_and_the_user_block_is_kept() {
        let merged = merge(EXISTING, &paper(), "# 새 분석\n\n## 요약\n\n내용", &[]).unwrap();

        assert!(merged.contains("# 새 분석"));
        assert!(!merged.contains("# 오래된 내용"));
        assert!(merged.contains("직접 쓴 메모. [[다른 노트]]"));
    }

    #[test]
    fn unknown_frontmatter_keys_and_sections_survive_a_merge() {
        let merged = merge(EXISTING, &paper(), "# 분석", &[]).unwrap();

        assert!(merged.contains("zotero_key: \"ABCD1234\""));
        assert!(merged.contains("## 내가 만든 섹션"));
        assert!(merged.contains("Bbrain이 모르는 내용"));
    }

    #[test]
    fn tags_edited_in_the_vault_win_over_the_apps_copy() {
        let merged = merge(EXISTING, &paper(), "# 분석", &[]).unwrap();

        // The vault has `my-own-tag`, which the app does not know about.
        assert!(merged.contains("- my-own-tag"));
        assert!(merged.contains("- transformer"));
    }

    #[test]
    fn groups_set_in_the_vault_are_preserved() {
        let merged = merge(EXISTING, &paper(), "# 분석", &[]).unwrap();

        assert!(merged.contains("- \"Reading List\""));
    }

    #[test]
    fn related_papers_are_written_as_plain_wiki_links() {
        let merged = merge(
            EXISTING,
            &paper(),
            "# 분석\n\n## 관련 논문\n\n아직 없음",
            &["Dense Passage Retrieval".into()],
        )
        .unwrap();

        assert!(merged.contains("- [[Dense Passage Retrieval]]"));
        assert!(!merged.contains("아직 없음"));
    }

    #[test]
    fn merging_is_idempotent_apart_from_the_timestamp() {
        let once = merge(EXISTING, &paper(), "# 분석", &[]).unwrap();
        let twice = merge(&once, &paper(), "# 분석", &[]).unwrap();

        let strip = |text: &str| {
            text.lines()
                .filter(|line| !line.starts_with("bbrain_updated_at"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        assert_eq!(strip(&once), strip(&twice));
    }

    #[test]
    fn a_note_with_broken_markers_refuses_to_merge() {
        let broken = "---\nbbrain_id: \"x\"\n---\n<!-- bbrain:managed:start -->\n내용\n";

        let error = merge(broken, &paper(), "# 분석", &[]).unwrap_err();

        assert!(matches!(error, AppError::NoteConflict(_)));
    }

    #[test]
    fn a_new_note_has_both_blocks_and_the_pdf_link() {
        let rendered = render_new(&paper(), "# 분석", &[]);

        assert!(rendered.contains(MANAGED_START));
        assert!(rendered.contains(USER_START));
        assert!(rendered.contains("bbrain_pdf: \"../Attachments/019512b0-0000-7000-8000-000000000000.pdf\""));
    }

    #[test]
    fn file_names_cannot_traverse_out_of_the_notes_directory() {
        let dir = Path::new("/vault/Bbrain/Papers");

        let mut hostile = paper();
        hostile.title = "../../../etc/passwd".into();
        let path = note_path(dir, &hostile);

        assert!(path.starts_with(dir), "the note must stay inside Papers/");
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn a_title_of_only_dots_still_produces_a_usable_name() {
        assert_eq!(safe_file_name("..."), "제목 없음");
        assert_eq!(safe_file_name("a/b:c"), "a-b-c");
    }

    #[test]
    fn wiki_links_drop_characters_obsidian_treats_as_syntax() {
        assert_eq!(sanitize_link("A [[B]] | C"), "A B C");
    }

    #[test]
    fn an_atomic_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");

        write_atomically(&path, "내용").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "내용");
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
    }
}
