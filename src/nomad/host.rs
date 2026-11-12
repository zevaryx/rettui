//! The NomadNet node hosted here, run by the network actor.
//!
//! The node serves `pages/` and `files/` from its folder at the
//! `nomadnetwork.node` destination of the identity, as NomadNet does. A
//! request names its path by a hash, so the node keeps the paths of what's
//! in its folder: new, renamed and deleted pages need [`Node::reload`].
//! Each page is read from disk for each request, so saved edits are live
//! straight away. A page that's a program is run for each visitor
//! (when "Run page scripts" is on), an `.allowed` list beside a page or
//! file says who may have it, and a page's pictures are sent as WebP.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use rns_identity::destination::DefaultAppData;
use rns_runtime::link_manager::{LinkManager, LinkManagerCommand, pack_file_name_metadata};
use rns_runtime::prelude::*;
use rns_transport::messages::TransportMessage;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use super::paths::{self, FILE_PREFIX, INDEX_PATH, MEDIA_PATH, NODE_ASPECT, PAGE_PREFIX, PathError};
use super::store::{self, MAX_FILE_BYTES, MAX_PAGE_BYTES};
use super::{pages, request, script};
use crate::config::{Paths, Settings, expand_home};

/// How the node should run, from the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    pub dir: PathBuf,
    pub name: String,
    pub announce_interval: Option<Duration>,
    pub executable_pages: bool,
    pub limits: script::Limits,
}

impl HostConfig {
    /// The node folder the settings point at (used even when not hosting,
    /// so pages can be written before the node is switched on).
    pub fn dir(settings: &Settings, paths: &Paths) -> PathBuf {
        settings.node_dir.as_deref().map_or_else(|| paths.node.clone(), expand_home)
    }

    /// `None` when hosting is off.
    pub fn from_settings(settings: &Settings, paths: &Paths) -> Option<Self> {
        settings.node_enabled.then(|| Self {
            dir: Self::dir(settings, paths),
            name: settings.node_name.clone().unwrap_or_else(|| settings.display_name.clone()),
            announce_interval: (settings.node_announce_interval_mins > 0)
                .then(|| Duration::from_secs(settings.node_announce_interval_mins * 60)),
            executable_pages: settings.node_executable_pages,
            limits: script::Limits::from_settings(settings),
        })
    }
}

/// What the node has served since it started.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// Requests taken, found or not.
    pub request_count: u64,
    pub page_hits: u64,
    pub file_hits: u64,
    /// Pictures sent.
    pub media_hits: u64,
    pub not_found_count: u64,
    /// When the last request came, in milliseconds since 1970.
    pub last_request_ms: Option<u64>,
    /// Different identities that identified themselves (a visitor who
    /// doesn't can't be told from another).
    #[serde(default)]
    pub visitors: u64,
}

/// The most requests answered in [`WINDOW`]; the rest get nothing, so a
/// flood of them can't keep the node (or its scripts) busy.
const MAX_REQUESTS: u32 = 60;
const WINDOW: Duration = Duration::from_secs(10);

/// The most identities remembered to count visitors.
const MAX_VISITORS: usize = 10_000;

/// Longest `.allowed` list read, in bytes (32 hex characters a line).
const MAX_ALLOWED: usize = 64 * 1024;

/// What `/media` sends: WebP as it is, the others made into WebP, no side
/// longer than [`MAX_SIDE`] pixels.
const PICTURES: [&str; 7] = ["webp", "png", "jpg", "jpeg", "gif", "bmp", "tiff"];
const MAX_SIDE: u32 = 1200;
/// The most pictures made into WebP that are kept, and their size together.
const MAX_KEPT: usize = 256;
const MAX_KEPT_BYTES: usize = 64 * 1024 * 1024;

/// NomadNet's own page for someone an `.allowed` list leaves out.
const NOT_ALLOWED: &str = ">Request Not Allowed\n\nYou are not authorised to carry out the request.\n";

fn failed(path: &str) -> String {
    format!("#!c=0\n>Page failed\n\nThe page {} could not be made on this node.\n", paths::clean_text(path))
}

