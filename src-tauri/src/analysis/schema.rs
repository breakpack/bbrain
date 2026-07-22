use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, Result};

pub const SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub claim: String,
    #[serde(default)]
    pub evidence_pages: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub finding: String,
    #[serde(default)]
    pub evidence_pages: Vec<i64>,
}

/// `PaperAnalysisV1` from DEVELOPMENT.md §10.3. Provider output is validated
/// against this before it is stored; the Markdown is then rendered from the
/// validated JSON, never taken from the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperAnalysisV1 {
    pub schema_version: String,
    pub short_summary: String,
    pub detailed_summary: String,
    pub research_problem: String,
    pub contributions: Vec<Claim>,
    pub methodology: String,
    pub results: Vec<Finding>,
    pub limitations: Vec<String>,
    pub keywords: Vec<String>,
    pub suggested_tags: Vec<String>,
    pub follow_up_questions: Vec<String>,
}

/// JSON Schema sent to the provider. `additionalProperties: false` plus a full
/// `required` list is what makes OpenAI's strict mode and Anthropic's tool input
/// reject a malformed answer at the source.
pub fn json_schema() -> serde_json::Value {
    let string_array = json!({ "type": "array", "items": { "type": "string" } });
    let pages = json!({ "type": "array", "items": { "type": "integer" } });

    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schemaVersion", "shortSummary", "detailedSummary", "researchProblem",
            "contributions", "methodology", "results", "limitations", "keywords",
            "suggestedTags", "followUpQuestions"
        ],
        "properties": {
            "schemaVersion": { "type": "string", "enum": ["1"] },
            "shortSummary": { "type": "string" },
            "detailedSummary": { "type": "string" },
            "researchProblem": { "type": "string" },
            "contributions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["claim", "evidencePages"],
                    "properties": {
                        "claim": { "type": "string" },
                        "evidencePages": pages,
                    }
                }
            },
            "methodology": { "type": "string" },
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["finding", "evidencePages"],
                    "properties": {
                        "finding": { "type": "string" },
                        "evidencePages": pages,
                    }
                }
            },
            "limitations": string_array,
            "keywords": string_array,
            "suggestedTags": string_array,
            "followUpQuestions": string_array,
        }
    })
}

/// Validates and normalizes provider output. Evidence pages that fall outside
/// the document are dropped rather than trusted — a citation must be verifiable
/// (DEVELOPMENT.md §3).
pub fn validate(value: serde_json::Value, page_count: i64) -> Result<PaperAnalysisV1> {
    let mut analysis: PaperAnalysisV1 = serde_json::from_value(value)
        .map_err(|e| AppError::ProviderResponse(format!("analysis schema: {e}")))?;

    if analysis.schema_version != SCHEMA_VERSION {
        return Err(AppError::ProviderResponse(format!(
            "unexpected schema version {}",
            analysis.schema_version
        )));
    }

    if analysis.short_summary.trim().is_empty() {
        return Err(AppError::ProviderResponse("empty summary".into()));
    }

    let in_range = |page: &i64| *page >= 1 && *page <= page_count;

    for contribution in &mut analysis.contributions {
        contribution.evidence_pages.retain(in_range);
        contribution.evidence_pages.sort_unstable();
        contribution.evidence_pages.dedup();
    }
    for result in &mut analysis.results {
        result.evidence_pages.retain(in_range);
        result.evidence_pages.sort_unstable();
        result.evidence_pages.dedup();
    }

    analysis.suggested_tags.retain(|tag| !tag.trim().is_empty());
    analysis.keywords.retain(|keyword| !keyword.trim().is_empty());

    Ok(analysis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> serde_json::Value {
        json!({
            "schemaVersion": "1",
            "shortSummary": "요약",
            "detailedSummary": "자세한 요약",
            "researchProblem": "연구 문제",
            "contributions": [{ "claim": "기여", "evidencePages": [2, 2, 99, 0] }],
            "methodology": "방법론",
            "results": [{ "finding": "결과", "evidencePages": [3] }],
            "limitations": ["한계"],
            "keywords": ["rag", ""],
            "suggestedTags": ["retrieval", "  "],
            "followUpQuestions": ["다음 질문"]
        })
    }

    #[test]
    fn accepts_a_well_formed_analysis() {
        let analysis = validate(payload(), 10).unwrap();

        assert_eq!(analysis.short_summary, "요약");
        assert_eq!(analysis.results[0].evidence_pages, vec![3]);
    }

    #[test]
    fn evidence_pages_outside_the_document_are_dropped() {
        let analysis = validate(payload(), 10).unwrap();

        // 99 is past the end and 0 is not a page; the duplicate 2 collapses.
        assert_eq!(analysis.contributions[0].evidence_pages, vec![2]);
    }

    #[test]
    fn blank_keywords_and_tags_are_removed() {
        let analysis = validate(payload(), 10).unwrap();

        assert_eq!(analysis.keywords, vec!["rag"]);
        assert_eq!(analysis.suggested_tags, vec!["retrieval"]);
    }

    #[test]
    fn a_missing_field_is_rejected_rather_than_defaulted() {
        let mut broken = payload();
        broken["methodology"] = serde_json::Value::Null;

        let error = validate(broken, 10).unwrap_err();
        assert!(matches!(error, AppError::ProviderResponse(_)));
    }

    #[test]
    fn a_wrong_schema_version_is_rejected() {
        let mut broken = payload();
        broken["schemaVersion"] = json!("2");

        assert!(validate(broken, 10).is_err());
    }

    #[test]
    fn an_empty_summary_is_rejected() {
        let mut broken = payload();
        broken["shortSummary"] = json!("   ");

        assert!(validate(broken, 10).is_err());
    }

    #[test]
    fn the_wire_schema_forbids_extra_properties() {
        let schema = json_schema();

        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["required"].as_array().unwrap().len(), 11);
    }
}
