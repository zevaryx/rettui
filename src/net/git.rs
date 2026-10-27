//! rngit, Git over Reticulum: requests to an rngit node's
//! `git.repositories` destination, for the Repos tab. A repository's
//! branches and tags, its releases (and their files, checked against the
//! signed manifest), its work documents and its permissions; and writing
//! documents and releases, signed with the identity the node knows you by.
//! The protocol is the `rngit` crate's; this carries it over rettui's links
//! and runs each request as a task of its own.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rngit::client::{ReleaseDetail, ReleaseList, WorkKind, WorkList, WorkScope};
use rngit::rns::IdentitySigner;
use rngit::sign::Signer;
use rngit::{Client, Limits, RemoteRefs, RepoUrl, Response, Status, Transport};
use rns_runtime::prelude::*;
use tokio::sync::{Mutex, mpsc};

use super::{Hash, NetEvent, remote};

/// What rngit nodes announce under.
pub const GIT_ASPECT: &str = "git.repositories";

/// Longest a request may take unless the settings say otherwise, a
/// release's files included.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Largest release file taken (it's held in memory).
pub const MAX_ARTIFACT: usize = 64 * 1024 * 1024;
/// Longest document, comment or release notes kept, in characters.
pub const MAX_TEXT: usize = 100_000;
/// Most comments kept of a document.
pub const MAX_COMMENTS: usize = 500;
/// Links to nodes not asked anything for this long are closed.
const IDLE: Duration = Duration::from_secs(5 * 60);

/// What to ask a repository.
#[derive(Debug, Clone, PartialEq)]
pub enum GitRequest {
    /// Its branches and tags.
    Refs,
    /// Its `.allowed` file.
    Perms,
    Releases,
    Release(String),
    /// One file of a release, checked against the release's manifest.
    Artifact {
        tag: String,
        name: String,
    },
    Work(WorkScope),
    WorkView {
        id: u64,
        scope: WorkScope,
    },
    WorkNew {
        kind: WorkKind,
        title: String,
        text: String,
    },
    WorkComment {
        id: u64,
        scope: WorkScope,
        text: String,
    },
    WorkEdit {
        id: u64,
        scope: WorkScope,
        title: String,
        text: String,
    },
    WorkComplete(u64),
    WorkActivate(u64),
    WorkDelete {
        id: u64,
        scope: WorkScope,
    },
    /// Publish a release of a tag already pushed, with these files.
    ReleaseNew {
        tag: String,
        notes: String,
        files: Vec<(String, Vec<u8>)>,
    },
    ReleaseDelete(String),
}

/// A comment on a work document, as the node keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: u64,
    /// The identity hash (hex) of who wrote it.
    pub author: String,
    pub created: f64,
    pub text: String,
}

/// A work document, cleaned for showing.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub id: u64,
    pub scope: String,
    pub title: String,
    /// The identity hash (hex) the node says wrote it.
    pub author: String,
    pub created: f64,
    pub edited: f64,
    pub text: String,
    /// The identity hash (hex) of the key it's signed with, when the
    /// signature checks out.
    pub signer: Option<String>,
    pub comments: Vec<Comment>,
}

/// What came back.
#[derive(Debug, Clone, PartialEq)]
pub enum GitReply {
    Refs(RemoteRefs),
    Perms(String),
    Releases(ReleaseList),
    Release(ReleaseDetail),
    /// A release file, and the identity hash (hex) of who signed it.
    Artifact {
        name: String,
        data: Vec<u8>,
        signer: String,
    },
    Work(WorkList),
    Doc(Doc),
    /// A change made: what to say of it.
    Done(String),
}

/// Text from a node, safe to draw: no control characters but line breaks
/// and tabs, and at most `max` characters.
pub fn clean_text(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).take(max).collect()
}

/// An error, as rettui says it.
pub fn describe(error: rngit::Error) -> String {
    match error {
        rngit::Error::Remote { status: Status::NotAllowed, message } => {
            format!("The node doesn't allow you that ({}). Its owner grants it to your identity", clean_text(&message, 200))
        }
        rngit::Error::Remote { status: Status::NotFound, message } => format!("Not on the node: {}", clean_text(&message, 200)),
        other => clean_text(&other.to_string(), 500),
    }
}