#[derive(Default)]
struct Counts {
    requests: AtomicU64,
    pages: AtomicU64,
    files: AtomicU64,
    media: AtomicU64,
    not_found: AtomicU64,
    last_ms: AtomicU64,
    visitors: Mutex<HashSet<[u8; 16]>>,
}

/// Pictures already made into WebP, by the hash of what they were made from.
#[derive(Default)]
struct Converted {
    kept: HashMap<[u8; 32], Arc<Vec<u8>>>,
    bytes: usize,
}

/// What answering a request takes.
struct Shared {
    pages: PathBuf,
    files: PathBuf,
    /// "Run page scripts".
    scripts: bool,
    limits: script::Limits,
    counts: Counts,
    /// The paths answered at, by the hash a request names them with.
    routes: RwLock<HashMap<[u8; 16], String>>,
    /// When the current window began, and the requests taken in it.
    window: Mutex<(Instant, u32)>,
    converted: Mutex<Converted>,
}

/// A lock's contents, even if something panicked while holding it.
fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

impl Shared {
    fn new(config: &HostConfig) -> Self {
        Self {
            pages: config.dir.join("pages"),
            files: config.dir.join("files"),
            scripts: config.executable_pages,
            limits: config.limits.clone(),
            counts: Counts::default(),
            routes: RwLock::default(),
            window: Mutex::new((Instant::now(), 0)),
            converted: Mutex::default(),
        }
    }

    fn stats(&self) -> Stats {
        let counts = &self.counts;
        let last = counts.last_ms.load(Ordering::Relaxed);
        Stats {
            request_count: counts.requests.load(Ordering::Relaxed),
            page_hits: counts.pages.load(Ordering::Relaxed),
            file_hits: counts.files.load(Ordering::Relaxed),
            media_hits: counts.media.load(Ordering::Relaxed),
            not_found_count: counts.not_found.load(Ordering::Relaxed),
            last_request_ms: (last > 0).then_some(last),
            visitors: locked(&counts.visitors).len() as u64,
        }
    }

    /// Whether there's room for one more request in this window.
    fn admit(&self) -> bool {
        let mut window = locked(&self.window);
        if window.0.elapsed() >= WINDOW {
            *window = (Instant::now(), 0);
        }
        window.1 += 1;
        window.1 <= MAX_REQUESTS
    }

    fn missing(&self, path: &str) -> RequestOutcome {
        self.counts.not_found.fetch_add(1, Ordering::Relaxed);
        RequestOutcome::Reply(pages::not_found(path).into_bytes())
    }

    /// Whether `who` may have `file`: yes with no `.allowed` list beside
    /// it; with one, only an identity it names. A list that's a program is
    /// run (when scripts are), and what it prints is the list.
    fn allowed(&self, file: &Path, who: Option<[u8; 16]>) -> bool {
        let mut list = file.as_os_str().to_owned();
        list.push(".allowed");
        let list = PathBuf::from(list);
        let Ok(metadata) = std::fs::symlink_metadata(&list) else { return true };
        // A list that's a symlink could be anyone's file: no one, to be safe.
        if metadata.file_type().is_symlink() {
            return false;
        }
        if !metadata.is_file() {
            return true;
        }
        let Some(who) = who else { return false };
        let text = if self.scripts && is_executable(&list) {
            let run = script::run(&list, Some(who), &Default::default(), MAX_ALLOWED, &self.limits);
            if run.failure.is_some() {
                return false;
            }
            run.stdout
        } else if metadata.len() > MAX_ALLOWED as u64 {
            return false;
        } else {
            match std::fs::read(&list) {
                Ok(text) => text,
                Err(_) => return false,
            }
        };
        pages::allowed_hashes(&String::from_utf8_lossy(&text)).contains(&who)
    }

    /// Look at the folder again: answer at the pages and files that are
    /// there now, at the index (even before it's there) and at `/media`.
    fn reload(&self) -> Result<(), String> {
        let mut paths = vec![INDEX_PATH.to_string(), MEDIA_PATH.to_string()];
        for (root, prefix) in [(&self.pages, PAGE_PREFIX), (&self.files, FILE_PREFIX)] {
            paths.extend(store::list(root).map_err(|e| e.to_string())?.into_iter().map(|entry| format!("{prefix}{}", entry.path)));
        }
        let routes = paths.into_iter().map(|path| (rns_crypto::sha::truncated_hash(path.as_bytes()), path)).collect();
        *self.routes.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = routes;
        Ok(())
    }

