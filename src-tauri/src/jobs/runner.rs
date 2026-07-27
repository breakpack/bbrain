use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, Notify};

use super::queue::{self, Job, JobStatus};
use super::{FrontendJobRequest, JobProgress, JOB_FRONTEND_REQUEST, JOB_PROGRESS};
use crate::error::{AppError, Result};
use crate::state::AppState;

/// A frontend-assisted job waits this long for the webview to answer. Long
/// enough for a 500-page PDF, short enough that a closed window does not pin the
/// job forever — it goes back to `queued` and retries on the next run.
const FRONTEND_TIMEOUT: Duration = Duration::from_secs(180);

/// Serial by design: extraction and embedding are CPU-heavy, and running one at
/// a time keeps the UI thread and the user's laptop responsive (§4.1).
pub struct JobRunner {
    wake: Arc<Notify>,
    /// Frontend-assisted jobs that have been dispatched and are awaiting a
    /// `submit_*` callback from the webview.
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<()>>>>>,
}

impl JobRunner {
    pub fn new() -> Self {
        Self {
            wake: Arc::new(Notify::new()),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Tells the runner there may be new work. Safe to call from any thread.
    pub fn notify(&self) {
        self.wake.notify_one();
    }

    /// Resolves the frontend-assisted job the webview just answered. Returns
    /// false if the job is unknown — typically because it already timed out.
    pub fn resolve(&self, job_id: &str, outcome: Result<()>) -> bool {
        let sender = self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(job_id);
        match sender {
            Some(sender) => sender.send(outcome).is_ok(),
            None => false,
        }
    }

    fn register(&self, job_id: &str) -> oneshot::Receiver<Result<()>> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(job_id.to_string(), sender);
        receiver
    }

    fn forget(&self, job_id: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(job_id);
    }
}

impl Default for JobRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Drains the queue, then sleeps until notified or until the next backoff window
/// could have elapsed.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let wake = {
                let state = app.state::<AppState>();
                state.jobs.wake.clone()
            };

            loop {
                let claimed = {
                    let state = app.state::<AppState>();
                    let conn = state.db.conn();
                    queue::claim_next(&conn)
                };

                match claimed {
                    Ok(Some(job)) => run_one(&app, job).await,
                    Ok(None) => break,
                    Err(error) => {
                        tracing::error!(code = ?error.code(), "could not claim a job");
                        break;
                    }
                }
            }

            // A backoff-gated job becomes runnable on a timer, not on a notify.
            tokio::select! {
                _ = wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
}

async fn run_one(app: &AppHandle, job: Job) {
    tracing::info!(
        job = %job.id,
        job_type = job.job_type.as_str(),
        attempt = job.attempts + 1,
        "running job"
    );
    emit_progress(app, &job, JobStatus::Running, None);

    let outcome = if job.job_type.is_frontend_assisted() {
        dispatch_to_frontend(app, &job).await
    } else {
        super::handlers::run(app, &job).await
    };

    let state = app.state::<AppState>();
    let conn = state.db.conn();

    let status = match outcome {
        Ok(()) => {
            if let Err(error) = queue::complete(&conn, &job.id) {
                tracing::error!(code = ?error.code(), "could not mark job completed");
            }
            JobStatus::Completed
        }
        Err(error) => {
            // Debug format so the wrapped cause (e.g. the rusqlite error inside
            // Storage) reaches the log — the bare code alone is undiagnosable.
            // AppError variants carry labels and system errors, never keys or
            // provider bodies (§16.1).
            tracing::warn!(
                job = %job.id,
                job_type = job.job_type.as_str(),
                code = ?error.code(),
                "job failed: {error:?}"
            );
            queue::fail(&conn, &job, &error).unwrap_or(JobStatus::Failed)
        }
    };

    let error_code = (status == JobStatus::Failed || status == JobStatus::WaitingForKey)
        .then(|| format!("{:?}", status));
    drop(conn);

    emit_progress(app, &job, status, error_code);
}

/// PDF.js lives in the webview, so text extraction and thumbnails are performed
/// there. The job row stays the durable record: if the app closes mid-extraction
/// the job is `running`, and the next launch requeues it (§10.1).
async fn dispatch_to_frontend(app: &AppHandle, job: &Job) -> Result<()> {
    let Some(paper_id) = job.paper_id.clone() else {
        return Err(AppError::Internal("frontend job without a paper".into()));
    };

    let mut receiver = {
        let state = app.state::<AppState>();
        state.jobs.register(&job.id)
    };

    let request = FrontendJobRequest {
        job_id: job.id.clone(),
        paper_id,
        job_type: job.job_type,
    };

    // Re-emit until the webview answers. At startup the runner can dispatch a
    // (re-)extraction before the webview's listener has mounted; a single emit
    // would then be lost and the job would stall until the long timeout. Re-
    // emitting every couple of seconds means the request lands within one tick
    // of the listener coming up, while a genuinely closed window still stops at
    // the overall deadline.
    let deadline = tokio::time::Instant::now() + FRONTEND_TIMEOUT;
    loop {
        app.emit(JOB_FRONTEND_REQUEST, request.clone())
            .map_err(|e| AppError::Internal(format!("emit frontend request: {e}")))?;

        tokio::select! {
            answered = &mut receiver => {
                return match answered {
                    Ok(result) => result,
                    Err(_) => Err(AppError::Internal("frontend job was dropped".into())),
                };
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                if tokio::time::Instant::now() >= deadline {
                    let state = app.state::<AppState>();
                    state.jobs.forget(&job.id);
                    // A silent webview (window closed) is transient: back off and
                    // retry rather than failing the paper.
                    return Err(AppError::ProviderUnavailable);
                }
            }
        }
    }
}

fn emit_progress(app: &AppHandle, job: &Job, status: JobStatus, error_code: Option<String>) {
    let pending = {
        let state = app.state::<AppState>();
        let conn = state.db.conn();
        queue::pending_count(&conn).unwrap_or(0)
    };

    let _ = app.emit(
        JOB_PROGRESS,
        JobProgress {
            job_id: job.id.clone(),
            paper_id: job.paper_id.clone(),
            job_type: job.job_type,
            status,
            error_code,
            pending,
        },
    );
}