/// Run one request with `client`, signing with `signer` where it writes.
pub async fn run<T: Transport>(client: &Client<T>, repo: &RepoUrl, signer: &dyn Signer, request: GitRequest) -> Result<GitReply, String> {
    let reply = match request {
        GitRequest::Refs => GitReply::Refs(client.list(repo, false).await.map_err(describe)?),
        GitRequest::Perms => GitReply::Perms(clean_text(&client.repo_perms(repo).await.map_err(describe)?, MAX_TEXT)),
        GitRequest::Releases => GitReply::Releases(client.releases(repo).await.map_err(describe)?),
        GitRequest::Release(tag) => GitReply::Release(client.release(repo, &tag).await.map_err(describe)?),
        GitRequest::Artifact { tag, name } => artifact(client, repo, &tag, &name).await?,
        GitRequest::Work(scope) => GitReply::Work(client.work_list(repo, scope).await.map_err(describe)?),
        GitRequest::WorkView { id, scope } => GitReply::Doc(doc(client.work_view(repo, id, scope).await.map_err(describe)?)),
        GitRequest::WorkNew { kind, title, text } => {
            let (id, scope) = client.work_create(repo, signer, kind, &title, &text, "markdown").await.map_err(describe)?;
            GitReply::Done(format!("Made #{id} ({})", clean_text(&scope, 20)))
        }
        GitRequest::WorkComment { id, scope, text } => {
            client.work_comment(repo, id, scope, &text, "markdown").await.map_err(describe)?;
            GitReply::Done(format!("Commented on #{id}"))
        }
        GitRequest::WorkEdit { id, scope, title, text } => {
            client.work_edit(repo, signer, id, scope, &title, &text).await.map_err(describe)?;
            GitReply::Done(format!("Changed #{id}"))
        }
        GitRequest::WorkComplete(id) => {
            let (id, _) = client.work_complete(repo, id).await.map_err(describe)?;
            GitReply::Done(format!("Marked #{id} done"))
        }
        GitRequest::WorkActivate(id) => {
            let (id, _) = client.work_activate(repo, id).await.map_err(describe)?;
            GitReply::Done(format!("Made #{id} active again"))
        }
        GitRequest::WorkDelete { id, scope } => {
            client.work_delete(repo, id, scope).await.map_err(describe)?;
            GitReply::Done(format!("Deleted #{id}"))
        }
        GitRequest::ReleaseNew { tag, notes, files } => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let released =
                chrono::DateTime::from_timestamp(now as i64, 0).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_default();
            client.create_release(repo, signer, &tag, None, &notes, &files, &released, now).await.map_err(describe)?;
            GitReply::Done(format!("Published {tag}"))
        }
        GitRequest::ReleaseDelete(tag) => {
            client.delete_release(repo, &tag).await.map_err(describe)?;
            GitReply::Done(format!("Deleted the release {tag}"))
        }
    };
    Ok(reply)
}

/// One release file, with its signature checked against the release's
/// manifest (itself signed): a node can't hand over another file.
async fn artifact<T: Transport>(client: &Client<T>, repo: &RepoUrl, tag: &str, name: &str) -> Result<GitReply, String> {
    let rsm = client.fetch_artifact(repo, tag, "manifest.rsm").await.map_err(describe)?;
    let manifest = rngit::sign::verify_manifest(&rsm).map_err(|e| format!("The release's manifest doesn't check out: {}", describe(e)))?;
    let listed = manifest.artifacts.iter().find(|a| a.name == name).ok_or_else(|| format!("The release's manifest doesn't list {name}"))?;
    let data = client.fetch_artifact(repo, tag, name).await.map_err(describe)?;
    let signed = rngit::sign::verify_rsg(&data, &listed.rsg).map_err(|e| format!("{name} doesn't match its signature: {}", describe(e)))?;
    if signed.signer != manifest.signer.signer {
        return Err(format!("{name} was signed by someone else than the release"));
    }
    Ok(GitReply::Artifact { name: name.to_string(), data, signer: hex::encode(signed.signer) })
}

