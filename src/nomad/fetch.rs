//! Page, file and media requests over (reused) NomadNet Links.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rns_runtime::prelude::*;
use tokio::sync::Mutex;

use super::{paths, request};
use crate::net::{Hash, Progress, link_options};

#[derive(Debug, Clone)]
pub struct FetchedContent {
    /// What came, unless it was kept on disk (`file`): then empty.
    pub data: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
    /// A file downloaded (a `/file/` request with a spool folder), kept on
    /// disk rather than in memory: it may be up to the 128 MB a Resource
    /// holds.
    pub file: Option<DownloadedFile>,
}

impl FetchedContent {
    /// Content held in memory.
    pub fn bytes(data: Vec<u8>, metadata: Option<Vec<u8>>) -> Self {
        Self { data, metadata, file: None }
    }
}

/// A downloaded file waiting on disk to be saved or sent on. Copies share
/// it; the file is removed when the last of them goes.
#[derive(Debug, Clone)]
pub struct DownloadedFile(Arc<Spooled>);

#[derive(Debug)]
struct Spooled {
    path: PathBuf,
    len: u64,
}

impl Drop for Spooled {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl DownloadedFile {
    /// Take charge of the file at `path`, of `len` bytes.
    pub fn new(path: PathBuf, len: u64) -> Self {
        Self(Arc::new(Spooled { path, len }))
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }

    /// Its size in bytes.
    pub fn size(&self) -> u64 {
        self.0.len
    }

    /// Save a copy as `target`, which must not exist. The copy is a new
    /// file made here, so it has the permissions any new file of yours gets
    /// (the spooled one is readable by you alone), and it is written beside
    /// `target` first and only then given its name: never over a file, or
    /// through a link, that took the name meanwhile.
    pub fn save_as(self, target: &Path) -> std::io::Result<()> {
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = target.with_file_name(format!(".{name}.{}.tmp", hex::encode(rns_crypto::random::random_bytes(8))));
        let saved = (|| {
            let mut copy = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            std::io::copy(&mut std::fs::File::open(self.path())?, &mut copy)?;
            copy.sync_all()?;
            drop(copy);
            name_without_replacing(&tmp, target)
        })();
        // After a hard link it has two names; after a rename, none to remove.
        let _ = std::fs::remove_file(&tmp);
        saved
    }
}

/// Give the file at `tmp` the name `target` too, failing if something has
/// that name: a hard link, which never replaces. Where there are none (a
/// FAT drive), a rename once nothing has the name.
fn name_without_replacing(tmp: &Path, target: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(tmp, target) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
            if std::fs::symlink_metadata(target).is_ok() {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} exists", target.display())));
            }
            crate::config::replace(tmp, target)
        }
        linked => linked,
    }
}

/// The most a page or an image may be, to be kept or shown. (A file being
/// downloaded isn't held to it.) A node can send more than it was asked for.
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// Open NomadNet Links, reused across page loads, with whether we identified.
pub type LinkCache = Arc<Mutex<HashMap<Hash, (LinkSessionHandle, bool)>>>;

