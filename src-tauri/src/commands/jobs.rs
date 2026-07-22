use serde::Serialize;
use tauri::State;

use crate::error::CommandResult;
use crate::jobs::queue::{self, JobStatus, JobType};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSummary {
    pub id: String,
    pub paper_id: Option<String>,
    pub paper_title: Option<String>,
    pub job_type: JobType,
    pub status: JobStatus,
    pub attempts: i64,
    pub error_code: Option<String>,
    pub updated_at: String,
}

/// Jobs the user may need to act on: failures and anything stalled on a key.
#[tauri::command]
pub fn list_blocked_jobs(state: State<'_, AppState>) -> CommandResult<Vec<JobSummary>> {
    let conn = state.db.conn();
    let mut statement = conn.prepare(
        "SELECT j.id, j.paper_id, p.title, j.type, j.status, j.attempts, j.error_code, j.updated_at
         FROM jobs j LEFT JOIN papers p ON p.id = j.paper_id
         WHERE j.status IN ('failed', 'waiting_for_key')
         ORDER BY j.updated_at DESC",
    )?;

    let rows = statement.query_map([], |row| {
        let job_type: String = row.get(3)?;
        let status: String = row.get(4)?;
        Ok(JobSummary {
            id: row.get(0)?,
            paper_id: row.get(1)?,
            paper_title: row.get(2)?,
            job_type: JobType::from_str(&job_type).unwrap_or(JobType::Extract),
            status: match status.as_str() {
                "waiting_for_key" => JobStatus::WaitingForKey,
                _ => JobStatus::Failed,
            },
            attempts: row.get(5)?,
            error_code: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[tauri::command]
pub fn retry_job(state: State<'_, AppState>, job_id: String) -> CommandResult<()> {
    queue::retry(&state.db.conn(), &job_id)?;
    state.jobs.notify();
    Ok(())
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, job_id: String) -> CommandResult<()> {
    queue::cancel(&state.db.conn(), &job_id)?;
    Ok(())
}

#[tauri::command]
pub fn pending_job_count(state: State<'_, AppState>) -> CommandResult<i64> {
    Ok(queue::pending_count(&state.db.conn())?)
}