    fn answer(&self, link_id: [u8; 16], path_hash: [u8; 16], data: &[u8], who: Option<[u8; 16]>) -> RequestOutcome {
        if !self.admit() {
            return RequestOutcome::Drop;
        }
        self.counts.requests.fetch_add(1, Ordering::Relaxed);
        self.counts.last_ms.store((crate::app::now() * 1000.0) as u64, Ordering::Relaxed);
        if let Some(who) = who {
            let mut visitors = locked(&self.counts.visitors);
            if visitors.len() < MAX_VISITORS {
                visitors.insert(who);
            }
        }
        let path = self.routes.read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&path_hash).cloned();
        // Nothing here by that hash: said at once, rather than leaving the visitor waiting.
        let Some(path) = path else { return self.missing("asked for") };
        if path == MEDIA_PATH {
            self.picture(data, who)
        } else if path.starts_with(PAGE_PREFIX) {
            self.page(&path, link_id, data, who)
        } else {
            self.file(&path, who)
        }
    }

    fn page(&self, path: &str, link_id: [u8; 16], data: &[u8], who: Option<[u8; 16]>) -> RequestOutcome {
        let Ok(name) = paths::page_name(path) else { return RequestOutcome::Drop };
        let file = match paths::resolve(&self.pages, name) {
            Ok(file) => file,
            Err(PathError::NotFound | PathError::Outside) => return self.missing(path),
            Err(_) => return RequestOutcome::Drop,
        };
        if !self.allowed(&file, who) {
            return RequestOutcome::Reply(NOT_ALLOWED.as_bytes().to_vec());
        }
        if is_executable(&file) {
            // With scripts off a program's text is never sent: it could hold secrets.
            if !self.scripts {
                return self.missing(path);
            }
            let fields = request::decode_fields(data);
            let run = script::run_on(link_id, &file, who, &fields, MAX_PAGE_BYTES, &self.limits);
            return match run.failure {
                None => {
                    self.counts.pages.fetch_add(1, Ordering::Relaxed);
                    RequestOutcome::Reply(run.stdout)
                }
                Some(why) => {
                    tracing::warn!("script page {path} failed: {why}");
                    RequestOutcome::Reply(failed(path).into_bytes())
                }
            };
        }
        match store::read(&self.pages, name, MAX_PAGE_BYTES) {
            Ok(page) => {
                self.counts.pages.fetch_add(1, Ordering::Relaxed);
                RequestOutcome::Reply(page)
            }
            Err(PathError::NotFound) => self.missing(path),
            Err(e) => {
                tracing::warn!("page {path} couldn't be sent: {e}");
                RequestOutcome::Drop
            }
        }
    }

    fn file(&self, path: &str, who: Option<[u8; 16]>) -> RequestOutcome {
        let Ok(name) = paths::file_name(path) else { return RequestOutcome::Drop };
        let Ok(file) = paths::resolve(&self.files, name) else {
            self.counts.not_found.fetch_add(1, Ordering::Relaxed);
            return RequestOutcome::Drop;
        };
        if !self.allowed(&file, who) {
            return RequestOutcome::Reply(NOT_ALLOWED.as_bytes().to_vec());
        }
        match store::read(&self.files, name, MAX_FILE_BYTES) {
            Ok(data) => {
                self.counts.files.fetch_add(1, Ordering::Relaxed);
                RequestOutcome::ReplyFile { data, metadata: Some(pack_file_name_metadata(name)), auto_compress: true }
            }
            Err(e) => {
                if matches!(e, PathError::NotFound) {
                    self.counts.not_found.fetch_add(1, Ordering::Relaxed);
                } else {
                    tracing::warn!("file {path} couldn't be sent: {e}");
                }
                RequestOutcome::Drop
            }
        }
    }

    /// One of a page's pictures, from `pages/`. Whatever is wrong with the
    /// request, nothing is sent back.
    fn picture(&self, data: &[u8], who: Option<[u8; 16]>) -> RequestOutcome {
        let Some(asked) = request::decode_media(data) else { return RequestOutcome::Drop };
        let asked = asked.trim();
        let name = asked.strip_prefix("/media/").unwrap_or(asked).trim_start_matches('/');
        let Some((stem, extension)) = name.rsplit('/').next().and_then(|file| file.rsplit_once('.')) else { return RequestOutcome::Drop };
        let extension = extension.to_ascii_lowercase();
        if !PICTURES.contains(&extension.as_str()) {
            return RequestOutcome::Drop;
        }
        let Ok(file) = paths::resolve(&self.pages, name) else { return RequestOutcome::Drop };
        if !self.allowed(&file, who) {
            return RequestOutcome::Drop;
        }
        let source = match store::read(&self.pages, name, MAX_FILE_BYTES) {
            Ok(source) => source,
            Err(e) => {
                if matches!(e, PathError::NotFound) {
                    self.counts.not_found.fetch_add(1, Ordering::Relaxed);
                }
                return RequestOutcome::Drop;
            }
        };
        let (data, sent_as) = if extension == "webp" {
            (source, name.rsplit('/').next().unwrap_or(name).to_string())
        } else {
            match self.as_webp(&source) {
                Some(webp) => (webp.to_vec(), format!("{stem}.webp")),
                None => {
                    tracing::warn!("picture {name} couldn't be made into WebP");
                    return RequestOutcome::Drop;
                }
            }
        };
        self.counts.media.fetch_add(1, Ordering::Relaxed);
        RequestOutcome::ReplyFile { data, metadata: Some(pack_file_name_metadata(&sent_as)), auto_compress: true }
    }

    /// A picture as WebP, made once and kept for the next visitor.
    fn as_webp(&self, source: &[u8]) -> Option<Arc<Vec<u8>>> {
        use sha2::Digest;
        let key: [u8; 32] = sha2::Sha256::digest(source).into();
        if let Some(kept) = locked(&self.converted).kept.get(&key) {
            return Some(kept.clone());
        }
        let webp = Arc::new(to_webp(source)?);
        let mut converted = locked(&self.converted);
        if converted.kept.len() >= MAX_KEPT || converted.bytes + webp.len() > MAX_KEPT_BYTES {
            *converted = Converted::default();
        }
        if webp.len() <= MAX_KEPT_BYTES {
            converted.bytes += webp.len();
            converted.kept.insert(key, webp.clone());
        }
        Some(webp)
    }
}

