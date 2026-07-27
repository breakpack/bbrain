use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::ids::new_id;
use crate::time::{now_iso8601, plus_seconds};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    /// Page text, sentences and coordinates. Performed by PDF.js in the webview
    /// and persisted here, so Bbrain has exactly one PDF engine.
    Extract,
    Thumbnail,
    Embed,
    Analyze,
    Relations,
    ObsidianSync,
}

impl JobType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extract => "extract",
            Self::Thumbnail => "thumbnail",
            Self::Embed => "embed",
            Self::Analyze => "analyze",
            Self::Relations => "relations",
            Self::ObsidianSync => "obsidian_sync",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "extract" => Self::Extract,
            "thumbnail" => Self::Thumbnail,
            "embed" => Self::Embed,
            "analyze" => Self::Analyze,
            "relations" => Self::Relations,
            "obsidian_sync" => Self::ObsidianSync,
            _ => return None,
        })
    }

    /// Default priorities from DEVELOPMENT.md §10.2. Interactive work (chat,
    /// current-page translation) does not queue — the user is waiting, so it
    /// runs directly on its own task.
    pub fn priority(self) -> i64 {
        match self {
            Self::Extract | Self::Thumbnail | Self::Embed | Self::Analyze => 40,
            Self::Relations | Self::ObsidianSync => 20,
        }
    }

    /// Whether the job needs the webview (PDF.js) to do the actual work.
    pub fn is_frontend_assisted(self) -> bool {
        matches!(self, Self::Extract | Self::Thumbnail)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    WaitingForKey,
    Failed,
    Completed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingForKey => "waiting_for_key",
            Self::Failed => "failed",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub paper_id: Option<String>,
    pub job_type: JobType,
    pub status: JobStatus,
    pub priority: i64,
    pub attempts: i64,
    pub payload: String,
    pub error_code: Option<String>,
}

pub const MAX_ATTEMPTS: i64 = 3;

/// Every step is idempotent on `paper_id + content_hash + task_version`
/// (DEVELOPMENT.md §10.2), so re-enqueuing the same work is a no-op instead of
/// a duplicate.
pub fn idempotency_key(paper_id: &str, job_type: JobType, version: &str) -> String {
    format!("{paper_id}:{}:{version}", job_type.as_str())
}

pub fn enqueue(
    conn: &Connection,
    paper_id: Option<&str>,
    job_type: JobType,
    version: &str,
) -> Result<Option<String>> {
    let key = paper_id.map(|id| idempotency_key(id, job_type, version));
    let now = now_iso8601();
    let id = new_id();

    let inserted = conn.execute(
        "INSERT OR IGNORE INTO jobs
           (id, paper_id, type, status, priority, attempts, payload, idempotency_key, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'queued', ?4, 0, '{}', ?5, ?6, ?6)",
        params![
            id,
            paper_id,
            job_type.as_str(),
            job_type.priority(),
            key,
            now
        ],
    )?;

    Ok((inserted > 0).then_some(id))
}

/// Claims the highest-priority runnable job. `not_before` gates jobs waiting out
/// a backoff window.
pub fn claim_next(conn: &Connection) -> Result<Option<Job>> {
    let now = now_iso8601();

    let job = conn
        .query_row(
            "SELECT id, paper_id, type, status, priority, attempts, payload, error_code
             FROM jobs
             WHERE status = 'queued' AND (not_before IS NULL OR not_before <= ?1)
             ORDER BY priority DESC, created_at ASC
             LIMIT 1",
            params![now],
            |row| {
                let job_type: String = row.get(2)?;
                Ok(Job {
                    id: row.get(0)?,
                    paper_id: row.get(1)?,
                    job_type: JobType::from_str(&job_type).unwrap_or(JobType::Extract),
                    status: JobStatus::Running,
                    priority: row.get(4)?,
                    attempts: row.get(5)?,
                    payload: row.get(6)?,
                    error_code: row.get(7)?,
                })
            },
        )
        .optional()?;

    let Some(job) = job else { return Ok(None) };

    let claimed = conn.execute(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1, updated_at = ?1
         WHERE id = ?2 AND status = 'queued'",
        params![now, job.id],
    )?;

    // Another worker won the race.
    if claimed == 0 {
        return Ok(None);
    }

    Ok(Some(job))
}

