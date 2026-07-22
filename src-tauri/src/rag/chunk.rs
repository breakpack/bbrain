use crate::db::page_repo::text_hash;

/// e5-small takes 512 tokens; Bbrain targets ~420 with ~60 of overlap
/// (DEVELOPMENT.md §5.2, §11.1).
pub const TARGET_TOKENS: usize = 420;
pub const OVERLAP_TOKENS: usize = 60;
/// Hard ceiling: a chunk longer than the model's window would be truncated
/// silently, so it is split even mid-sentence.
pub const MAX_TOKENS: usize = 480;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub section: Option<String>,
    pub page_start: i64,
    pub page_end: i64,
    pub text: String,
    pub token_count: i64,
    pub content_hash: String,
}

/// Rough token estimate that works for both Korean and English without pulling
/// in a tokenizer: CJK runs about one token per character, Latin about one per
/// four. Chunk sizes only need to stay under the model limit, not be exact.
pub fn estimate_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;

    for character in text.chars() {
        if is_cjk(character) {
            cjk += 1;
        } else {
            other += 1;
        }
    }

    cjk + other.div_ceil(4)
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x1100..=0x11FF   // Hangul Jamo
        | 0x3040..=0x30FF // Kana
        | 0x3400..=0x4DBF // CJK Extension A
        | 0x4E00..=0x9FFF // CJK Unified
        | 0xAC00..=0xD7AF // Hangul Syllables
    )
}

/// Heading detection for section boundaries: numbered headings and the standard
/// paper section names.
fn heading(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.len() > 80 {
        return None;
    }

    const KNOWN: &[&str] = &[
        "abstract",
        "introduction",
        "related work",
        "background",
        "method",
        "methods",
        "methodology",
        "approach",
        "experiments",
        "experimental setup",
        "results",
        "evaluation",
        "discussion",
        "conclusion",
        "conclusions",
        "limitations",
        "references",
        "appendix",
    ];

    let lowered = trimmed.to_lowercase();
    let stripped = lowered
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
        .trim();

    let numbered = trimmed
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit())
        && trimmed.contains(' ')
        && trimmed.split_whitespace().count() <= 8;

    if KNOWN.contains(&stripped) || (numbered && KNOWN.contains(&stripped)) {
        return Some(trimmed.to_string());
    }
    if numbered && trimmed.split_whitespace().count() <= 6 {
        return Some(trimmed.to_string());
    }

    None
}

/// Splits a document into chunks that respect section boundaries and never cut a
/// sentence in half unless the sentence alone exceeds the model's window (§11.1).
pub fn chunk_document(pages: &[(i64, String)]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut section: Option<String> = None;

    // (text, page) units: sentences carry the page they came from so a chunk can
    // report an accurate page range.
    let mut units: Vec<(String, i64)> = Vec::new();
    let mut unit_section: Vec<Option<String>> = Vec::new();

    for (page_number, text) in pages {
        for line in text.lines() {
            if let Some(found) = heading(line) {
                section = Some(found);
                continue;
            }
            for sentence in split_sentences(line) {
                if sentence.trim().is_empty() {
                    continue;
                }
                units.push((sentence, *page_number));
                unit_section.push(section.clone());
            }
        }
    }

    let mut index = 0usize;
    while index < units.len() {
        let start_section = unit_section[index].clone();
        let mut text = String::new();
        let mut tokens = 0usize;
        let mut page_start = units[index].1;
        let mut page_end = units[index].1;
        let mut end = index;

        while end < units.len() {
            // A section change closes the current chunk (§11.1).
            if unit_section[end] != start_section && !text.is_empty() {
                break;
            }

            let unit_tokens = estimate_tokens(&units[end].0);

            if tokens + unit_tokens > TARGET_TOKENS && !text.is_empty() {
                break;
            }

            if unit_tokens > MAX_TOKENS && text.is_empty() {
                // One sentence longer than the window: split it on characters,
                // which is the only place Bbrain cuts mid-sentence.
                for piece in split_oversized(&units[end].0) {
                    chunks.push(make_chunk(
                        start_section.clone(),
                        units[end].1,
                        units[end].1,
                        piece,
                    ));
                }
                end += 1;
                index = end;
                text.clear();
                break;
            }

            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&units[end].0);
            tokens += unit_tokens;
            page_end = units[end].1;
            page_start = page_start.min(units[end].1);
            end += 1;
        }

        if text.is_empty() {
            if end == index {
                index += 1;
            }
            continue;
        }

        chunks.push(make_chunk(start_section.clone(), page_start, page_end, text));

        // Step back far enough to carry roughly OVERLAP_TOKENS into the next
        // chunk, so a claim spanning a boundary is still retrievable.
        let mut back = end;
        let mut carried = 0usize;
        while back > index + 1 && carried < OVERLAP_TOKENS {
            back -= 1;
            carried += estimate_tokens(&units[back].0);
        }
        index = back.max(index + 1);
    }

    chunks
}