/// A picture as (lossless) WebP, made smaller if a side is over [`MAX_SIDE`].
fn to_webp(source: &[u8]) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(source)).with_guessed_format().ok()?;
    // A small file can claim to be a huge picture.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    let picture = if picture.width().max(picture.height()) > MAX_SIDE {
        picture.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        picture
    };
    let pixels = picture.to_rgba8();
    let mut webp = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .encode(pixels.as_raw(), pixels.width(), pixels.height(), image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(webp)
}

/// A running node.
pub struct Node {
    hash: [u8; 16],
    shared: Arc<Shared>,
    name: String,
    /// What the node's link manager is asked to do: announce, and stop.
    commands: mpsc::Sender<LinkManagerCommand>,
    /// Hears when the link manager has stopped.
    stopped: Option<oneshot::Receiver<()>>,
    _announcer: Announcer,
}

/// The task that announces the node, stopped when the node goes.
struct Announcer(tokio::task::JoinHandle<()>);

impl Drop for Announcer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn announce(commands: &mpsc::Sender<LinkManagerCommand>, name: &str) -> Result<(), String> {
    let (result_tx, result) = oneshot::channel();
    let options = DestinationAnnounceOptions { app_data: Some(name.as_bytes().to_vec()), ..DestinationAnnounceOptions::default() };
    const GONE: &str = "the node has stopped";
    commands.send(LinkManagerCommand::AnnounceWith { options, result_tx: Some(result_tx) }).await.map_err(|_| GONE.to_string())?;
    result.await.map_err(|_| GONE.to_string())?.map_err(|e| e.to_string())
}

