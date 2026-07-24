pub mod handlers;
pub mod queue;
pub mod runner;

use rusqlite::Connection;
use serde::Serialize;

use crate::error::Result;
pub use queue::{Job, JobStatus, JobType};
pub use runner::JobRunner;

/// Bump when a step's output format changes, so existing papers re-run it
/// instead of being skipped by the idempotency key.
// 6: footnote/imprint band moves to the end of the page's reading order.
pub const EXTRACT_VERSION: &str = "6";
pub const THUMBNAIL_VERSION: &str = "1";
// Bumped with EXTRACT: a re-extraction that fixes the reading order must also
// refresh the embeddings built from that text.
pub const EMBED_VERSION: &str = "5";
pub const ANALYZE_VERSION: &str = "1";
pub const RELATIONS_VERSION: &str = "1";
pub const OBSIDIAN_VERSION: &str = "1";

/// The pipeline a freshly imported paper walks through (DEVELOPMENT.md §10.1).
/// Each step enqueues the next only after it commits, so a crash resumes at the
/// last completed step rather than restarting the paper.
pub fn enqueue_import_pipeline(conn: &Connection, paper_ids: &[String]) -> Result<()> {
    for paper_id in paper_ids {
        queue::enqueue(conn, Some(paper_id), JobType::Extract, EXTRACT_VERSION)?;
        queue::enqueue(conn, Some(paper_id), JobType::Thumbnail, THUMBNAIL_VERSION)?;
    }
    Ok(())
}

/// Re-processes existing papers after an extraction-format change. Enqueuing at
/// the current EXTRACT_VERSION is idempotency-gated, so each paper re-extracts
/// at most once per version bump; the re-extraction then refreshes embeddings
/// (local, free) while `submit_extraction` deliberately keeps the existing
/// analysis (which would cost provider calls). Settled papers only — a paper
/// still mid-import already has the current pipeline queued.
pub fn reprocess_existing_papers(conn: &Connection) -> Result<usize> {
    let mut statement = conn.prepare(
        "SELECT id FROM papers
         WHERE import_status IN ('ready', 'partial', 'failed')",
    )?;
    let paper_ids: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut enqueued = 0;
    for paper_id in &paper_ids {
        // Thumbnail format is unchanged, so re-extraction alone; the idempotency
        // key skips papers already extracted at this version.
        if queue::enqueue(conn, Some(paper_id), JobType::Extract, EXTRACT_VERSION)?.is_some() {
            enqueued += 1;
        }
    }

    if enqueued > 0 {
        tracing::info!(enqueued, "re-extracting papers for the updated extraction format");
    }
    Ok(enqueued)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub job_id: String,
    pub paper_id: Option<String>,
    pub job_type: JobType,
    pub status: JobStatus,
    pub error_code: Option<String>,
    pub pending: i64,
}

/// Payload for a frontend-assisted job: the webview does the PDF.js work and
/// calls back with the result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendJobRequest {
    pub job_id: String,
    pub paper_id: String,
    pub job_type: JobType,
}

pub const JOB_PROGRESS: &str = "job://progress";
pub const JOB_FRONTEND_REQUEST: &str = "job://frontend-request";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::paper_repo::{self, ImportStatus};
    use crate::db::Database;

    fn seed(conn: &Connection, id: &str, status: ImportStatus) {
        paper_repo::insert(conn, id, id, "/tmp/x.pdf", "Paper", status).unwrap();
    }

    #[test]
    fn reprocess_re_extracts_settled_papers_once_per_version() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed(&conn, "ready", ImportStatus::Ready);
        seed(&conn, "partial", ImportStatus::Partial);

        let first = reprocess_existing_papers(&conn).unwrap();
        let second = reprocess_existing_papers(&conn).unwrap();

        assert_eq!(first, 2, "both settled papers are queued for re-extraction");
        assert_eq!(second, 0, "the idempotency key blocks a second pass");
    }

    #[test]
    fn reprocess_skips_papers_still_mid_import() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed(&conn, "extracting", ImportStatus::Extracting);
        seed(&conn, "analyzing", ImportStatus::Analyzing);

        assert_eq!(reprocess_existing_papers(&conn).unwrap(), 0);
    }

    #[test]
    fn reprocess_re_extracts_a_failed_paper() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed(&conn, "failed", ImportStatus::Failed);

        assert_eq!(reprocess_existing_papers(&conn).unwrap(), 1);
    }
}
