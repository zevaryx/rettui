//! NomadNet: fetching pages, files and media from nodes, rendering Micron,
//! the local cache, and the node hosted here.

pub mod cache;
mod fetch;
pub mod host;
pub mod micron;
pub mod pages;
pub mod paths;
pub mod request;
pub mod script;
pub mod store;

pub use fetch::{FetchedContent, LinkCache, fetch, fetch_once};
