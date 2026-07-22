pub mod pdf_file;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;

use crate::db::paper_repo::{self, ImportStatus};
use crate::error::{AppError, Result};
use crate::ids::new_id;
use crate::paths::AppPaths;

/// Outcome per file, so the UI can report partial success honestly instead of
/// failing the whole drop.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "outcome")]
pub enum ImportOutcome {
    Imported { paper_id: String, title: String },
    /// Same SHA-256 as a paper already in the library — the existing one is
    /// opened rather than copied again (§8.2).
    Duplicate { paper_id: String, title: String },
    Rejected { file_name: String, reason: pdf_file::RejectReason, message: String },
}

/// Import one file. Order matters: validate, hash, then dedupe, then copy the
/// bytes durably, and only commit the DB row once the file is safely in place.
pub fn import_one(
    conn: &mut Connection,
    paths: &AppPaths,
    source: &Path,
    target_group_id: Option<&str>,
) -> ImportOutcome {
    let file_name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown.pdf".into());

    match try_import(conn, paths, source, target_group_id) {
        Ok(outcome) => outcome,
        Err(AppError::Rejected(reason)) => ImportOutcome::Rejected {
            file_name,
            reason,
            message: reason.message().to_string(),
        },
        Err(error) => {
            tracing::warn!(code = ?error.code(), "import failed");
            ImportOutcome::Rejected {
                file_name,
                reason: pdf_file::RejectReason::Unreadable,
                message: error.redacted_message(),
            }
        }
    }
}

fn try_import(
    conn: &mut Connection,
    paths: &AppPaths,
    source: &Path,
    target_group_id: Option<&str>,
) -> Result<ImportOutcome> {
    pdf_file::validate(source)?;
    let hash = pdf_file::sha256(source)?;

    if let Some(existing) = paper_repo::find_by_hash(conn, &hash)? {
        // A duplicate may still be filed into the active group.
        if let Some(group_id) = target_group_id {
            paper_repo::add_to_group(conn, &existing, group_id)?;
        }
        let title = paper_repo::get(conn, &existing)?.title;
        return Ok(ImportOutcome::Duplicate {
            paper_id: existing,
            title,
        });
    }

    let paper_id = new_id();
    let managed = managed_pdf_path(paths, &paper_id);
    pdf_file::copy_atomically(source, &managed)?;

    let title = title_from_file_name(source);
    let managed_str = managed.to_string_lossy().to_string();

    let commit = (|| -> Result<()> {
        let tx = conn.transaction()?;
        paper_repo::insert(
            &tx,
            &paper_id,
            &hash,
            &managed_str,
            &title,
            ImportStatus::Extracting,
        )?;
        if let Some(group_id) = target_group_id {
            paper_repo::add_to_group(&tx, &paper_id, group_id)?;
        }
        tx.commit()?;
        Ok(())
    })();

    if let Err(error) = commit {
        // The row never landed, so the managed copy is an orphan — remove it
        // rather than leaving unreferenced bytes in app storage.
        let _ = std::fs::remove_dir_all(paths.paper_dir(&paper_id));
        return Err(error);
    }

    Ok(ImportOutcome::Imported { paper_id, title })
}

pub fn managed_pdf_path(paths: &AppPaths, paper_id: &str) -> PathBuf {
    paths.paper_dir(paper_id).join("source.pdf")
}

