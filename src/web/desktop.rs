//! What only the desktop app's own window may do: keep, copy and delete
//! identity keys, make backups that hold the identity, and set up pipe
//! interfaces, which run programs. The web UI leaves all of these out,
//! since anyone with its login link controls it.
//!
//! None of this is reachable over HTTP. The desktop app (a separate
//! program that runs rettui in-process) calls these functions from Tauri
//! commands, which only its local management window may invoke; the web
//! UI's page is a remote origin to Tauri and has no permission for them.
//! Outside the desktop app nothing registers the channel they use, so they
//! all fail.

use std::sync::Mutex;

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::{Job, Owner};
use crate::app::files;
use crate::reticulum::{self as rns, Section};

/// Where jobs for the owner loop go, set while the desktop app's web UI runs.
static JOBS: Mutex<Option<mpsc::UnboundedSender<Job>>> = Mutex::new(None);

/// Longest name, passphrase, path or command taken (a bound on what an
/// untrusted caller could make rettui hold).
const LONGEST: usize = 4096;

/// Called by the web UI when it starts, and again when it starts over as
/// another identity. Only the desktop app is given the channel.
pub(super) fn register(jobs: &mpsc::UnboundedSender<Job>) {
    if crate::update::install::DESKTOP_APP.load(std::sync::atomic::Ordering::Relaxed) {
        *JOBS.lock().unwrap_or_else(|e| e.into_inner()) = Some(jobs.clone());
    }
}

#[cfg(test)]
pub(super) fn register_for_test(jobs: &mpsc::UnboundedSender<Job>) {
    *JOBS.lock().unwrap_or_else(|e| e.into_inner()) = Some(jobs.clone());
}

async fn run<T: Send + 'static>(f: impl FnOnce(&mut Owner) -> T + Send + 'static) -> Result<T, String> {
    let jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("rettui isn't running in the desktop app")?;
    let (tx, rx) = oneshot::channel();
    let run = Box::new(move |owner: &mut Owner| {
        let _ = tx.send(f(owner));
    });
    jobs.send(Job { run, changes: true }).map_err(|_| "rettui is shutting down".to_string())?;
    rx.await.map_err(|_| "That failed inside rettui (or rettui is shutting down)".to_string())
}

/// Text from outside, checked: not too long, nothing that would be a second
/// line in a config file or a file name.
fn plain(what: &str, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.len() > LONGEST {
        return Err(format!("{what} is too long"));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("{what} can't have line breaks or other control characters"));
    }
    Ok(text.to_string())
}

fn identity_id(id: &str) -> String {
    if id == "-" { String::new() } else { id.to_string() }
}

// ---- Identities --------------------------------------------------------------

/// The identities kept here.
pub async fn identities() -> Result<Value, String> {
    let list = run(|o| o.app.identities()).await?;
    let listed: Vec<Value> = list
        .iter()
        .map(|p| {
            json!({
                "id": if p.id.is_empty() { "-" } else { p.id.as_str() },
                "name": p.name,
                "address": hex::encode(p.lxmf),
                "active": p.active,
                "first": p.id.is_empty(),
            })
        })
        .collect();
    Ok(json!({ "identities": listed, "most": crate::identities::MOST }))
}

/// Keep another identity: a new one, or the one in the file `from` (as
/// Sideband, NomadNet, MeshChat or rettui keep them).
pub async fn keep_identity(name: &str, from: Option<&str>) -> Result<Value, String> {
    let name = plain("The name", name)?;
    let from = from.map(|f| plain("The file", f)).transpose()?.filter(|f| !f.is_empty()).map(|f| files::path_from_input(&f));
    let profile = run(move |o| o.app.add_identity(&name, from.as_deref())).await??;
    Ok(json!({ "id": profile.id, "name": profile.name, "address": hex::encode(profile.lxmf) }))
}

/// Save a copy of the identity in use to `to`, for its owner only and never
/// over another file.
pub async fn export_identity(to: &str) -> Result<String, String> {
    let to = files::path_from_input(&plain("The file", to)?);
    let saved = run(move |o| o.app.back_up_identity(&to)).await??;
    Ok(saved.display().to_string())
}

/// Switch: rettui saves everything and starts over as the other identity.
pub async fn switch_identity(id: &str) -> Result<Value, String> {
    let id = identity_id(id);
    let profile = run(move |o| o.app.switch_identity(&id)).await??;
    Ok(json!({ "name": profile.name, "address": hex::encode(profile.lxmf) }))
}