/// Start the node on the running Reticulum instance.
pub async fn start(runtime: &ReticulumHandle, identity: &Identity, config: &HostConfig) -> Result<Node, String> {
    let name = paths::clamp_name(&config.name);
    pages::Pages::open(&config.dir)?.ensure_index(&name)?;
    let shared = Arc::new(Shared::new(config));
    let listing = shared.clone();
    tokio::task::spawn_blocking(move || listing.reload())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Could not start the node: {e}"))?;

    // A link manager of the node's own, so one handler gets every request,
    // whatever path it names: a destination registered with the runtime
    // answers only at paths it knows, and says nothing for the rest.
    let transport = runtime.transport_tx.clone();
    let (delivery_tx, delivery_rx) = mpsc::channel(256);
    let mut manager = LinkManager::with_destination(transport.clone(), delivery_rx, identity, NODE_ASPECT, identity.get_signing_key());
    manager.set_use_implicit_proof(true);
    // Nothing is taken from visitors.
    manager.set_resource_strategy(ResourceStrategy::AcceptNone);
    let Some(destination) = manager.destination_mut() else { return Err("Could not start the node: its address couldn't be made".into()) };
    let hash = destination.hash;
    destination.set_default_app_data(DefaultAppData::Static(name.as_bytes().to_vec()));
    let serving = shared.clone();
    manager.set_request_handler_ex(move |link_id, path_hash, data, remote_identity| {
        let who = remote_identity.map(|identity| identity.hash);
        // A script or a big picture takes a while: let the runtime's other threads carry on meanwhile.
        match tokio::runtime::Handle::try_current() {
            Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| serving.answer(link_id, path_hash, &data, who))
            }
            _ => serving.answer(link_id, path_hash, &data, who),
        }
    });
    transport
        .send(TransportMessage::RegisterDestination { hash, app_name: NODE_ASPECT.to_string(), delivery_tx: Some(delivery_tx) })
        .await
        .map_err(|_| "Could not start the node: Reticulum has stopped".to_string())?;
    let (commands, command_rx) = mpsc::channel(32);
    let (stopped_tx, stopped) = oneshot::channel();
    tokio::spawn(async move {
        manager.run_with_commands(command_rx).await;
        let _ = transport.send(TransportMessage::DeregisterDestination { hash }).await;
        let _ = stopped_tx.send(());
    });

    // Announced now, and again every so often if the settings say so.
    let (announcing, announced, every) = (commands.clone(), name.clone(), config.announce_interval);
    let announcer = tokio::spawn(async move {
        loop {
            if let Err(e) = announce(&announcing, &announced).await {
                tracing::warn!("node announce failed: {e}");
            }
            match every {
                Some(every) => tokio::time::sleep(every).await,
                None => break,
            }
        }
    });
    Ok(Node { hash, shared, name, commands, stopped: Some(stopped), _announcer: Announcer(announcer) })
}

impl Node {
    pub fn destination_hash(&self) -> [u8; 16] {
        self.hash
    }

    pub fn stats(&self) -> Stats {
        self.shared.stats()
    }

    /// Look at the folder again, for pages and files that have come or gone.
    pub async fn reload(&self) -> Result<(), String> {
        let shared = self.shared.clone();
        tokio::task::spawn_blocking(move || shared.reload()).await.map_err(|e| e.to_string())?
    }

    pub async fn announce_now(&self) -> Result<(), String> {
        announce(&self.commands, &self.name).await
    }

