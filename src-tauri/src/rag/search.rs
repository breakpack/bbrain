use std::collections::HashMap;

use super::embedder::cosine;

/// Reciprocal Rank Fusion constant from DEVELOPMENT.md §11.2.
pub const RRF_K: f64 = 60.0;
pub const CANDIDATES_PER_SOURCE: usize = 40;
pub const MAX_CHUNKS: usize = 10;
pub const MAX_PAPERS: usize = 5;
/// MMR trade-off: 0.7 relevance, 0.3 novelty.
pub const MMR_LAMBDA: f32 = 0.7;
/// A chunk this similar to one already chosen is the same passage twice — it
/// costs context and adds nothing, so MMR alone is not enough to drop it (a
/// near-duplicate that is also highly relevant still wins on score).
pub const DUPLICATE_SIMILARITY: f32 = 0.95;

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub chunk_id: String,
    pub paper_id: String,
    pub page_start: i64,
    pub page_end: i64,
    pub section: Option<String>,
    pub text: String,
    pub embedding: Vec<f32>,
}

/// Fuses two ranked lists without needing their scores to be comparable — the
/// whole point of RRF, since BM25 and cosine live on different scales.
pub fn reciprocal_rank_fusion(vector: &[String], keyword: &[String]) -> Vec<(String, f64)> {
    let mut scores: HashMap<&str, f64> = HashMap::new();

    for list in [vector, keyword] {
        for (rank, id) in list.iter().enumerate() {
            *scores.entry(id.as_str()).or_insert(0.0) += 1.0 / (RRF_K + (rank + 1) as f64);
        }
    }

    let mut fused: Vec<(String, f64)> = scores
        .into_iter()
        .map(|(id, score)| (id.to_string(), score))
        .collect();

    // Ties broken by id so the ordering is deterministic across runs.
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused
}

/// Maximal Marginal Relevance: drops near-duplicate chunks so the context window
/// carries distinct evidence rather than the same paragraph five times (§11.2).
pub fn mmr(query: &[f32], candidates: &[Candidate], limit: usize) -> Vec<Candidate> {
    let mut selected: Vec<Candidate> = Vec::new();
    let mut remaining: Vec<&Candidate> = candidates.iter().collect();

    while selected.len() < limit && !remaining.is_empty() {
        let mut best_index = 0;
        let mut best_score = f32::MIN;

        let mut found = false;

        for (index, candidate) in remaining.iter().enumerate() {
            let relevance = cosine(query, &candidate.embedding);
            let redundancy = selected
                .iter()
                .map(|chosen| cosine(&chosen.embedding, &candidate.embedding))
                .fold(f32::MIN, f32::max)
                .max(0.0);

            if !selected.is_empty() && redundancy >= DUPLICATE_SIMILARITY {
                continue;
            }

            let score = if selected.is_empty() {
                relevance
            } else {
                MMR_LAMBDA * relevance - (1.0 - MMR_LAMBDA) * redundancy
            };

            if score > best_score {
                best_score = score;
                best_index = index;
                found = true;
            }
        }

        // Everything left is a duplicate of something already chosen.
        if !found {
            break;
        }

        selected.push(remaining.remove(best_index).clone());
    }

    selected
}

/// Caps how many papers the context may draw from, keeping the answer focused
/// and the citation list readable (§11.2).
pub fn cap_papers(chunks: Vec<Candidate>, max_papers: usize) -> Vec<Candidate> {
    let mut seen: Vec<String> = Vec::new();
    let mut kept = Vec::new();

    for chunk in chunks {
        if !seen.contains(&chunk.paper_id) {
            if seen.len() >= max_papers {
                continue;
            }
            seen.push(chunk.paper_id.clone());
        }
        kept.push(chunk);
    }

    kept
}

/// A model may only cite sources that were actually in its context. Anything
/// else is a hallucinated citation and is dropped (§11.3).
pub fn validate_citations(cited: &[String], context: &[Candidate]) -> Vec<String> {
    cited
        .iter()
        .filter(|id| context.iter().any(|chunk| &chunk.chunk_id == *id))
        .cloned()
        .collect()
}

