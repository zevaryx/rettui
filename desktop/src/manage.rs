//! The management window's commands: identity keys, backups that hold the
//! identity, and pipe interfaces (which run programs). The web UI leaves
//! these out on purpose, since anyone with its login link controls it, and
//! so the web UI's page can't call these either: they have permissions
//! (build.rs) that only the local `manage` window is granted
//! (capabilities/manage.json), and each also checks which window it's
//! asked from. What they do is rettui's `manage` module, which only works
//! inside this app.

use serde_json::{Value, json};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// The window's label: also the one name its capability is given to.
pub const LABEL: &str = "manage";

/// Opens the window, or brings it forward.
pub fn open(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("rettui: identities, backups and pipes")
        .inner_size(760.0, 640.0)
        .min_inner_size(420.0, 420.0)
        .build();
    if let Err(e) = built {
        eprintln!("rettui: couldn't open the management window: {e}");
    }
}

/// Whether `window` is the management window, showing the app's own page
/// (not something it navigated to).
fn allowed(window: &WebviewWindow) -> Result<(), String> {
    let local = |url: Url| matches!(url.scheme(), "tauri") || matches!(url.host_str(), Some("tauri.localhost"));
    if window.label() == LABEL && window.url().is_ok_and(local) {
        Ok(())
    } else {
        Err("This can only be done from the app's management window".into())
    }
}

#[tauri::command]
pub async fn manage_defaults(window: WebviewWindow) -> Result<Value, String> {
    allowed(&window)?;
    let downloads = window.app_handle().state::<Downloads>().0.clone();
    let join = |name: String| downloads.join(name).display().to_string();
    Ok(json!({
        "backup": join(rettui::manage::backup_name()),
        "identity": join("rettui-identity".into()),
    }))
}

/// Where files are saved to by default.
pub struct Downloads(pub std::path::PathBuf);

#[tauri::command]
pub async fn manage_identities(window: WebviewWindow) -> Result<Value, String> {
    allowed(&window)?;
    rettui::manage::identities().await
}

#[tauri::command]
pub async fn manage_keep_identity(window: WebviewWindow, name: String, from: Option<String>) -> Result<Value, String> {
    allowed(&window)?;
    rettui::manage::keep_identity(&name, from.as_deref()).await
}

#[tauri::command]
pub async fn manage_export_identity(window: WebviewWindow, to: String) -> Result<String, String> {
    allowed(&window)?;
    rettui::manage::export_identity(&to).await
}

#[tauri::command]
pub async fn manage_switch_identity(window: WebviewWindow, id: String) -> Result<Value, String> {
    allowed(&window)?;
    rettui::manage::switch_identity(&id).await
}

#[tauri::command]
pub async fn manage_rename_identity(window: WebviewWindow, id: String, name: String) -> Result<(), String> {
    allowed(&window)?;
    rettui::manage::rename_identity(&id, &name).await
}

#[tauri::command]
pub async fn manage_delete_identity(window: WebviewWindow, id: String) -> Result<String, String> {
    allowed(&window)?;
    rettui::manage::delete_identity(&id).await
}

#[tauri::command]
pub async fn manage_back_up(window: WebviewWindow, to: String, passphrase: Option<String>) -> Result<usize, String> {
    allowed(&window)?;
    rettui::manage::back_up(&to, passphrase).await
}

#[tauri::command]
pub async fn manage_passphrase_weakness(window: WebviewWindow, passphrase: String) -> Result<Option<String>, String> {
    allowed(&window)?;
    Ok(rettui::manage::passphrase_weakness(&passphrase))
}

#[tauri::command]
pub async fn manage_pipes(window: WebviewWindow) -> Result<Value, String> {
    allowed(&window)?;
    rettui::manage::pipes().await
}

#[tauri::command]
pub async fn manage_add_pipe(window: WebviewWindow, name: String, command: String) -> Result<Vec<String>, String> {
    allowed(&window)?;
    rettui::manage::add_pipe(&name, &command).await
}

#[tauri::command]
pub async fn manage_set_pipe(window: WebviewWindow, name: String, command: String, enabled: bool) -> Result<Vec<String>, String> {
    allowed(&window)?;
    rettui::manage::set_pipe(&name, &command, enabled).await
}

#[tauri::command]
pub async fn manage_remove_pipe(window: WebviewWindow, name: String) -> Result<Vec<String>, String> {
    allowed(&window)?;
    rettui::manage::remove_pipe(&name).await
}

#[tauri::command]
pub async fn manage_restart_reticulum(window: WebviewWindow) -> Result<(), String> {
    allowed(&window)?;
    rettui::manage::restart_reticulum().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_has_a_name_its_capability_matches() {
        let capability = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities/manage.json")).unwrap();
        let capability: Value = serde_json::from_str(&capability).unwrap();
        assert_eq!(capability["windows"], json!([LABEL]));
        assert!(capability.get("remote").is_none(), "a local page only");
        let granted: Vec<&str> = capability["permissions"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        // Every command is in build.rs's list and granted to this window only.
        let build = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs")).unwrap();
        for name in build.lines().filter_map(|l| l.trim().strip_prefix('"')?.strip_suffix("\",")) {
            assert!(granted.contains(&format!("allow-{}", name.replace('_', "-")).as_str()), "{name}");
        }
        let web = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities/web-ui.json")).unwrap();
        assert!(!web.contains("manage"), "the web UI's page can't call these");
    }
}
