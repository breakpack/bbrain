use std::collections::BTreeMap;

use crate::error::{AppError, Result};

pub const MANAGED_START: &str = "<!-- bbrain:managed:start -->";
pub const MANAGED_END: &str = "<!-- bbrain:managed:end -->";
pub const USER_START: &str = "<!-- bbrain:user:start -->";
pub const USER_END: &str = "<!-- bbrain:user:end -->";

/// A parsed Bbrain note. Anything Bbrain does not own is preserved verbatim so a
/// round-trip through the app never loses the user's work (DEVELOPMENT.md §13.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Frontmatter in file order, including keys Bbrain knows nothing about.
    pub frontmatter: BTreeMap<String, String>,
    /// Raw frontmatter lines, so unknown structures (lists, nested maps) survive.
    pub frontmatter_lines: Vec<String>,
    pub managed: String,
    pub user: String,
    /// Content outside both managed and user blocks, kept in place.
    pub prologue: String,
    pub epilogue: String,
}

/// The managed markers are what make a safe merge possible. If one is missing or
/// they are out of order, Bbrain stops rather than overwriting (§13.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    UnbalancedManagedMarkers,
    UnbalancedUserMarkers,
}

impl ParseError {
    pub fn message(self) -> &'static str {
        match self {
            Self::UnbalancedManagedMarkers => {
                "노트의 Bbrain 관리 영역 표시가 손상되어 자동 동기화를 중지했습니다."
            }
            Self::UnbalancedUserMarkers => {
                "노트의 사용자 영역 표시가 손상되어 자동 동기화를 중지했습니다."
            }
        }
    }
}

pub fn parse(content: &str) -> Result<Note> {
    let (frontmatter_lines, body) = split_frontmatter(content);
    let frontmatter = parse_frontmatter(&frontmatter_lines);

    let managed = extract_block(body, MANAGED_START, MANAGED_END)
        .map_err(|_| AppError::NoteConflict(ParseError::UnbalancedManagedMarkers))?;
    let user = extract_block(body, USER_START, USER_END)
        .map_err(|_| AppError::NoteConflict(ParseError::UnbalancedUserMarkers))?;

    // Everything before the first block and after the last is the user's own
    // structure — headings, links, whatever — and is never touched.
    let first = [managed, user]
        .iter()
        .filter_map(|block| block.map(|(start, _, _)| start))
        .min()
        .unwrap_or(body.len());
    let last = [managed, user]
        .iter()
        .filter_map(|block| block.map(|(_, _, end)| end))
        .max()
        .unwrap_or(body.len());

    Ok(Note {
        frontmatter,
        frontmatter_lines,
        managed: managed
            .map(|(_, text, _)| text.to_string())
            .unwrap_or_default(),
        user: user.map(|(_, text, _)| text.to_string()).unwrap_or_default(),
        prologue: body[..first].to_string(),
        epilogue: body[last.min(body.len())..].to_string(),
    })
}

/// Returns (block start offset, inner text, block end offset).
type Block<'a> = Option<(usize, &'a str, usize)>;

fn extract_block<'a>(
    body: &'a str,
    start_marker: &str,
    end_marker: &str,
) -> std::result::Result<Block<'a>, ()> {
    let start = body.find(start_marker);
    let end = body.find(end_marker);

    match (start, end) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) if end > start => {
            let inner_start = start + start_marker.len();
            Ok(Some((start, body[inner_start..end].trim_matches('\n'), end + end_marker.len())))
        }
        // One marker without the other, or reversed: refuse to guess.
        _ => Err(()),
    }
}

fn split_frontmatter(content: &str) -> (Vec<String>, &str) {
    let trimmed = content.strip_prefix('\u{feff}').unwrap_or(content);
    let Some(rest) = trimmed.strip_prefix("---\n") else {
        return (Vec::new(), trimmed);
    };
    let Some(end) = rest.find("\n---\n") else {
        return (Vec::new(), trimmed);
    };

    let lines = rest[..end].lines().map(str::to_string).collect();
    (lines, &rest[end + 5..])
}

/// A deliberately small YAML reader: scalar keys are understood, and everything
/// else (lists, nested maps) is preserved as raw lines rather than reformatted.
fn parse_frontmatter(lines: &[String]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();

    for line in lines {
        if line.starts_with(' ') || line.starts_with('-') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            map.insert(
                key.trim().to_string(),
                value.trim_matches('"').to_string(),
            );
        }
    }

    map
}

/// Reads a YAML list under `key`, which is how groups, tags and authors are
/// stored.
pub fn list_field(lines: &[String], key: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut inside = false;

    for line in lines {
        if !inside {
            if line.trim_end() == format!("{key}:") {
                inside = true;
            }
            continue;
        }

        let trimmed = line.trim_start();
        if let Some(item) = trimmed.strip_prefix("- ") {
            values.push(item.trim().trim_matches('"').to_string());
        } else if !line.starts_with(' ') {
            break;
        }
    }

    values
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = r#"---
bbrain_id: "019-abc"
title: "Attention Is All You Need"
year: 2017
custom_field: "the user added this"
tags:
  - rag
  - retrieval
---

<!-- bbrain:managed:start -->
# Attention Is All You Need

## 요약

트랜스포머.
<!-- bbrain:managed:end -->

<!-- bbrain:user:start -->
## My Notes

이 논문은 중요하다. [[Related note]]
<!-- bbrain:user:end -->

## 사용자가 만든 섹션

Bbrain이 모르는 내용.
"#;

    #[test]
    fn parses_the_managed_and_user_blocks() {
        let note = parse(NOTE).unwrap();

        assert!(note.managed.contains("## 요약"));
        assert!(note.user.contains("이 논문은 중요하다"));
    }

    #[test]
    fn unknown_frontmatter_keys_are_preserved() {
        let note = parse(NOTE).unwrap();

        assert_eq!(
            note.frontmatter.get("custom_field").map(String::as_str),
            Some("the user added this")
        );
        assert_eq!(note.frontmatter.get("bbrain_id").map(String::as_str), Some("019-abc"));
    }

    #[test]
    fn list_fields_are_read_as_lists() {
        let note = parse(NOTE).unwrap();

        assert_eq!(list_field(&note.frontmatter_lines, "tags"), vec!["rag", "retrieval"]);
    }

    #[test]
    fn content_outside_the_blocks_is_kept() {
        let note = parse(NOTE).unwrap();

        assert!(
            note.epilogue.contains("Bbrain이 모르는 내용"),
            "a section Bbrain does not own must survive a round trip"
        );
    }

    #[test]
    fn a_note_with_only_an_opening_marker_is_a_conflict_rather_than_an_overwrite() {
        let broken = "---\nbbrain_id: \"x\"\n---\n\n<!-- bbrain:managed:start -->\n내용\n";

        let error = parse(broken).unwrap_err();

        assert!(matches!(
            error,
            AppError::NoteConflict(ParseError::UnbalancedManagedMarkers)
        ));
    }

    #[test]
    fn reversed_markers_are_a_conflict() {
        let broken = "<!-- bbrain:managed:end -->\ntext\n<!-- bbrain:managed:start -->";

        assert!(parse(broken).is_err());
    }

    #[test]
    fn a_note_with_no_frontmatter_still_parses() {
        let note = parse("just text").unwrap();

        assert!(note.frontmatter.is_empty());
        assert_eq!(note.prologue, "just text");
    }
}
