use tauri::AppHandle;

use super::queue::{Job, JobType};
use crate::error::{AppError, Result};

/// Runs a backend job. Frontend-assisted types never reach here — the runner
/// dispatches those to the webview instead.
pub async fn run(app: &AppHandle, job: &Job) -> Result<()> {
    let Some(paper_id) = job.paper_id.as_deref() else {
        return Err(AppError::Internal("job without a paper".into()));
    };

    match job.job_type {
        JobType::Embed => crate::rag::embed_paper(app, paper_id).await,
        JobType::Analyze => crate::analysis::analyze_paper(app, paper_id).await,
        JobType::Relations => crate::relations::recompute_for_paper(app, paper_id).await,
        JobType::ObsidianSync => crate::obsidian::sync_paper(app, paper_id).await,

        JobType::Extract | JobType::Thumbnail => Err(AppError::Internal(
            "frontend job reached the backend handler".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_job_types_are_routed_away_from_the_backend_handler() {
        assert!(JobType::Extract.is_frontend_assisted());
        assert!(JobType::Thumbnail.is_frontend_assisted());
        assert!(!JobType::Embed.is_frontend_assisted());
        assert!(!JobType::Analyze.is_frontend_assisted());
    }
}
