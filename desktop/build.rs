/// The commands of the management window (src/manage.rs), which may change
/// identity keys, back up with the identity, restore a backup and set up pipe interfaces. Each
/// needs a permission (`allow-<name>`), and only the local `manage` window's
/// capability (capabilities/manage.json) grants them: the web UI's page,
/// which anyone with its login link controls, is given none.
const MANAGE: &[&str] = &[
    "manage_defaults",
    "manage_identities",
    "manage_keep_identity",
    "manage_export_identity",
    "manage_switch_identity",
    "manage_rename_identity",
    "manage_delete_identity",
    "manage_back_up",
    "manage_passphrase_weakness",
    "manage_restore_check",
    "manage_restore",
    "manage_pipes",
    "manage_add_pipe",
    "manage_set_pipe",
    "manage_remove_pipe",
    "manage_restart_reticulum",
];

fn main() {
    let app = tauri_build::AppManifest::new().commands(MANAGE);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(app)).expect("failed to run the Tauri build script");
}