/// A document as shown: its text cleaned, its signature's identity, and
/// its comments read (each `{id, content, created, author, …}`, as stock
/// nodes send them; anything else is left out).
fn doc(doc: rngit::client::WorkDoc) -> Doc {
    let signer = doc.identity.filter(|_| doc.verified).map(|key| hex::encode(rngit::sign::identity_hash(&key)));
    let comments = doc
        .comments
        .iter()
        .take(MAX_COMMENTS)
        .filter_map(|value| {
            let map = value.as_map()?;
            let get = |key: &str| map.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v);
            Some(Comment {
                id: get("id").and_then(rmpv::Value::as_u64).unwrap_or(0),
                author: clean_text(get("author").and_then(rmpv::Value::as_str).unwrap_or_default(), 64),
                created: get("created").and_then(rmpv::Value::as_f64).unwrap_or(0.0),
                text: clean_text(get("content").and_then(rmpv::Value::as_str)?, MAX_TEXT),
            })
        })
        .collect();
    Doc {
        id: doc.id,
        scope: clean_text(&doc.scope, 20),
        title: clean_text(&doc.title, 200),
        author: clean_text(&doc.author, 64),
        created: doc.created,
        edited: doc.edited,
        text: clean_text(&doc.content, MAX_TEXT),
        signer,
        comments,
    }
}

/// What a node may send, kept to what rettui holds in memory.
fn limits() -> Limits {
    Limits { max_artifact_bytes: MAX_ARTIFACT, max_refs: 10_000, max_list_entries: 2_000, ..Limits::default() }
}

/// rngit over a link of rettui's: a path found first, a stale one dropped
/// and found again (see [`remote::connect`]), the link kept for the next
/// request and opened again when it drops. One request at a time.
pub struct LinkTransport {
    runtime: ReticulumHandle,
    identity: Identity,
    destination: Hash,
    timeout: Mutex<Duration>,
    link: Mutex<Option<LinkSession>>,
}

impl LinkTransport {
    fn new(runtime: ReticulumHandle, identity: Identity, destination: Hash) -> Self {
        LinkTransport { runtime, identity, destination, timeout: Mutex::new(DEFAULT_TIMEOUT), link: Mutex::new(None) }
    }

    async fn close(&self) {
        if let Some(link) = self.link.lock().await.take() {
            link.handle.close().await;
        }
    }
}

/// A reply, or a file: only a Resource reply carries metadata.
fn classify(response: LinkSessionResponse) -> Response {
    match response.metadata {
        None => Response::Packet(response.data),
        metadata => Response::File { data: response.data, metadata },
    }
}

impl Transport for LinkTransport {
    async fn request(&self, path: &str, data: Vec<u8>) -> Result<Response, rngit::Error> {
        let timeout = *self.timeout.lock().await;
        let mut link = self.link.lock().await;
        for attempt in 0..2 {
            if link.is_none() {
                let options = remote::link_options("rngit", true);
                let session = remote::connect(&self.runtime, self.destination, self.identity.clone(), options, &|_| {}).await;
                *link = Some(session.map_err(|e| rngit::Error::Transport(format!("Couldn't reach the node: {e}")))?);
            }
            let session = link.as_ref().ok_or_else(|| rngit::Error::Transport("no link".into()))?;
            match session.handle.request(path, &data, Some(timeout)).await {
                Ok(response) => return Ok(classify(response)),
                // A link gone quiet is worth one fresh try.
                Err(LinkSessionError::SessionClosed | LinkSessionError::LinkNotActive) if attempt == 0 => *link = None,
                Err(e) => return Err(rngit::Error::Transport(e.to_string())),
            }
        }
        Err(rngit::Error::Transport("the link kept dropping".into()))
    }
}

/// A client, and when it was last used.
type Kept = (Arc<Client<LinkTransport>>, Instant);

/// Clients by node and identity.
#[derive(Default, Clone)]
pub struct Clients(Arc<Mutex<HashMap<(Hash, Hash), Kept>>>);

impl Clients {
    async fn get(&self, runtime: &ReticulumHandle, identity: &Identity, destination: Hash) -> Arc<Client<LinkTransport>> {
        let mut clients = self.0.lock().await;
        let mut idle = Vec::new();
        clients.retain(|_, (client, used)| {
            let keep = used.elapsed() < IDLE;
            if !keep {
                idle.push(client.clone());
            }
            keep
        });
        let entry = clients.entry((destination, identity.hash)).or_insert_with(|| {
            let transport = LinkTransport::new(runtime.clone(), identity.clone(), destination);
            (Arc::new(Client::new(transport).with_limits(limits())), Instant::now())
        });
        entry.1 = Instant::now();
        let client = entry.0.clone();
        drop(clients);
        for client in idle {
            client.transport().close().await;
        }
        client
    }
}