/// Fetch a page, file or media item. With `identity`, the Link identifies
/// us to the node (some pages personalise content or require it).
/// `progress` hears how finding a path to it goes, when that takes a while.
/// With `spool`, a file (`/file/`) is written into that folder as it comes
/// and handed over there ([`FetchedContent::file`]), not kept in memory.
#[allow(clippy::too_many_arguments)]
pub async fn fetch(
    runtime: &ReticulumHandle,
    links: &LinkCache,
    node: Hash,
    path: &str,
    fields: &BTreeMap<String, String>,
    identity: Option<Identity>,
    progress: Progress<'_>,
    spool: Option<&Path>,
) -> Result<FetchedContent, String> {
    let spool = spool.filter(|_| path.starts_with(paths::FILE_PREFIX));
    let request = if let Some(media) = path.strip_prefix("/media/") {
        Ok(request::media(media))
    } else if path.starts_with(paths::FILE_PREFIX) {
        request::file(path)
    } else {
        request::page(path, fields)
    }
    .map_err(|e| e.to_string())?;

    for attempt in 0..2 {
        let handle = node_link(runtime, links, node, identity.as_ref(), progress).await?;
        let options = request_options(path, spool);
        match handle.request_with_options(&request.path, rmpv::Value::Binary(request.data.clone()), options).await {
            Ok(response) => {
                if let Some(spool) = spool {
                    return crate::net::response_file(response, spool).await;
                }
                let (data, metadata) = crate::net::response_bytes(response).await?;
                if data.len() > MAX_PAGE_BYTES && !path.starts_with(paths::FILE_PREFIX) {
                    return Err(format!("The node sent {} MB for a page or image, more than rettui takes", data.len() / 1_000_000));
                }
                return Ok(FetchedContent::bytes(data, metadata));
            }
            Err(LinkSessionError::ResponseRejected { size, max }) => {
                return Err(format!(
                    "The node offered {} MB, more than the {} MB rettui takes for a file",
                    size / 1_000_000,
                    max / 1_000_000
                ));
            }
            Err(e @ (LinkSessionError::LinkNotActive | LinkSessionError::SessionClosed)) if attempt == 0 => {
                tracing::debug!("NomadNet link to {} went away ({e}); reconnecting", hex::encode(node));
                links.lock().await.remove(&node);
            }
            Err(e) => {
                links.lock().await.remove(&node);
                return Err(format!("Request failed: {e}"));
            }
        }
    }
    Err("Request failed".to_string())
}

/// How a request for `path` is sent: as `request` sends it, with a folder
/// for a file to be written into (if one is given), and for a file (with a
/// folder or not, as `rettui fetch` asks) its size limit told to the
/// library, which then refuses a bigger one before taking it.
fn request_options(path: &str, spool: Option<&Path>) -> RequestOptions {
    RequestOptions {
        timeout: Some(request::TIMEOUT),
        spool_dir: spool.map(Path::to_path_buf),
        max_response_size: path.starts_with(paths::FILE_PREFIX).then_some(crate::net::MAX_FILE_RESPONSE as usize),
        ..RequestOptions::default()
    }
}

async fn node_link(
    runtime: &ReticulumHandle,
    links: &LinkCache,
    node: Hash,
    identity: Option<&Identity>,
    progress: Progress<'_>,
) -> Result<LinkSessionHandle, String> {
    let identify = identity.is_some();
    let stale = {
        let mut cache = links.lock().await;
        match cache.get(&node) {
            Some((handle, identified)) if *identified == identify => return Ok(handle.clone()),
            // Identification changed: start a fresh Link.
            Some(_) => cache.remove(&node).map(|(handle, _)| handle),
            None => None,
        }
    };
    if let Some(handle) = stale {
        handle.close().await;
    }
    let local = identity.cloned().unwrap_or_else(Identity::new);
    let session = crate::net::connect(runtime, node, local, link_options("rettui.nomad", identify), progress)
        .await
        .map_err(|e| format!("Could not connect to node: {e}"))?;
    let LinkSession { handle, mut events, mut resource_offers } = session;

    // Drain session events so the session never stalls, and forget the Link
    // when it closes. Unsolicited Resources are refused.
    let (links_for_task, handle_for_task) = (links.clone(), handle.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(LinkSessionEvent::Closed { .. }) | None => break,
                    Some(_) => {}
                },
                Some(offer) = resource_offers.recv() => {
                    let _ = offer.reject().await;
                }
            }
        }
        let mut links = links_for_task.lock().await;
        if links.get(&node).is_some_and(|(h, _)| h.link_id() == handle_for_task.link_id()) {
            links.remove(&node);
        }
    });

    links.lock().await.insert(node, (handle.clone(), identify));
    Ok(handle)
}

