use super::schema::PaperAnalysisV1;
use crate::db::paper_repo::Paper;

/// Renders the note from validated JSON, deterministically. The model's own
/// Markdown is never treated as canonical data (DEVELOPMENT.md §10.3), so the
/// same analysis always produces the same note — which is what makes the
/// Obsidian merge stable.
pub fn render(paper: &Paper, analysis: &PaperAnalysisV1, related: &[String]) -> String {
    let mut out = String::new();

    // 1. Bibliography
    out.push_str(&format!("# {}\n\n", paper.title));
    let mut bibliography = Vec::new();
    if !paper.authors.is_empty() {
        bibliography.push(paper.authors.join(", "));
    }
    if let Some(year) = paper.year {
        bibliography.push(year.to_string());
    }
    if let Some(venue) = &paper.venue {
        bibliography.push(venue.clone());
    }
    if let Some(doi) = &paper.doi {
        bibliography.push(format!("DOI: {doi}"));
    }
    if !bibliography.is_empty() {
        out.push_str(&format!("{}\n\n", bibliography.join(" · ")));
    }

    // 2. Summary
    out.push_str("## 요약\n\n");
    out.push_str(&format!("{}\n\n", analysis.short_summary.trim()));
    if !analysis.detailed_summary.trim().is_empty() {
        out.push_str(&format!("{}\n\n", analysis.detailed_summary.trim()));
    }

    // 3. Research problem
    out.push_str("## 연구 문제\n\n");
    out.push_str(&format!("{}\n\n", analysis.research_problem.trim()));

    // 4. Contributions, with page links
    out.push_str("## 기여점\n\n");
    if analysis.contributions.is_empty() {
        out.push_str("추출된 기여점이 없습니다.\n\n");
    } else {
        for contribution in &analysis.contributions {
            out.push_str(&format!(
                "- {}{}\n",
                contribution.claim.trim(),
                page_links(&contribution.evidence_pages)
            ));
        }
        out.push('\n');
    }

    // 5. Methodology
    out.push_str("## 방법론\n\n");
    out.push_str(&format!("{}\n\n", analysis.methodology.trim()));

    // 6. Results, with page links
    out.push_str("## 주요 결과\n\n");
    if analysis.results.is_empty() {
        out.push_str("추출된 결과가 없습니다.\n\n");
    } else {
        for result in &analysis.results {
            out.push_str(&format!(
                "- {}{}\n",
                result.finding.trim(),
                page_links(&result.evidence_pages)
            ));
        }
        out.push('\n');
    }

    // 7. Limitations
    out.push_str("## 한계\n\n");
    if analysis.limitations.is_empty() {
        out.push_str("명시된 한계가 없습니다.\n\n");
    } else {
        for limitation in &analysis.limitations {
            out.push_str(&format!("- {}\n", limitation.trim()));
        }
        out.push('\n');
    }

    // 8. Keywords and tags
    out.push_str("## 키워드와 태그\n\n");
    if !analysis.keywords.is_empty() {
        out.push_str(&format!("키워드: {}\n\n", analysis.keywords.join(", ")));
    }
    if !analysis.suggested_tags.is_empty() {
        out.push_str(&format!(
            "제안 태그: {}\n\n",
            analysis
                .suggested_tags
                .iter()
                .map(|tag| format!("#{}", tag.replace(' ', "-")))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }

    // 9. Follow-up questions
    out.push_str("## 후속 연구 질문\n\n");
    if analysis.follow_up_questions.is_empty() {
        out.push_str("제안된 질문이 없습니다.\n\n");
    } else {
        for question in &analysis.follow_up_questions {
            out.push_str(&format!("- {}\n", question.trim()));
        }
        out.push('\n');
    }

    // 10. Related papers — plain wiki links, so Obsidian's graph picks them up
    // without a plugin (DEVELOPMENT.md §13.4).
    out.push_str("## 관련 논문\n\n");
    if related.is_empty() {
        out.push_str("아직 관련 논문이 없습니다.\n");
    } else {
        for title in related {
            out.push_str(&format!("- [[{title}]]\n"));
        }
    }

    out
}

fn page_links(pages: &[i64]) -> String {
    if pages.is_empty() {
        return String::new();
    }
    format!(
        " ({})",
        pages
            .iter()
            .map(|page| format!("{page}쪽"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::schema::{Claim, Finding};
    use crate::db::paper_repo::ImportStatus;

    fn paper() -> Paper {
        Paper {
            id: "p1".into(),
            sha256: "hash".into(),
            title: "Attention Is All You Need".into(),
            import_status: ImportStatus::Ready,
            page_count: Some(11),
            is_favorite: false,
            last_opened_at: None,
            created_at: "2026-07-14T00:00:00Z".into(),
            updated_at: "2026-07-14T00:00:00Z".into(),
            authors: vec!["Vaswani".into(), "Shazeer".into()],
            year: Some(2017),
            venue: Some("NeurIPS".into()),
            doi: None,
            abstract_text: None,
            group_ids: vec![],
            tags: vec![],
        }
    }

    fn analysis() -> PaperAnalysisV1 {
        PaperAnalysisV1 {
            schema_version: "1".into(),
            short_summary: "트랜스포머를 제안한다.".into(),
            detailed_summary: "자세한 요약".into(),
            research_problem: "순환 신경망의 병렬화 한계".into(),
            contributions: vec![Claim {
                claim: "self-attention만으로 번역 성능을 낸다".into(),
                evidence_pages: vec![2, 3],
            }],
            methodology: "인코더-디코더 구조".into(),
            results: vec![Finding {
                finding: "BLEU 28.4 달성".into(),
                evidence_pages: vec![8],
            }],
            limitations: vec!["긴 문서에서 계산량이 증가한다".into()],
            keywords: vec!["attention".into()],
            suggested_tags: vec!["neural machine translation".into()],
            tag_insights: vec![],
            follow_up_questions: vec!["더 긴 문맥에서도 성립하는가?".into()],
        }
    }

    #[test]
    fn renders_all_ten_sections_in_order() {
        let markdown = render(&paper(), &analysis(), &["Related".into()]);

        let expected = [
            "# Attention Is All You Need",
            "## 요약",
            "## 연구 문제",
            "## 기여점",
            "## 방법론",
            "## 주요 결과",
            "## 한계",
            "## 키워드와 태그",
            "## 후속 연구 질문",
            "## 관련 논문",
        ];

        let mut cursor = 0;
        for heading in expected {
            let found = markdown[cursor..]
                .find(heading)
                .unwrap_or_else(|| panic!("missing or out-of-order section: {heading}"));
            cursor += found + heading.len();
        }
    }

    #[test]
    fn bibliography_joins_only_the_fields_that_exist() {
        let markdown = render(&paper(), &analysis(), &[]);

        assert!(markdown.contains("Vaswani, Shazeer · 2017 · NeurIPS"));
        assert!(!markdown.contains("DOI"));
    }

    #[test]
    fn evidence_pages_are_rendered_as_page_links() {
        let markdown = render(&paper(), &analysis(), &[]);

        assert!(markdown.contains("self-attention만으로 번역 성능을 낸다 (2쪽, 3쪽)"));
        assert!(markdown.contains("BLEU 28.4 달성 (8쪽)"));
    }

    #[test]
    fn related_papers_use_plain_wiki_links() {
        let markdown = render(&paper(), &analysis(), &["Dense Passage Retrieval".into()]);

        assert!(markdown.contains("- [[Dense Passage Retrieval]]"));
    }

    #[test]
    fn rendering_is_deterministic() {
        let first = render(&paper(), &analysis(), &["A".into()]);
        let second = render(&paper(), &analysis(), &["A".into()]);

        assert_eq!(first, second);
    }

    #[test]
    fn empty_sections_say_so_rather_than_vanishing() {
        let mut empty = analysis();
        empty.contributions.clear();
        empty.results.clear();
        empty.limitations.clear();

        let markdown = render(&paper(), &empty, &[]);

        assert!(markdown.contains("추출된 기여점이 없습니다"));
        assert!(markdown.contains("추출된 결과가 없습니다"));
        assert!(markdown.contains("명시된 한계가 없습니다"));
    }

    #[test]
    fn suggested_tags_become_hashtags_without_spaces() {
        let markdown = render(&paper(), &analysis(), &[]);

        assert!(markdown.contains("#neural-machine-translation"));
    }
}