/// Best-effort title until extraction finds a real one. Strips the extension and
/// tidies the separators most download sites use.
pub fn title_from_file_name(source: &Path) -> String {
    let file_name = source
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // Work from the file name, not `file_stem`: for a dotfile like `.pdf` the
    // stem is the whole name, which would yield a title of "pdf".
    let stem = file_name
        .strip_suffix(".pdf")
        .or_else(|| file_name.strip_suffix(".PDF"))
        .unwrap_or(&file_name);

    let cleaned = stem
        .replace(['_', '+'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if cleaned.is_empty() {
        "제목 없음".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::io::Write;

    fn minimal_pdf() -> Vec<u8> {
        let mut bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        bytes.extend_from_slice(b"trailer\n<< /Root 1 0 R >>\nstartxref\n0\n%%EOF\n");
        bytes
    }

    fn write_pdf(dir: &Path, name: &str, extra: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        let mut bytes = minimal_pdf();
        bytes.extend_from_slice(extra);
        bytes.extend_from_slice(b"\n%%EOF\n");
        file.write_all(&bytes).unwrap();
        path
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: AppPaths,
        db: Database,
        source_dir: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path().join("Bbrain")).unwrap();
        let source_dir = dir.path().join("downloads");
        std::fs::create_dir_all(&source_dir).unwrap();

        Fixture {
            _dir: dir,
            paths,
            db: Database::open_in_memory().unwrap(),
            source_dir,
        }
    }

    #[test]
    fn imports_a_pdf_and_copies_it_into_managed_storage() {
        let f = fixture();
        let source = write_pdf(&f.source_dir, "attention_is_all_you_need.pdf", b"");

        let outcome = import_one(&mut f.db.conn(), &f.paths, &source, None);

        let ImportOutcome::Imported { paper_id, title } = outcome else {
            panic!("expected an import, got {outcome:?}");
        };
        assert_eq!(title, "attention is all you need");
        assert!(managed_pdf_path(&f.paths, &paper_id).is_file());

        let conn = f.db.conn();
        let paper = paper_repo::get(&conn, &paper_id).unwrap();
        assert_eq!(paper.import_status, ImportStatus::Extracting);
    }

    #[test]
    fn the_same_content_under_a_different_name_is_a_duplicate() {
        let f = fixture();
        let first = write_pdf(&f.source_dir, "paper.pdf", b"");
        let second = write_pdf(&f.source_dir, "paper-copy.pdf", b"");

        let ImportOutcome::Imported { paper_id, .. } =
            import_one(&mut f.db.conn(), &f.paths, &first, None)
        else {
            panic!("first import should succeed");
        };
        let outcome = import_one(&mut f.db.conn(), &f.paths, &second, None);

        let ImportOutcome::Duplicate { paper_id: existing, .. } = outcome else {
            panic!("expected a duplicate, got {outcome:?}");
        };
        assert_eq!(existing, paper_id);

        let conn = f.db.conn();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM papers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "a duplicate must not create a second row");
    }

    #[test]
    fn a_duplicate_still_joins_the_active_group() {
        let f = fixture();
        let source = write_pdf(&f.source_dir, "paper.pdf", b"");
        let group = paper_repo::create_group(&f.db.conn(), "Reading List", None).unwrap();

        import_one(&mut f.db.conn(), &f.paths, &source, None);
        let outcome = import_one(&mut f.db.conn(), &f.paths, &source, Some(&group));

        let ImportOutcome::Duplicate { paper_id, .. } = outcome else {
            panic!("expected a duplicate");
        };
        let conn = f.db.conn();
        assert_eq!(paper_repo::get(&conn, &paper_id).unwrap().group_ids, vec![group]);
    }

    #[test]
    fn different_content_with_the_same_name_imports_separately() {
        let f = fixture();
        let first = write_pdf(&f.source_dir, "v1.pdf", b"version one");
        let second = write_pdf(&f.source_dir, "v2.pdf", b"version two");

        import_one(&mut f.db.conn(), &f.paths, &first, None);
        let outcome = import_one(&mut f.db.conn(), &f.paths, &second, None);

        assert!(matches!(outcome, ImportOutcome::Imported { .. }));
        let conn = f.db.conn();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM papers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn a_rejected_file_leaves_no_row_and_no_managed_bytes() {
        let f = fixture();
        let path = f.source_dir.join("notes.pdf");
        std::fs::write(&path, b"this is not a pdf").unwrap();

        let outcome = import_one(&mut f.db.conn(), &f.paths, &path, None);

        assert!(matches!(
            outcome,
            ImportOutcome::Rejected {
                reason: pdf_file::RejectReason::NotPdf,
                ..
            }
        ));
        let conn = f.db.conn();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM papers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(std::fs::read_dir(f.paths.papers_dir()).unwrap().count(), 0);
    }

    #[test]
    fn a_rejected_file_explains_itself_in_korean() {
        let f = fixture();
        let path = f.source_dir.join("locked.pdf");
        std::fs::write(
            &path,
            b"%PDF-1.7\ntrailer\n<< /Encrypt 9 0 R >>\nstartxref\n0\n%%EOF\n",
        )
        .unwrap();

        let outcome = import_one(&mut f.db.conn(), &f.paths, &path, None);

        let ImportOutcome::Rejected { message, reason, .. } = outcome else {
            panic!("expected a rejection");
        };
        assert_eq!(reason, pdf_file::RejectReason::Encrypted);
        assert!(message.contains("암호"));
    }

    #[test]
    fn file_name_titles_are_tidied() {
        assert_eq!(
            title_from_file_name(Path::new("/tmp/dense_passage+retrieval.pdf")),
            "dense passage retrieval"
        );
        assert_eq!(title_from_file_name(Path::new("/tmp/.pdf")), "제목 없음");
    }
}
