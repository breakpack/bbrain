use tauri::State;

use crate::db::settings_repo::{self, Settings, SettingsPatch};
use crate::error::CommandResult;
use crate::state::AppState;

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CommandResult<Settings> {
    Ok(settings_repo::get(&state.db.conn())?)
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> CommandResult<Settings> {
    Ok(settings_repo::update(&state.db.conn(), &patch)?)
}