pub async fn rename_identity(id: &str, name: &str) -> Result<(), String> {
    let (id, name) = (identity_id(id), plain("The name", name)?);
    run(move |o| o.app.rename_identity(&id, &name)).await?
}

/// Delete an identity kept here (not the first, nor the one in use).
pub async fn delete_identity(id: &str) -> Result<String, String> {
    let id = identity_id(id);
    let profile = run(move |o| o.app.delete_identity(&id)).await??;
    Ok(profile.name)
}

// ---- Backups -----------------------------------------------------------------

/// Back up settings, contacts, messages and the identity to the new file
/// `to`, sealed with `passphrase` if there is one. How many files it holds.
pub async fn back_up(to: &str, passphrase: Option<String>) -> Result<usize, String> {
    let to = files::path_from_input(&plain("The file", to)?);
    if to.as_os_str().is_empty() {
        return Err("Say where to save the backup".into());
    }
    if passphrase.as_ref().is_some_and(|p| p.len() > LONGEST) {
        return Err("The passphrase is too long".into());
    }
    let passphrase = passphrase.filter(|p| !p.is_empty()).map(Zeroizing::new);
    let (done, wait) = oneshot::channel();
    run(move |o| o.app.back_up_all_then(to, passphrase, Some(done))).await??;
    wait.await.map_err(|_| "The backup stopped before it was written".to_string())?
}

/// A name for a backup, with today's date, as the terminal suggests.
pub fn backup_name() -> String {
    crate::backup::default_name()
}

/// Why a passphrase is easy to guess, if it is (it may be used anyway).
pub fn passphrase_weakness(passphrase: &str) -> Option<String> {
    crate::backup::sealed::weakness(passphrase).map(|why| why.to_string())
}

// ---- Pipe interfaces ---------------------------------------------------------

fn pipe_list(o: &mut Owner) -> Result<Value, String> {
    let (text, _) = rns::load(&o.app.rns_path())?;
    let (config, _) = rns::check(&text);
    let pipes: Vec<Value> = rns::sections(&text)
        .into_iter()
        .filter_map(|section| {
            let Section::Interface(name) = &section else { return None };
            let values = config.as_ref()?.subsection("interfaces", name)?;
            (values.get("type") == Some("PipeInterface")).then(
                || json!({ "name": name, "command": values.get("command").unwrap_or_default(), "enabled": values.interface_enabled() }),
            )
        })
        .collect();
    Ok(json!({ "pipes": pipes, "note": rns::RESTART_NOTE }))
}

/// The pipe interfaces in Reticulum's config: each runs a program.
pub async fn pipes() -> Result<Value, String> {
    run(pipe_list).await?
}

/// Add a pipe interface that runs `command`, turned off until asked.
pub async fn add_pipe(name: &str, command: &str) -> Result<Vec<String>, String> {
    let (name, command) = (plain("The name", name)?, plain("The command", command)?);
    if name.is_empty() || command.is_empty() {
        return Err("A pipe interface needs a name and a command".into());
    }
    run(move |o| {
        o.app.rns_add_interface(&name, "PipeInterface", false)?;
        let section = Section::Interface(name.clone());
        o.app.rns_set_options(&section, &[("command", command.as_str()), ("enabled", "no")], false)
    })
    .await?
}

/// Change the command and whether the pipe interface `name` is on.
pub async fn set_pipe(name: &str, command: &str, enabled: bool) -> Result<Vec<String>, String> {
    let (name, command) = (plain("The name", name)?, plain("The command", command)?);
    if command.is_empty() {
        return Err("A pipe interface needs a command".into());
    }
    run(move |o| {
        let section = Section::Interface(name);
        o.app.rns_set_options(&section, &[("command", command.as_str()), ("enabled", if enabled { "yes" } else { "no" })], false)
    })
    .await?
}

pub async fn remove_pipe(name: &str) -> Result<Vec<String>, String> {
    let name = plain("The name", name)?;
    run(move |o| {
        let is_pipe = pipe_list(o)?["pipes"].as_array().is_some_and(|p| p.iter().any(|p| p["name"] == name.as_str()));
        if !is_pipe {
            return Err(format!("{name} isn't a pipe interface"));
        }
        o.app.rns_remove_interface(&name, false)
    })
    .await?
}