/// FTS5 treats punctuation as syntax; quoting each term keeps a user's question
/// from becoming a query error.
pub fn fts_query(question: &str) -> String {
    let terms: Vec<String> = question
        .split_whitespace()
        .filter(|term| term.chars().any(char::is_alphanumeric))
        .map(|term| {
            let cleaned: String = term
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            format!("\"{cleaned}\"")
        })
        .filter(|term| term.len() > 2)
        .collect();

    if terms.is_empty() {
        return String::new();
    }
    terms.join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, paper: &str, embedding: Vec<f32>) -> Candidate {
        Candidate {
            chunk_id: id.into(),
            paper_id: paper.into(),
            page_start: 1,
            page_end: 1,
            section: None,
            text: format!("text of {id}"),
            embedding,
        }
    }

    #[test]
    fn fusion_rewards_a_chunk_that_both_searches_found() {
        let vector = vec!["a".to_string(), "b".to_string()];
        let keyword = vec!["b".to_string(), "c".to_string()];

        let fused = reciprocal_rank_fusion(&vector, &keyword);

        assert_eq!(fused[0].0, "b", "found by both lists, so it should rank first");
    }

    #[test]
    fn fusion_keeps_results_that_only_one_search_found() {
        let fused = reciprocal_rank_fusion(&["a".into()], &["z".into()]);

        let ids: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"a") && ids.contains(&"z"));
    }

    #[test]
    fn fusion_is_deterministic_for_tied_scores() {
        let first = reciprocal_rank_fusion(&["a".into(), "b".into()], &[]);
        let second = reciprocal_rank_fusion(&["a".into(), "b".into()], &[]);

        assert_eq!(first, second);
    }

    #[test]
    fn mmr_prefers_a_novel_chunk_over_a_near_duplicate() {
        let query = vec![1.0f32, 0.0];
        let candidates = vec![
            candidate("near-1", "p1", vec![0.99, 0.14]),
            candidate("near-2", "p1", vec![0.98, 0.19]), // almost the same chunk
            candidate("other", "p2", vec![0.6, 0.8]),    // less relevant, but new
        ];

        let selected = mmr(&query, &candidates, 2);

        assert_eq!(selected[0].chunk_id, "near-1");
        assert_eq!(
            selected[1].chunk_id, "other",
            "the second pick should add information, not repeat it"
        );
    }

    #[test]
    fn mmr_returns_everything_when_the_limit_exceeds_the_candidates() {
        let selected = mmr(
            &[1.0, 0.0],
            &[candidate("a", "p1", vec![1.0, 0.0])],
            MAX_CHUNKS,
        );

        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn the_context_draws_from_at_most_five_papers() {
        let chunks: Vec<Candidate> = (0..8)
            .map(|i| candidate(&format!("c{i}"), &format!("p{i}"), vec![1.0, 0.0]))
            .collect();

        let capped = cap_papers(chunks, MAX_PAPERS);

        let papers: std::collections::HashSet<_> =
            capped.iter().map(|c| c.paper_id.clone()).collect();
        assert_eq!(papers.len(), MAX_PAPERS);
    }

    #[test]
    fn several_chunks_from_one_paper_do_not_count_against_the_paper_cap() {
        let chunks = vec![
            candidate("c1", "p1", vec![1.0, 0.0]),
            candidate("c2", "p1", vec![1.0, 0.0]),
            candidate("c3", "p2", vec![1.0, 0.0]),
        ];

        let capped = cap_papers(chunks, 2);

        assert_eq!(capped.len(), 3);
    }

    #[test]
    fn a_citation_that_was_not_in_the_context_is_rejected() {
        let context = vec![candidate("real", "p1", vec![1.0, 0.0])];

        let validated = validate_citations(
            &["real".to_string(), "invented".to_string()],
            &context,
        );

        assert_eq!(validated, vec!["real"]);
    }

    #[test]
    fn a_question_with_punctuation_produces_a_usable_fts_query() {
        let query = fts_query("What is RAG, exactly?");

        assert!(query.contains("\"What\""));
        assert!(query.contains("\"RAG\""));
        assert!(!query.contains('?'));
        assert!(!query.contains(','));
    }

    #[test]
    fn a_question_of_only_punctuation_yields_an_empty_query_rather_than_an_error() {
        assert_eq!(fts_query("??? !!!"), "");
    }
}