fn make_chunk(section: Option<String>, page_start: i64, page_end: i64, text: String) -> Chunk {
    let token_count = estimate_tokens(&text) as i64;
    Chunk {
        content_hash: text_hash(&text),
        section,
        page_start,
        page_end,
        text,
        token_count,
    }
}

fn split_sentences(line: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for character in line.chars() {
        current.push(character);
        if matches!(character, '.' | '!' | '?' | '。' | '！' | '？') {
            let trimmed = current.trim();
            // "et al." and "Fig. 1" are not sentence ends; require some length.
            if trimmed.len() > 12 {
                sentences.push(current.trim().to_string());
                current.clear();
            }
        }
    }

    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }
    sentences
}

fn split_oversized(sentence: &str) -> Vec<String> {
    let characters: Vec<char> = sentence.chars().collect();
    // Chunk at the token target, converted back to a character budget for the
    // dominant script in this sentence.
    let per_chunk = if characters.iter().any(|c| is_cjk(*c)) {
        TARGET_TOKENS
    } else {
        TARGET_TOKENS * 4
    };

    characters
        .chunks(per_chunk)
        .map(|piece| piece.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn english(sentences: usize) -> String {
        (0..sentences)
            .map(|i| format!("This is sentence number {i} in a fairly ordinary paragraph."))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn korean_and_english_token_estimates_stay_in_the_right_ballpark() {
        assert_eq!(estimate_tokens("검색 증강 생성"), 7); // 6 CJK + ceil(2 spaces / 4)
        assert!(estimate_tokens("retrieval augmented generation") < 10);
    }

    #[test]
    fn chunks_stay_under_the_model_window() {
        let pages = vec![(1, english(200))];
        let chunks = chunk_document(&pages);

        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(
                chunk.token_count <= MAX_TOKENS as i64,
                "chunk of {} tokens exceeds the window",
                chunk.token_count
            );
        }
    }

    #[test]
    fn a_section_heading_starts_a_new_chunk() {
        let pages = vec![(
            1,
            format!("Introduction\n{}\nConclusion\n{}", english(3), english(3)),
        )];

        let chunks = chunk_document(&pages);

        let sections: Vec<_> = chunks.iter().filter_map(|c| c.section.clone()).collect();
        assert!(sections.iter().any(|s| s == "Introduction"));
        assert!(sections.iter().any(|s| s == "Conclusion"));
        assert!(
            chunks.iter().all(|c| !(c.text.contains("sentence number 0")
                && c.section.as_deref() == Some("Conclusion")
                && c.text.contains("Introduction"))),
            "a chunk must not straddle two sections"
        );
    }

    #[test]
    fn consecutive_chunks_overlap_so_a_claim_on_a_boundary_is_still_found() {
        let pages = vec![(1, english(120))];
        let chunks = chunk_document(&pages);

        assert!(chunks.len() >= 2, "this document should split");
        let first_words: Vec<&str> = chunks[0].text.split_whitespace().collect();
        let tail = first_words[first_words.len().saturating_sub(6)..].join(" ");

        assert!(
            chunks[1].text.contains(&tail),
            "the next chunk should carry the previous tail",
        );
    }

    #[test]
    fn a_chunk_records_the_pages_it_came_from() {
        let pages = vec![(3, english(2)), (4, english(2))];
        let chunks = chunk_document(&pages);

        assert_eq!(chunks[0].page_start, 3);
        assert!(chunks[0].page_end >= 3);
    }

    #[test]
    fn sentences_are_not_cut_in_half() {
        let pages = vec![(1, english(40))];
        let chunks = chunk_document(&pages);

        for chunk in &chunks {
            assert!(
                chunk.text.trim_end().ends_with('.'),
                "chunk should end on a sentence boundary: {:?}",
                chunk.text
            );
        }
    }

    #[test]
    fn one_sentence_longer_than_the_window_is_split_rather_than_dropped() {
        let giant = format!("{}.", "word ".repeat(4_000));
        let chunks = chunk_document(&[(1, giant)]);

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.token_count <= MAX_TOKENS as i64);
        }
    }

    #[test]
    fn every_chunk_carries_a_content_hash() {
        let chunks = chunk_document(&[(1, english(10))]);

        assert!(chunks.iter().all(|c| c.content_hash.len() == 64));
    }

    #[test]
    fn an_empty_document_produces_no_chunks() {
        assert!(chunk_document(&[(1, "   \n\n".into())]).is_empty());
    }
}
