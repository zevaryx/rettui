//! Page, file and media requests over (reused) NomadNet Links.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rns_runtime::prelude::*;
use tokio::sync::Mutex;

use super::{paths, request};
use crate::net::{Hash, Progress, link_options};

#[derive(Debug, Clone)]
pub struct FetchedContent {
    pub data: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
}

/// The most a page or an image may be, to be kept or shown. (A file being
/// downloaded isn't held to it.) A node can send more than it was asked for.
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// Open NomadNet Links, reused across page loads, with whether we identified.
pub type LinkCache = Arc<Mutex<HashMap<Hash, (LinkSessionHandle, bool)>>>;

/// Fetch a page, file or media item. With `identity`, the Link identifies
/// us to the node (some pages personalise content or require it).
/// `progress` hears how finding a path to it goes, when that takes a while.
pub async fn fetch(
    runtime: &ReticulumHandle,
    links: &LinkCache,
    node: Hash,
    path: &str,
    fields: &BTreeMap<String, String>,
    identity: Option<Identity>,
    progress: Progress<'_>,
) -> Result<FetchedContent, String> {
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
        match handle.request(&request.path, &request.data, Some(request::TIMEOUT)).await {
            Ok(response) if response.data.len() > MAX_PAGE_BYTES && !path.starts_with(paths::FILE_PREFIX) => {
                return Err(format!("The node sent {} MB for a page or image, more than rettui takes", response.data.len() / 1_000_000));
            }
            Ok(response) => {
                return Ok(FetchedContent { data: response.data, metadata: response.metadata });
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
    let result = fetch(runtime, &links, node, path, &BTreeMap::new(), identity, progress).await;
    for (handle, _) in links.lock().await.values() {
        handle.close().await;
    }
    result
}