/// Start Reticulum again, for changes to its config to apply.
pub async fn restart_reticulum() -> Result<(), String> {
    run(|o| o.app.request_rns_restart()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::testing::TestWeb;

    // The channel is global, so tests that use it can't overlap.
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn nothing_works_outside_the_desktop_app() {
        let _one = ONE_AT_A_TIME.lock().await;
        *JOBS.lock().unwrap() = None;
        assert!(identities().await.unwrap_err().contains("desktop app"));
        assert!(back_up("x", None).await.is_err());
        assert!(add_pipe("p", "cat").await.is_err());
    }

    #[test]
    fn text_from_outside_is_checked() {
        assert_eq!(plain("x", "  a b ").unwrap(), "a b");
        assert!(plain("x", "a\nb").is_err());
        assert!(plain("x", &"a".repeat(LONGEST + 1)).is_err());
        assert_eq!(identity_id("-"), "");
        assert_eq!(identity_id("ab"), "ab");
    }

    #[tokio::test]
    async fn identities_are_kept_copied_and_deleted() {
        let _one = ONE_AT_A_TIME.lock().await;
        let web = TestWeb::start("desktop-identities");
        register_for_test(&web.jobs());
        let file = web.with(|app| app.paths.identity.clone()).await;
        rns_identity::identity::Identity::new().to_file(&file).unwrap();
        let listed = identities().await.unwrap();
        assert_eq!(listed["identities"].as_array().unwrap().len(), 1);
        let kept = keep_identity("Second", None).await.unwrap();
        let id = kept["id"].as_str().unwrap().to_string();
        assert_eq!(identities().await.unwrap()["identities"].as_array().unwrap().len(), 2);
        rename_identity(&id, "Other").await.unwrap();
        // Never over another file, and nothing with line breaks.
        let copy = web.dir.join("copy");
        export_identity(&copy.display().to_string()).await.unwrap();
        assert!(copy.is_file());
        assert!(export_identity(&copy.display().to_string()).await.is_err());
        assert!(keep_identity("two\nlines", None).await.is_err());
        // One from a file comes in as a third; the same twice doesn't.
        let other = web.dir.join("other");
        rns_identity::identity::Identity::new().to_file(&other).unwrap();
        let third = keep_identity("Third", Some(&other.display().to_string())).await.unwrap();
        assert!(keep_identity("Again", Some(&other.display().to_string())).await.is_err());
        assert!(keep_identity("Same", Some(&copy.display().to_string())).await.is_err());
        assert_eq!(identities().await.unwrap()["identities"].as_array().unwrap().len(), 3);
        assert_eq!(delete_identity(third["id"].as_str().unwrap()).await.unwrap(), "Third");
        assert_eq!(delete_identity(&id).await.unwrap(), "Other");
        assert!(delete_identity("-").await.is_err(), "the first stays");
    }

    #[tokio::test]
    async fn backups_hold_the_identity() {
        let _one = ONE_AT_A_TIME.lock().await;
        let web = TestWeb::start("desktop-backup");
        register_for_test(&web.jobs());
        let to = web.dir.join("all.tar.gz");
        let files = back_up(&to.display().to_string(), None).await.unwrap();
        assert!(files > 0 && to.is_file());
        assert!(back_up(&to.display().to_string(), None).await.unwrap_err().contains("exists already"));
        let sealed = web.dir.join("sealed");
        back_up(&sealed.display().to_string(), Some("a long and unlikely passphrase 41".into())).await.unwrap();
        assert!(crate::backup::is_sealed(&sealed));
        assert!(!backup_name().is_empty());
        assert!(passphrase_weakness("abc").is_some());
    }

    #[tokio::test]
    async fn pipe_interfaces_are_added_changed_and_removed() {
        let _one = ONE_AT_A_TIME.lock().await;
        let web = TestWeb::start("desktop-pipes");
        register_for_test(&web.jobs());
        assert!(pipes().await.unwrap()["pipes"].as_array().unwrap().is_empty());
        add_pipe("Radio", "/bin/cat").await.unwrap();
        let listed = pipes().await.unwrap();
        assert_eq!(listed["pipes"][0]["command"], "/bin/cat");
        assert_eq!(listed["pipes"][0]["enabled"], false, "off until asked");
        set_pipe("Radio", "/bin/true", true).await.unwrap();
        let listed = pipes().await.unwrap();
        assert_eq!((&listed["pipes"][0]["command"], &listed["pipes"][0]["enabled"]), (&json!("/bin/true"), &json!(true)));
        assert!(add_pipe("Bad\nname", "x").await.is_err());
        assert!(add_pipe("", "x").await.is_err());
        assert!(set_pipe("Radio", "", true).await.is_err());
        // Only pipes are removed here.
        assert!(remove_pipe("Default Interface").await.is_err());
        remove_pipe("Radio").await.unwrap();
        assert!(pipes().await.unwrap()["pipes"].as_array().unwrap().is_empty());
        restart_reticulum().await.unwrap();
    }
}