pub fn complete(conn: &Connection, job_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE jobs SET status = 'completed', error_code = NULL, updated_at = ?1 WHERE id = ?2",
        params![now_iso8601(), job_id],
    )?;
    Ok(())
}

/// How a failure is handled depends on what failed (DEVELOPMENT.md §10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Auth, permission, unusable model: ask the user, never spin.
    WaitForKey,
    /// Rate limit, timeout, connection, provider 5xx: exponential backoff.
    Backoff { seconds: u64 },
    /// Corrupt input or a bad structured response after repair: terminal.
    GiveUp,
}

pub fn classify(error: &AppError, attempts: i64) -> Retry {
    match error {
        // CredentialStore included: a keychain prompt the user dismissed (or a
        // re-signed dev binary macOS no longer trusts) resolves by user action
        // or a relaunch — terminal failure here left papers stuck in
        // waiting_for_ai with no path back.
        AppError::ProviderAuth | AppError::ModelUnsupported(_) | AppError::CredentialStore(_) => {
            Retry::WaitForKey
        }

        AppError::ProviderRateLimit {
            retry_after_seconds,
        } => Retry::Backoff {
            seconds: retry_after_seconds.unwrap_or_else(|| backoff_seconds(attempts)),
        },

        AppError::ProviderUnavailable | AppError::Network => Retry::Backoff {
            seconds: backoff_seconds(attempts),
        },

        // A rejected PDF will not become valid on its own, and a response that
        // fails schema validation after one repair is not worth retrying.
        _ => Retry::GiveUp,
    }
}

/// 2s, 4s, 8s — capped so a long outage does not push a job hours out.
pub fn backoff_seconds(attempts: i64) -> u64 {
    let exponent = attempts.clamp(1, 6) as u32;
    2u64.saturating_pow(exponent).min(300)
}

pub fn fail(conn: &Connection, job: &Job, error: &AppError) -> Result<JobStatus> {
    let now = now_iso8601();
    let code = format!("{:?}", error.code());

    let status = match classify(error, job.attempts) {
        Retry::WaitForKey => {
            conn.execute(
                "UPDATE jobs SET status = 'waiting_for_key', error_code = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![code, now, job.id],
            )?;
            JobStatus::WaitingForKey
        }

        Retry::Backoff { seconds } if job.attempts < MAX_ATTEMPTS => {
            conn.execute(
                "UPDATE jobs SET status = 'queued', error_code = ?1, not_before = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![code, plus_seconds(seconds), now, job.id],
            )?;
            JobStatus::Queued
        }

        _ => {
            conn.execute(
                "UPDATE jobs SET status = 'failed', error_code = ?1, updated_at = ?2 WHERE id = ?3",
                params![code, now, job.id],
            )?;
            JobStatus::Failed
        }
    };

    Ok(status)
}

/// A process that dies mid-job leaves it `running` with no worker. Since a job's
/// result is only committed in its own transaction, an interrupted job did not
/// take effect and can safely go back to `queued` (DEVELOPMENT.md §10.1).
pub fn recover_running(conn: &Connection) -> Result<usize> {
    let recovered = conn.execute(
        "UPDATE jobs SET status = 'queued', not_before = NULL, updated_at = ?1
         WHERE status = 'running'",
        params![now_iso8601()],
    )?;

    if recovered > 0 {
        tracing::info!(recovered, "requeued jobs interrupted by a previous run");
    }
    Ok(recovered)
}

/// Failures whose cause is typically gone after a relaunch (keychain access
/// restored, network back, a since-fixed storage bug) get one fresh chance per
/// launch. Terminal causes — corrupt PDFs, schema-invalid responses — stay
/// failed so a bad input never spins across launches.
pub fn recover_transient_failures(conn: &Connection) -> Result<usize> {
    let recovered = conn.execute(
        "UPDATE jobs SET status = 'queued', attempts = 0, not_before = NULL, updated_at = ?1
         WHERE status = 'failed'
           AND error_code IN ('CredentialStore', 'Network', 'Storage', 'ProviderUnavailable')",
        params![now_iso8601()],
    )?;

    if recovered > 0 {
        tracing::info!(recovered, "requeued jobs that failed for transient reasons");
    }
    Ok(recovered)
}