/// The identity to ask as: rettui's own, or the one in `file` (stock
/// rngit's `client_identity`, which nodes may already know).
fn identity_for(own: &Identity, file: Option<&PathBuf>) -> Result<Identity, String> {
    match file {
        None => Ok(own.clone()),
        Some(path) => Identity::from_file(path).map_err(|e| format!("Couldn't read the rngit identity {}: {e}", path.display())),
    }
}

/// Run a request from the actor, in a task of its own; answered with
/// [`NetEvent::Git`].
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    runtime: &ReticulumHandle,
    clients: &Clients,
    own: &Identity,
    id: u64,
    repo: RepoUrl,
    request: GitRequest,
    identity_file: Option<PathBuf>,
    timeout: Duration,
    ev: &mpsc::UnboundedSender<NetEvent>,
) {
    let (runtime, clients, own, ev) = (runtime.clone(), clients.clone(), own.clone(), ev.clone());
    // On a thread of its own: rngit's calls hold a signer that can't move
    // between threads, and reading what comes back (gix) is work that
    // shouldn't take one of the runtime's.
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let result = handle.block_on(async {
            let identity = identity_for(&own, identity_file.as_ref())?;
            let client = clients.get(&runtime, &identity, repo.destination).await;
            *client.transport().timeout.lock().await = timeout;
            run(&client, &repo, &IdentitySigner(&identity), request).await
        });
        let _ = ev.send(NetEvent::Git { id, result });
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rmpv::Value;

    /// A node that answers from a table: by path and `operation`, a reply
    /// (status byte then MessagePack or text) or a file. What it was asked
    /// is kept.
    #[derive(Default)]
    pub struct Canned {
        pub replies: std::sync::Mutex<Vec<(String, String, Response)>>,
        pub asked: std::sync::Mutex<Vec<(String, Value)>>,
    }

    impl Canned {
        pub fn reply(self, path: &str, operation: &str, response: Response) -> Self {
            self.replies.lock().unwrap().push((path.into(), operation.into(), response));
            self
        }

        /// A `0x00` reply with `value` as MessagePack.
        pub fn ok(self, path: &str, operation: &str, value: Value) -> Self {
            let mut data = vec![0];
            rmpv::encode::write_value(&mut data, &value).unwrap();
            self.reply(path, operation, Response::Packet(data))
        }

        pub fn asked(&self) -> Vec<(String, Value)> {
            self.asked.lock().unwrap().clone()
        }
    }

    pub fn operation(value: &Value) -> String {
        let map = value.as_map().cloned().unwrap_or_default();
        let get = |key: &str| map.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v.clone());
        let op = get("operation").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        // A release file is told apart by its name.
        match get("artifact").and_then(|v| v.as_str().map(str::to_string)) {
            Some(name) => format!("{op}:{name}"),
            None => op,
        }
    }

    impl Transport for Canned {
        async fn request(&self, path: &str, data: Vec<u8>) -> Result<Response, rngit::Error> {
            let value = rmpv::decode::read_value(&mut &data[..]).unwrap();
            let op = operation(&value);
            self.asked.lock().unwrap().push((path.to_string(), value));
            let replies = self.replies.lock().unwrap();
            let found = replies.iter().find(|(p, o, _)| p == path && *o == op);
            found.map(|(_, _, r)| r.clone()).ok_or_else(|| rngit::Error::Transport(format!("nothing canned for {path} {op}")))
        }
    }

    pub fn map(entries: &[(&str, Value)]) -> Value {
        Value::Map(entries.iter().map(|(k, v)| (Value::from(*k), v.clone())).collect())
    }

    pub fn repo() -> RepoUrl {
        "rns://0123456789abcdef0123456789abcdef/public/demo".parse().unwrap()
    }

    pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
    }

    /// A canned node a client can own while the test keeps it too.
    pub struct Shared(pub Arc<Canned>);

    impl Transport for Shared {
        async fn request(&self, path: &str, data: Vec<u8>) -> Result<Response, rngit::Error> {
            self.0.request(path, data).await
        }
    }

    fn ask(node: Canned, request: GitRequest) -> (Result<GitReply, String>, Arc<Canned>) {
        let node = Arc::new(node);
        let client = Client::new(Shared(node.clone()));
        let identity = Identity::new();
        (block_on(run(&client, &repo(), &IdentitySigner(&identity), request)), node)
    }

    #[test]
    fn refs_are_read_from_the_list() {
        let sha = "1".repeat(40);
        let list = format!("{sha} refs/heads/main\n{sha} refs/tags/v1\n@refs/heads/main HEAD\n");
        let mut reply = vec![0];
        reply.extend_from_slice(list.as_bytes());
        let (result, node) = ask(Canned::default().reply("/git/list", "", Response::Packet(reply)), GitRequest::Refs);
        let GitReply::Refs(refs) = result.unwrap() else { panic!("not refs") };
        assert_eq!(refs.refs.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(), ["refs/heads/main", "refs/tags/v1"]);
        assert_eq!(refs.head.as_deref(), Some("refs/heads/main"));
        // Asked of the right repository, for reading.
        let asked = node.asked();
        assert_eq!(asked[0].0, "/git/list");
        assert!(asked[0].1.as_map().unwrap().iter().any(|(_, v)| v.as_str() == Some("public/demo.git")));
    }

    #[test]
    fn refusals_say_what_to_do() {
        let reply = Response::Packet([&[0x01][..], b"Not allowed"].concat());
        let (result, _) = ask(Canned::default().reply("/mgmt/perms", "rperms", reply), GitRequest::Perms);
        let error = result.unwrap_err();
        assert!(error.contains("doesn't allow you") && error.contains("Not allowed"), "{error}");
        let (result, _) = ask(Canned::default(), GitRequest::Releases);
        assert!(result.unwrap_err().contains("nothing canned"));
    }

    #[test]
    fn documents_are_cleaned_and_their_signatures_checked() {
        let author = Identity::new();
        let text = "Body\x1b[31m with\nlines";
        let signature = author.sign(text.as_bytes()).unwrap();
        let comment = map(&[
            ("id", Value::from(1)),
            ("content", Value::from("A comment\x07")),
            ("created", Value::F64(1.5)),
            ("author", Value::from("ab".repeat(16))),
        ]);
        let view = |signature: Vec<u8>| {
            map(&[
                ("id", Value::from(3)),
                ("scope", Value::from("active")),
                ("content", Value::from(text)),
                ("comments", Value::Array(vec![comment.clone(), Value::from("not a comment")])),
                (
                    "meta",
                    map(&[
                        ("title", Value::from("Fix\x1b it")),
                        ("created", Value::F64(1.0)),
                        ("edited", Value::F64(2.0)),
                        ("author", Value::from("cd".repeat(16))),
                        ("identity", Value::Binary(author.get_public_key().to_vec())),
                        ("signature", Value::Binary(signature)),
                        ("format", Value::from("markdown")),
                    ]),
                ),
            ])
        };
        let request = GitRequest::WorkView { id: 3, scope: WorkScope::Active };
        let (result, _) = ask(Canned::default().ok("/mgmt/work", "view", view(signature.to_vec())), request.clone());
        let GitReply::Doc(doc) = result.unwrap() else { panic!("not a document") };
        assert_eq!((doc.id, doc.title.as_str(), doc.text.as_str()), (3, "Fix it", "Body[31m with\nlines"));
        assert_eq!(doc.signer, Some(hex::encode(author.hash)));
        assert_eq!(doc.comments, [Comment { id: 1, author: "ab".repeat(16), created: 1.5, text: "A comment".into() }]);
        // A signature that doesn't match: no signer.
        let (result, _) = ask(Canned::default().ok("/mgmt/work", "view", view(vec![0; 64])), request);
        let GitReply::Doc(doc) = result.unwrap() else { panic!("not a document") };
        assert_eq!(doc.signer, None);
    }

    /// A release with one file, `app.txt`, signed by `signer`, and the
    /// node's answers for it (the file given as `served`).
    pub fn release_node(signer: &Identity, served: &[u8]) -> Canned {
        let data = b"the app".to_vec();
        let rsg = rngit::sign::make_rsg(&IdentitySigner(signer), &data, 10).unwrap();
        let input = rngit::sign::ManifestInput {
            name: "demo.git",
            version: "v1",
            released: "2026-01-01T00:00:00Z",
            origin: repo().destination,
            path: "public/demo.git",
            commit: "",
            message: b"notes",
            artifacts: &[("app.txt".to_string(), rsg)],
        };
        let rsm = rngit::sign::make_rsm(&IdentitySigner(signer), &input, 10).unwrap();
        let file = |data: Vec<u8>| Response::File { data, metadata: Some(vec![0x80]) };
        Canned::default().reply("/mgmt/release", "fetch:manifest.rsm", file(rsm)).reply(
            "/mgmt/release",
            "fetch:app.txt",
            file(served.to_vec()),
        )
    }

    /// A whole repository's answers: two branches and a tag, one release
    /// with a file, a document in each list (the active one signed by
    /// `author`, with a comment), and every change taken.
    pub fn demo_node(author: &Identity) -> Canned {
        let sha = "1".repeat(40);
        let list = format!("{sha} refs/heads/main\n{sha} refs/heads/dev\n{sha} refs/tags/v1\n@refs/heads/main HEAD\n");
        let summary = |id: u64, title: &str| {
            map(&[
                ("id", Value::from(id)),
                ("title", Value::from(title)),
                ("created", Value::F64(1.0)),
                ("edited", Value::F64(2.0)),
                ("author", Value::from(hex::encode(author.hash))),
                ("format", Value::from("markdown")),
                ("comments", Value::from(u64::from(id == 1))),
            ])
        };
        let text = "Make it faster";
        let view = map(&[
            ("id", Value::from(1)),
            ("scope", Value::from("active")),
            ("content", Value::from(text)),
            (
                "comments",
                Value::Array(vec![map(&[
                    ("id", Value::from(1)),
                    ("content", Value::from("On it")),
                    ("created", Value::F64(3.0)),
                    ("author", Value::from(hex::encode(author.hash))),
                ])]),
            ),
            (
                "meta",
                map(&[
                    ("title", Value::from("Speed")),
                    ("created", Value::F64(1.0)),
                    ("edited", Value::F64(2.0)),
                    ("author", Value::from(hex::encode(author.hash))),
                    ("identity", Value::Binary(author.get_public_key().to_vec())),
                    ("signature", Value::Binary(author.sign(text.as_bytes()).unwrap().to_vec())),
                    ("format", Value::from("markdown")),
                ]),
            ),
        ]);
        let release = map(&[
            ("tag", Value::from("v1")),
            ("hash", Value::from(sha.clone())),
            ("created", Value::from(1_800_000_000u64)),
            ("status", Value::from("released")),
            ("created_by", Value::from(hex::encode(author.hash))),
            ("preview", Value::from("First")),
            ("format", Value::from("markdown")),
            ("artifacts", Value::from(1)),
        ]);
        let detail = map(&[
            ("tag", Value::from("v1")),
            ("hash", Value::from(sha)),
            ("created", Value::from(1_800_000_000u64)),
            ("status", Value::from("released")),
            ("created_by", Value::from(hex::encode(author.hash))),
            ("notes", Value::from("First release")),
            ("notes_format", Value::from("markdown")),
            ("artifacts", Value::Array(vec![map(&[("name", Value::from("app.txt")), ("size", Value::from(7))])])),
        ]);
        let moved = |scope: &str| map(&[("id", Value::from(1)), ("scope", Value::from(scope))]);
        let ack = || Response::Packet(vec![0]);
        release_node(author, b"the app")
            .reply("/git/list", "", Response::Packet([&[0][..], list.as_bytes()].concat()))
            .ok("/mgmt/perms", "rperms", map(&[("content", Value::from("r:all\nw:owner"))]))
            .ok("/mgmt/release", "list", map(&[("releases", Value::Array(vec![release])), ("latest", Value::from("v1"))]))
            .ok("/mgmt/release", "view", detail)
            .reply("/mgmt/release", "create", ack())
            .reply("/mgmt/release", "delete", ack())
            .ok(
                "/mgmt/work",
                "list",
                map(&[
                    ("active", Value::Array(vec![summary(1, "Speed")])),
                    ("completed", Value::Array(vec![summary(2, "Old")])),
                    ("proposed", Value::Array(vec![summary(3, "Idea")])),
                ]),
            )
            .ok("/mgmt/work", "view", view)
            .ok("/mgmt/work", "create", moved("active"))
            .ok("/mgmt/work", "propose", map(&[("id", Value::from(4)), ("scope", Value::from("proposed"))]))
            .ok("/mgmt/work", "comment", map(&[("id", Value::from(2))]))
            .reply("/mgmt/work", "edit", ack())
            .ok("/mgmt/work", "complete", moved("completed"))
            .ok("/mgmt/work", "activate", moved("active"))
            .reply("/mgmt/work", "delete", ack())
    }

    /// Answer a request as `node` would, over the protocol (for the UIs'
    /// tests).
    pub fn answer(node: &Arc<Canned>, repo: &RepoUrl, request: GitRequest) -> Result<GitReply, String> {
        let client = Client::new(Shared(node.clone()));
        let identity = Identity::new();
        block_on(run(&client, repo, &IdentitySigner(&identity), request))
    }

    #[test]
    fn release_files_are_checked_against_the_manifest() {
        let signer = Identity::new();
        let request = || GitRequest::Artifact { tag: "v1".into(), name: "app.txt".into() };
        let (result, _) = ask(release_node(&signer, b"the app"), request());
        assert_eq!(
            result.unwrap(),
            GitReply::Artifact { name: "app.txt".into(), data: b"the app".to_vec(), signer: hex::encode(signer.hash) }
        );
        // Another file served under its name.
        let (result, _) = ask(release_node(&signer, b"something else"), request());
        assert!(result.unwrap_err().contains("doesn't match its signature"));
        // A file the manifest doesn't list.
        let (result, _) = ask(release_node(&signer, b"the app"), GitRequest::Artifact { tag: "v1".into(), name: "other".into() });
        assert!(result.unwrap_err().contains("doesn't list other"));
    }

    #[test]
    fn writes_are_signed_and_said() {
        let node = Canned::default()
            .ok("/mgmt/work", "create", map(&[("id", Value::from(7)), ("scope", Value::from("active"))]))
            .ok("/mgmt/work", "comment", map(&[("id", Value::from(1))]))
            .ok("/mgmt/work", "complete", map(&[("id", Value::from(7)), ("scope", Value::from("completed"))]));
        let client = Client::new(node);
        let identity = Identity::new();
        let signer = IdentitySigner(&identity);
        let new = GitRequest::WorkNew { kind: WorkKind::Create, title: "T".into(), text: "Body".into() };
        assert_eq!(block_on(run(&client, &repo(), &signer, new)).unwrap(), GitReply::Done("Made #7 (active)".into()));
        let comment = GitRequest::WorkComment { id: 7, scope: WorkScope::Active, text: "Hi".into() };
        assert_eq!(block_on(run(&client, &repo(), &signer, comment)).unwrap(), GitReply::Done("Commented on #7".into()));
        assert_eq!(block_on(run(&client, &repo(), &signer, GitRequest::WorkComplete(7))).unwrap(), GitReply::Done("Marked #7 done".into()));
        // The new document carried a signature of its text, by us.
        let asked = client.transport().asked();
        let created = asked[0].1.as_map().unwrap();
        let signature = created.iter().find(|(k, _)| k.as_str() == Some("signature")).and_then(|(_, v)| v.as_slice()).unwrap();
        rngit::sign::verify_signature(&identity.get_public_key(), b"Body", signature).unwrap();
    }

    #[test]
    fn text_from_nodes_is_cleaned_and_capped() {
        assert_eq!(clean_text("a\x1b[2Jb\tc\nd\r", 100), "a[2Jb\tc\nd");
        assert_eq!(clean_text("héllo", 2), "hé");
    }

    #[test]
    fn the_identity_is_ours_or_read_from_a_file() {
        let dir = std::env::temp_dir().join(format!("rettui-git-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let own = Identity::new();
        assert_eq!(identity_for(&own, None).unwrap().hash, own.hash);
        let theirs = Identity::new();
        let path = dir.join("client_identity");
        theirs.to_file(&path).unwrap();
        assert_eq!(identity_for(&own, Some(&path)).unwrap().hash, theirs.hash);
        assert!(identity_for(&own, Some(&dir.join("missing"))).err().is_some_and(|e| e.contains("Couldn't read")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