/// Fetch one NomadNet item without the TUI (used by `rettui fetch`).
pub async fn fetch_once(
    runtime: &ReticulumHandle,
    node: Hash,
    path: &str,
    identity: Option<Identity>,
    progress: Progress<'_>,
) -> Result<FetchedContent, String> {
    let links: LinkCache = Arc::new(Mutex::new(HashMap::new()));
    let result = fetch(runtime, &links, node, path, &BTreeMap::new(), identity, progress, None).await;
    for (handle, _) in links.lock().await.values() {
        handle.close().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spooled(dir: &Path, name: &str, data: &[u8]) -> DownloadedFile {
        let path = dir.join(name);
        std::fs::write(&path, data).unwrap();
        DownloadedFile::new(path, data.len() as u64)
    }

    #[test]
    fn a_downloaded_file_is_removed_with_its_last_holder_and_saved_as_a_new_file() {
        let dir = std::env::temp_dir().join(format!("rettui-downloaded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("saved")).unwrap();
        let left_in = |dir: &Path| std::fs::read_dir(dir).unwrap().count();
        // Dropped by all: gone.
        let file = spooled(&dir, "a", b"aaa");
        let (copy, path) = (file.clone(), file.path().to_path_buf());
        drop(file);
        assert!(path.exists(), "a copy still holds it");
        drop(copy);
        assert!(!path.exists());
        // Saved by its only holder: the spooled file goes, the saved one stays,
        // nothing half-written left beside it.
        let file = spooled(&dir, "b", b"bbb");
        let path = file.path().to_path_buf();
        let target = dir.join("saved/b.txt");
        file.save_as(&target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"bbb");
        assert!(!path.exists());
        assert_eq!(left_in(&dir.join("saved")), 1);
        // Saved while something else holds it (a browser still being sent
        // it): the original stays for it.
        let file = spooled(&dir, "c", b"ccc");
        let other = file.clone();
        file.save_as(&dir.join("saved/c.txt")).unwrap();
        assert!(other.path().exists());
        drop(other);
        assert!(!dir.join("c").exists());
        // A name taken after it was chosen: refused, what has it untouched.
        let taken = dir.join("saved/taken.txt");
        std::fs::write(&taken, b"mine").unwrap();
        assert_eq!(spooled(&dir, "d", b"ddd").save_as(&taken).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&taken).unwrap(), b"mine");
        // Somewhere it can't go: an error, and the file is still removed.
        assert!(spooled(&dir, "e", b"eee").save_as(&dir.join("no/such/folder/e.txt")).is_err());
        assert!(!dir.join("e").exists());
        assert_eq!(left_in(&dir.join("saved")), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_file_request_tells_the_library_the_size_limit() {
        let limit = Some(crate::net::MAX_FILE_RESPONSE as usize);
        // With a folder (the app) or without (`rettui fetch`): a file is limited.
        let spool = Path::new("/spool");
        let app = request_options("/file/big.bin", Some(spool));
        assert_eq!((app.max_response_size, app.spool_dir.as_deref()), (limit, Some(spool)));
        let cli = request_options("/file/big.bin", None);
        assert_eq!((cli.max_response_size, cli.spool_dir), (limit, None));
        assert_eq!(cli.timeout, Some(request::TIMEOUT));
        // Pages keep the library's own limits, and are checked after.
        assert_eq!(request_options("/page/index.mu", None).max_response_size, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_file_gets_the_usual_permissions_and_no_link_is_followed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rettui-downloaded-unix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        // The spooled file is the owner's alone; the saved copy is as any new file.
        let file = spooled(&dir, "spooled", b"data");
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(dir.join("probe"), b"").unwrap();
        file.save_as(&dir.join("saved.bin")).unwrap();
        assert_eq!(mode(&dir.join("saved.bin")), mode(&dir.join("probe")));
        // A link put where the file is to go is not written through.
        let elsewhere = dir.join("elsewhere");
        std::fs::write(&elsewhere, b"keep").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("link.bin")).unwrap();
        assert!(spooled(&dir, "s2", b"data").save_as(&dir.join("link.bin")).is_err());
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