/// Once a key arrives, everything that stalled on it becomes runnable again.
pub fn release_waiting_for_key(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE jobs SET status = 'queued', attempts = 0, error_code = NULL, updated_at = ?1
         WHERE status = 'waiting_for_key'",
        params![now_iso8601()],
    )?)
}

pub fn retry(conn: &Connection, job_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE jobs SET status = 'queued', attempts = 0, error_code = NULL, not_before = NULL,
                updated_at = ?1
         WHERE id = ?2 AND status IN ('failed', 'waiting_for_key', 'cancelled')",
        params![now_iso8601(), job_id],
    )?;
    Ok(())
}

pub fn cancel(conn: &Connection, job_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE jobs SET status = 'cancelled', updated_at = ?1
         WHERE id = ?2 AND status IN ('queued', 'running', 'waiting_for_key')",
        params![now_iso8601(), job_id],
    )?;
    Ok(())
}

pub fn pending_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM jobs WHERE status IN ('queued', 'running')",
        [],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::paper_repo::{self, ImportStatus};
    use crate::db::Database;

    fn seed_paper(conn: &Connection, id: &str) {
        paper_repo::insert(conn, id, id, "/tmp/x.pdf", "Paper", ImportStatus::Extracting).unwrap();
    }

    #[test]
    fn enqueuing_the_same_work_twice_is_a_no_op() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");

        let first = enqueue(&conn, Some("p1"), JobType::Extract, "v1").unwrap();
        let second = enqueue(&conn, Some("p1"), JobType::Extract, "v1").unwrap();

        assert!(first.is_some());
        assert!(second.is_none(), "the idempotency key should suppress the duplicate");
        assert_eq!(pending_count(&conn).unwrap(), 1);
    }

    #[test]
    fn a_new_task_version_enqueues_fresh_work() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");

        enqueue(&conn, Some("p1"), JobType::Analyze, "v1").unwrap();
        let reanalysis = enqueue(&conn, Some("p1"), JobType::Analyze, "v2").unwrap();

        assert!(reanalysis.is_some());
        assert_eq!(pending_count(&conn).unwrap(), 2);
    }

    #[test]
    fn claiming_takes_the_highest_priority_job_first() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");

        enqueue(&conn, Some("p1"), JobType::Relations, "v1").unwrap();
        enqueue(&conn, Some("p1"), JobType::Extract, "v1").unwrap();

        let claimed = claim_next(&conn).unwrap().unwrap();

        assert_eq!(claimed.job_type, JobType::Extract);
        assert_eq!(claimed.attempts, 0, "attempts is the count before this run");
    }

    #[test]
    fn a_claimed_job_is_not_claimed_again() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        enqueue(&conn, Some("p1"), JobType::Extract, "v1").unwrap();

        assert!(claim_next(&conn).unwrap().is_some());
        assert!(claim_next(&conn).unwrap().is_none());
    }

    #[test]
    fn an_interrupted_job_is_requeued_on_the_next_run() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        enqueue(&conn, Some("p1"), JobType::Extract, "v1").unwrap();
        claim_next(&conn).unwrap().unwrap();

        // Simulate a crash: the process dies with the job still `running`.
        assert_eq!(recover_running(&conn).unwrap(), 1);

        assert!(claim_next(&conn).unwrap().is_some());
    }

    #[test]
    fn auth_failures_wait_for_a_key_instead_of_retrying() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        enqueue(&conn, Some("p1"), JobType::Analyze, "v1").unwrap();
        let job = claim_next(&conn).unwrap().unwrap();

        let status = fail(&conn, &job, &AppError::ProviderAuth).unwrap();

        assert_eq!(status, JobStatus::WaitingForKey);
        assert!(claim_next(&conn).unwrap().is_none(), "must not spin on a bad key");
    }

    /// 키체인 접근 거부는 사용자가 승인하거나 재실행하면 풀리는 문제다.
    /// 영구 실패로 종결하면 논문이 waiting_for_ai에 영원히 갇힌다.
    #[test]
    fn a_keychain_denial_waits_instead_of_failing_terminally() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        enqueue(&conn, Some("p1"), JobType::Analyze, "v1").unwrap();
        let job = claim_next(&conn).unwrap().unwrap();

        let status = fail(
            &conn,
            &job,
            &AppError::CredentialStore(keyring::Error::NoEntry),
        )
        .unwrap();

        assert_eq!(status, JobStatus::WaitingForKey);
    }

    /// 일시적 원인(키체인/네트워크/스토리지)으로 failed된 잡은 앱 재시작 시
    /// 한 번 다시 시도한다. 손상된 PDF 같은 영구 원인은 그대로 둔다.
    #[test]
    fn transient_failures_get_one_fresh_try_per_launch() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");

        enqueue(&conn, Some("p1"), JobType::Analyze, "v1").unwrap();
        enqueue(&conn, Some("p1"), JobType::Extract, "v1").unwrap();

        // Old builds wrote CredentialStore failures as terminal; simulate one,
        // plus a genuinely terminal rejection that must stay failed.
        conn.execute(
            "UPDATE jobs SET status = 'failed', error_code = 'CredentialStore' WHERE type = 'analyze'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE jobs SET status = 'failed', error_code = 'Rejected' WHERE type = 'extract'",
            [],
        )
        .unwrap();

        assert_eq!(recover_transient_failures(&conn).unwrap(), 1);

        let requeued = claim_next(&conn).unwrap().unwrap();
        assert_eq!(requeued.job_type, JobType::Analyze);
        assert!(claim_next(&conn).unwrap().is_none(), "rejected extract stays failed");
    }

    #[test]
    fn configuring_a_key_releases_everything_that_stalled_on_it() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        enqueue(&conn, Some("p1"), JobType::Analyze, "v1").unwrap();
        let job = claim_next(&conn).unwrap().unwrap();
        fail(&conn, &job, &AppError::ProviderAuth).unwrap();

        assert_eq!(release_waiting_for_key(&conn).unwrap(), 1);
        assert!(claim_next(&conn).unwrap().is_some());
    }

    #[test]
    fn transient_failures_back_off_and_then_give_up() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        enqueue(&conn, Some("p1"), JobType::Analyze, "v1").unwrap();

        let mut last = JobStatus::Queued;
        for _ in 0..MAX_ATTEMPTS + 1 {
            // Clear the backoff gate so the test does not wait on wall-clock time.
            conn.execute("UPDATE jobs SET not_before = NULL", []).unwrap();
            let Some(job) = claim_next(&conn).unwrap() else { break };
            last = fail(&conn, &job, &AppError::ProviderUnavailable).unwrap();
        }

        assert_eq!(last, JobStatus::Failed);
        assert!(claim_next(&conn).unwrap().is_none());
    }

    #[test]
    fn a_rate_limit_honours_the_provider_retry_after() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        enqueue(&conn, Some("p1"), JobType::Analyze, "v1").unwrap();
        let job = claim_next(&conn).unwrap().unwrap();

        let status = fail(
            &conn,
            &job,
            &AppError::ProviderRateLimit {
                retry_after_seconds: Some(120),
            },
        )
        .unwrap();

        assert_eq!(status, JobStatus::Queued);
        // Gated by not_before, so it is queued but not yet runnable.
        assert!(claim_next(&conn).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_input_is_terminal_on_the_first_failure() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        enqueue(&conn, Some("p1"), JobType::Extract, "v1").unwrap();
        let job = claim_next(&conn).unwrap().unwrap();

        let status = fail(
            &conn,
            &job,
            &AppError::Rejected(crate::import::pdf_file::RejectReason::Corrupt),
        )
        .unwrap();

        assert_eq!(status, JobStatus::Failed);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        assert_eq!(backoff_seconds(1), 2);
        assert_eq!(backoff_seconds(2), 4);
        assert_eq!(backoff_seconds(3), 8);
        assert!(backoff_seconds(50) <= 300);
    }

    #[test]
    fn retrying_a_failed_job_makes_it_runnable_again() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        let job_id = enqueue(&conn, Some("p1"), JobType::Analyze, "v1").unwrap().unwrap();
        let job = claim_next(&conn).unwrap().unwrap();
        fail(&conn, &job, &AppError::Internal("boom".into())).unwrap();

        retry(&conn, &job_id).unwrap();

        assert!(claim_next(&conn).unwrap().is_some());
    }

    #[test]
    fn a_cancelled_job_is_not_claimed() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_paper(&conn, "p1");
        let job_id = enqueue(&conn, Some("p1"), JobType::Embed, "v1").unwrap().unwrap();

        cancel(&conn, &job_id).unwrap();

        assert!(claim_next(&conn).unwrap().is_none());
    }
}