    /// Stop answering, closing the links so visitors see it go.
    pub async fn stop(mut self) {
        let _ = self.commands.send(LinkManagerCommand::Shutdown).await;
        if let Some(stopped) = self.stopped.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), stopped).await;
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        // Not stopped properly (a start that was no longer wanted): stop it all the same.
        if self.stopped.is_some() {
            let _ = self.commands.try_send(LinkManagerCommand::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use rmpv::Value;

    use super::*;

    struct Fixture {
        dir: PathBuf,
        shared: Shared,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn node(name: &str, scripts: bool) -> Fixture {
        let dir = std::env::temp_dir().join(format!("rettui-test-nomad-host-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = HostConfig {
            dir: dir.clone(),
            name: "Test".into(),
            announce_interval: None,
            executable_pages: scripts,
            limits: script::Limits::default(),
        };
        pages::Pages::open(&dir).unwrap();
        Fixture { shared: Shared::new(&config), dir }
    }

    impl Fixture {
        fn put(&self, path: &str, data: &[u8]) {
            let file = self.dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, data).unwrap();
        }

        #[cfg(unix)]
        fn put_program(&self, path: &str, text: &str) {
            use std::os::unix::fs::PermissionsExt;
            self.put(path, text.as_bytes());
            std::fs::set_permissions(self.dir.join(path), std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        /// Ask as a visitor does, the folder looked at just before.
        fn ask(&self, path: &str, data: &[u8], who: Option<&Identity>) -> RequestOutcome {
            self.shared.reload().unwrap();
            self.shared.answer([0xcd; 16], rns_crypto::sha::truncated_hash(path.as_bytes()), data, who.map(|identity| identity.hash))
        }

        fn page(&self, path: &str, who: Option<&Identity>) -> String {
            match self.ask(path, b"", who) {
                RequestOutcome::Reply(page) => String::from_utf8(page).unwrap(),
                other => panic!("{path}: {other:?}"),
            }
        }
    }

    fn file_reply(outcome: RequestOutcome) -> (Vec<u8>, String) {
        match outcome {
            RequestOutcome::ReplyFile { data, metadata, auto_compress: true } => {
                (data, request::reply_file_name(metadata.as_deref()).unwrap())
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pages_and_files_are_served_and_counted() {
        let node = node("serve", false);
        node.put("pages/index.mu", b">Home\n");
        node.put("pages/blog/one.mu", b">One\n");
        node.put("files/docs/manual.pdf", b"%PDF");
        assert_eq!(node.page("/page/index.mu", None), ">Home\n");
        assert_eq!(node.page("/page/blog/one.mu", None), ">One\n");
        // An edit is live straight away.
        node.put("pages/index.mu", b">Home again\n");
        assert_eq!(node.page("/page/index.mu", None), ">Home again\n");
        assert_eq!(file_reply(node.ask("/file/docs/manual.pdf", b"", None)), (b"%PDF".to_vec(), "docs/manual.pdf".to_string()));

        // Never there, not something the node keeps, or not an address at
        // all: a page says so at once.
        node.put("pages/x.mu.allowed", b"");
        node.put("pages/.hidden.mu", b"hidden");
        for path in
            ["/page/gone.mu", "/file/gone.pdf", "/page/../files/docs/manual.pdf", "/page/x.mu.allowed", "/page/.hidden.mu", "/other", ""]
        {
            let missing = node.page(path, None);
            assert!(missing.contains(">Not found") && missing.contains("asked for"), "{path}: {missing}");
        }
        // Gone since the folder was last looked at: a page says so, a file gets nothing.
        std::fs::remove_file(node.dir.join("pages/blog/one.mu")).unwrap();
        std::fs::remove_file(node.dir.join("files/docs/manual.pdf")).unwrap();
        let hash = |path: &str| rns_crypto::sha::truncated_hash(path.as_bytes());
        let RequestOutcome::Reply(gone) = node.shared.answer([0; 16], hash("/page/blog/one.mu"), b"", None) else { panic!() };
        assert!(String::from_utf8(gone).unwrap().contains("The page /page/blog/one.mu is not available"));
        assert!(matches!(node.shared.answer([0; 16], hash("/file/docs/manual.pdf"), b"", None), RequestOutcome::Drop));
        // A visitor who identifies is counted once.
        let ann = Identity::new();
        node.page("/page/index.mu", Some(&ann));
        node.page("/page/index.mu", Some(&ann));
        let stats = node.shared.stats();
        assert_eq!((stats.request_count, stats.page_hits, stats.file_hits, stats.media_hits), (15, 5, 1, 0));
        assert_eq!((stats.not_found_count, stats.visitors), (9, 1));
        assert!(stats.last_request_ms.is_some());
    }

    #[test]
    fn an_allowed_list_says_who_may_have_something() {
        let node = node("allowed", false);
        let (ann, ben) = (Identity::new(), Identity::new());
        node.put("pages/club.mu", b">Club\n");
        node.put("pages/club.mu.allowed", format!("# members\n {} \nnot a hash\n", hex::encode(ann.hash)).as_bytes());
        node.put("files/minutes.txt", b"minutes");
        node.put("files/minutes.txt.allowed", hex::encode(ann.hash).as_bytes());
        node.put("pages/images/badge.webp", b"RIFF");
        node.put("pages/images/badge.webp.allowed", hex::encode(ann.hash).as_bytes());

        assert_eq!(node.page("/page/club.mu", Some(&ann)), ">Club\n");
        assert_eq!(file_reply(node.ask("/file/minutes.txt", b"", Some(&ann))).0, b"minutes");
        let picture = request::media("images/badge.webp").data;
        assert_eq!(file_reply(node.ask("/media", &picture, Some(&ann))).0, b"RIFF");
        // Someone else, or no one in particular: NomadNet's own page, and no picture.
        for who in [Some(&ben), None] {
            assert_eq!(node.page("/page/club.mu", who), NOT_ALLOWED);
            assert!(matches!(node.ask("/file/minutes.txt", b"", who), RequestOutcome::Reply(page) if page == NOT_ALLOWED.as_bytes()));
            assert!(matches!(node.ask("/media", &picture, who), RequestOutcome::Drop));
        }
        // An empty list is no one; the list itself is never sent.
        node.put("pages/club.mu.allowed", b"");
        assert_eq!(node.page("/page/club.mu", Some(&ann)), NOT_ALLOWED);
        assert!(node.page("/page/club.mu.allowed", Some(&ann)).contains(">Not found"));
        // A list too long to be one is no one too.
        node.put("pages/club.mu.allowed", format!("{}\n", hex::encode(ann.hash)).repeat(2000).as_bytes());
        assert_eq!(node.page("/page/club.mu", Some(&ann)), NOT_ALLOWED);
    }

    #[cfg(unix)]
    #[test]
    fn a_page_thats_a_program_runs_for_each_visitor() {
        let node = node("scripts", true);
        let ann = Identity::new();
        node.put_program("pages/hello.mu", "#!/bin/sh\necho \">Hello $field_name ($var_mode) $remote_identity $link_id\"\n");
        let form = request::encode_fields(
            &[("field_name", "Ann"), ("var_mode", "x"), ("PATH", "/evil"), ("other", "y")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
        );
        let RequestOutcome::Reply(page) = node.ask("/page/hello.mu", &form, Some(&ann)) else { panic!() };
        assert_eq!(String::from_utf8(page).unwrap(), format!(">Hello Ann (x) {} {}\n", hex::encode(ann.hash), "cd".repeat(16)));
        // One that fails: a page that says so, not counted as served.
        node.put_program("pages/broken.mu", "#!/bin/nothing-here\n");
        let failed = node.page("/page/broken.mu", None);
        assert!(failed.contains(">Page failed") && failed.contains("/page/broken.mu"), "{failed}");
        assert_eq!(node.shared.stats().page_hits, 1);

        // An .allowed list that's a program prints who may come in.
        node.put("pages/club.mu", b">Club\n");
        node.put_program("pages/club.mu.allowed", &format!("#!/bin/sh\necho {}\n", hex::encode(ann.hash)));
        assert_eq!(node.page("/page/club.mu", Some(&ann)), ">Club\n");
        assert_eq!(node.page("/page/club.mu", Some(&Identity::new())), NOT_ALLOWED);
        node.put_program("pages/club.mu.allowed", "#!/bin/nothing-here\n");
        assert_eq!(node.page("/page/club.mu", Some(&ann)), NOT_ALLOWED);
    }

    #[cfg(unix)]
    #[test]
    fn with_scripts_off_a_programs_text_is_never_sent() {
        let node = node("scripts-off", false);
        let ann = Identity::new();
        node.put_program("pages/secret.mu", "#!/bin/sh\n# password: hunter2\necho hi\n");
        let page = node.page("/page/secret.mu", None);
        assert!(page.contains(">Not found") && !page.contains("hunter2"), "{page}");
        // A list that's a program is read as a list instead.
        node.put("pages/club.mu", b">Club\n");
        node.put_program("pages/club.mu.allowed", &format!("{}\n", hex::encode(ann.hash)));
        assert_eq!(node.page("/page/club.mu", Some(&ann)), ">Club\n");
        assert_eq!(node.page("/page/club.mu", None), NOT_ALLOWED);
    }

    #[test]
    fn pictures_are_sent_as_webp() {
        let node = node("pictures", false);
        let mut png = Vec::new();
        let wide = image::RgbaImage::from_pixel(2400, 600, image::Rgba([200, 30, 30, 255]));
        image::DynamicImage::ImageRgba8(wide).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        node.put("pages/images/banner.png", &png);
        node.put("pages/images/ready.webp", b"as it is");
        node.put("pages/images/broken.png", b"not a picture");
        node.put("pages/notes.txt", b"notes");

        // Made into WebP, no side over the limit, under the WebP's name.
        for asked in ["images/banner.png", "/media/images/banner.png", "/images/banner.png"] {
            let (data, name) = file_reply(node.ask("/media", &request::media(asked).data, None));
            assert_eq!(name, "banner.webp");
            let sent = image::load_from_memory_with_format(&data, image::ImageFormat::WebP).unwrap();
            assert_eq!((sent.width(), sent.height()), (1200, 300), "{asked}");
        }
        assert_eq!(locked(&node.shared.converted).kept.len(), 1);
        let mut tiff = Vec::new();
        let small = image::RgbaImage::from_pixel(8, 4, image::Rgba([0, 0, 0, 255]));
        image::DynamicImage::ImageRgba8(small).write_to(&mut std::io::Cursor::new(&mut tiff), image::ImageFormat::Tiff).unwrap();
        node.put("pages/scan.TIFF", &tiff);
        assert_eq!(file_reply(node.ask("/media", &request::media("scan.TIFF").data, None)).1, "scan.webp");
        assert_eq!(
            file_reply(node.ask("/media", &request::media("images/ready.webp").data, None)),
            (b"as it is".to_vec(), "ready.webp".into())
        );
        // Not a picture, not there, not in pages/, or not a request for one.
        let bytes = Value::Map(vec![(Value::from("path"), Value::Binary(b"images/ready.webp".to_vec())), (Value::from("key"), Value::Nil)]);
        let mut odd = Vec::new();
        rmpv::encode::write_value(&mut odd, &bytes).unwrap();
        for data in [
            request::media("images/broken.png").data,
            request::media("images/gone.png").data,
            request::media("notes.txt").data,
            request::media("../files/x.png").data,
            request::media("").data,
            odd,
            Vec::new(),
        ] {
            assert!(matches!(node.ask("/media", &data, None), RequestOutcome::Drop));
        }
        let stats = node.shared.stats();
        assert_eq!((stats.media_hits, stats.not_found_count), (5, 1));
    }

    #[test]
    fn a_flood_of_requests_gets_nothing_until_it_eases() {
        let node = node("flood", false);
        node.put("pages/index.mu", b">Home\n");
        for _ in 0..MAX_REQUESTS {
            node.page("/page/index.mu", None);
        }
        assert!(matches!(node.ask("/page/index.mu", b"", None), RequestOutcome::Drop));
        assert_eq!(node.shared.stats().request_count, u64::from(MAX_REQUESTS));
        // A new window.
        locked(&node.shared.window).0 = Instant::now().checked_sub(WINDOW).unwrap();
        assert_eq!(node.page("/page/index.mu", None), ">Home\n");
    }

    #[test]
    fn the_node_answers_at_what_is_in_its_folder() {
        let node = node("routes", false);
        let all = |node: &Fixture| {
            node.shared.reload().unwrap();
            let mut paths: Vec<String> = node.shared.routes.read().unwrap().values().cloned().collect();
            paths.sort();
            paths
        };
        assert_eq!(all(&node), ["/media", "/page/index.mu"]);
        node.put("pages/blog/one.mu", b"one");
        node.put("pages/.draft.mu", b"draft");
        node.put("pages/blog/one.mu.allowed", b"");
        node.put("files/manual.pdf", b"pdf");
        assert_eq!(all(&node), ["/file/manual.pdf", "/media", "/page/blog/one.mu", "/page/index.mu"]);
    }
}
