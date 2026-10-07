//! Map tiles for the web UI's map, fetched by rettui rather than the
//! browser: a browser with no internet of its own (a phone on the same
//! network) still gets them, the page loads nothing from elsewhere, and
//! they're kept in `map-tiles/` to show again when the internet is gone.
//!
//! OpenStreetMap's tile usage policy asks apps to say who they are (the
//! User-Agent below), to keep tiles at least a week (they're kept a month),
//! not to fetch in bulk (only what a map being looked at shows, a few at a
//! time) and to credit OpenStreetMap (the map does).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

/// Zoom levels OpenStreetMap's tiles go to.
pub const MAX_ZOOM: u32 = 19;
/// Tiles are fetched again after this long.
const KEEP: Duration = Duration::from_secs(30 * 24 * 3600);
/// A tile bigger than this isn't one.
const MAX_TILE_BYTES: u64 = 1_000_000;
/// The most disk tiles may take; the oldest go first over it.
const MAX_CACHE_BYTES: u64 = 200_000_000;
/// The cache is checked against its limit after this many tiles are kept.
const PRUNE_EVERY: usize = 200;
const USER_AGENT: &str = concat!("rettui/", env!("CARGO_PKG_VERSION"), " (+https://github.com/zevaryx/rettui)");

/// Tiles kept since the cache was last checked against its limit.
static KEPT: AtomicUsize = AtomicUsize::new(0);

/// A tile, and its media type.
#[derive(Debug, PartialEq, Eq)]
pub struct Tile {
    pub data: Vec<u8>,
    pub kind: &'static str,
}

/// Where tile `z`/`x`/`y` is fetched from with `template`, if there's
/// such a tile.
pub fn tile_url(template: &str, z: u32, x: u32, y: u32) -> Option<String> {
    let across = 1u32 << z.min(MAX_ZOOM);
    (z <= MAX_ZOOM && x < across && y < across)
        .then(|| template.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string()).replace("{s}", "a"))
}

/// Where it's kept: apart for each place tiles come from, so changing it
/// shows the new ones.
fn kept_path(dir: &Path, template: &str, z: u32, x: u32, y: u32) -> PathBuf {
    use sha2::Digest;
    let source = hex::encode(&sha2::Sha256::digest(template.as_bytes())[..6]);
    dir.join(source).join(z.to_string()).join(x.to_string()).join(y.to_string())
}

/// What kind of picture `data` is, from how it starts: PNG, JPEG or WebP.
pub(super) fn picture_kind(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if data.len() > 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// A kept tile, and whether it's still fresh.
fn kept(path: &Path) -> Option<(Tile, bool)> {
    let data = std::fs::read(path).ok()?;
    let kind = picture_kind(&data)?;
    let age = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
    Some((Tile { data, kind }, age.is_some_and(|a| a < KEEP)))
}

/// Tile `z`/`x`/`y` as kept, if it's fresh (nothing to wait for).
pub fn fresh(dir: &Path, template: &str, z: u32, x: u32, y: u32) -> Option<Tile> {
    tile_url(template, z, x, y)?;
    kept(&kept_path(dir, template, z, x, y)).and_then(|(tile, fresh)| fresh.then_some(tile))
}

/// Tile `z`/`x`/`y`: as kept, if it's fresh; else fetched and kept (or
/// as kept, if it can't be fetched). Blocking: not for the async threads.
pub fn tile(dir: &Path, template: &str, z: u32, x: u32, y: u32) -> Result<Tile, String> {
    let url = tile_url(template, z, x, y).ok_or("There's no such tile")?;
    let path = kept_path(dir, template, z, x, y);
    let old = kept(&path);
    if let Some((tile, true)) = old {
        return Ok(tile);
    }
    match fetch(&url) {
        Ok(tile) => {
            let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, &tile.data));
            if let Err(e) = saved {
                tracing::debug!("couldn't keep a map tile: {e}");
            } else if KEPT.fetch_add(1, Ordering::Relaxed) + 1 >= PRUNE_EVERY {
                KEPT.store(0, Ordering::Relaxed);
                prune(dir, MAX_CACHE_BYTES);
            }
            Ok(tile)
        }
        Err(e) => old.map(|(tile, _)| tile).ok_or(e),
    }
}

fn fetch(url: &str) -> Result<Tile, String> {
    use std::io::Read;
    let agent = crate::https::agent(Duration::from_secs(15)).user_agent(USER_AGENT).build();
    let response = agent.get(url).call().map_err(|e| format!("Couldn't fetch the map tile: {e}"))?;
    let mut data = Vec::new();
    response.into_reader().take(MAX_TILE_BYTES + 1).read_to_end(&mut data).map_err(|e| format!("Couldn't fetch the map tile: {e}"))?;
    if data.len() as u64 > MAX_TILE_BYTES {
        return Err("The map tile was too big".into());
    }
    // Only pictures: the address is a setting, and this isn't a way to
    // fetch anything else.
    let kind = picture_kind(&data).ok_or("What came back wasn't a picture")?;
    Ok(Tile { data, kind })
}

/// Keep the tiles within `limit` bytes, the oldest going first, down to
/// four fifths of it (so it isn't done again at once).
fn prune(dir: &Path, limit: u64) {
    fn walk(dir: &Path, files: &mut Vec<(SystemTime, u64, PathBuf)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => walk(&path, files),
                Ok(meta) => files.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), path)),
                Err(_) => {}
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    if total <= limit {
        return;
    }
    files.sort();
    for (_, size, path) in files {
        if total <= limit / 5 * 4 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest of it";

    #[test]
    fn only_tiles_there_are() {
        let osm = crate::config::OSM_TILES;
        assert_eq!(tile_url(osm, 2, 3, 1).as_deref(), Some("https://tile.openstreetmap.org/2/3/1.png"));
        assert_eq!(tile_url("https://{s}.tiles.example/{z}/{y}/{x}.jpg", 1, 0, 1).as_deref(), Some("https://a.tiles.example/1/1/0.jpg"));
        assert_eq!(tile_url(osm, 2, 4, 0), None);
        assert_eq!(tile_url(osm, 2, 0, 4), None);
        assert_eq!(tile_url(osm, 20, 0, 0), None);
        assert_eq!(tile_url(osm, 0, 0, 0).as_deref(), Some("https://tile.openstreetmap.org/0/0/0.png"));
    }

    #[test]
    fn pictures_only_kept_and_pruned_oldest_first() {
        assert_eq!(picture_kind(PNG), Some("image/png"));
        assert_eq!(picture_kind(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(picture_kind(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(picture_kind(b"<html>not a tile</html>"), None);

        let dir = std::env::temp_dir().join(format!("rettui-tiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A kept tile is shown without fetching (the address goes nowhere).
        let template = "http://127.0.0.1:9/{z}/{x}/{y}.png";
        let path = kept_path(&dir, template, 3, 2, 1);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, PNG).unwrap();
        assert_eq!(tile(&dir, template, 3, 2, 1), Ok(Tile { data: PNG.to_vec(), kind: "image/png" }));
        assert_eq!(fresh(&dir, template, 3, 2, 1), Some(Tile { data: PNG.to_vec(), kind: "image/png" }));
        assert_eq!(fresh(&dir, template, 3, 2, 2), None);
        // One that isn't kept, and can't be fetched: why not.
        assert!(tile(&dir, template, 3, 2, 2).unwrap_err().starts_with("Couldn't fetch"));
        // Another source keeps its own.
        assert_ne!(kept_path(&dir, crate::config::OSM_TILES, 3, 2, 1), path);

        // Over the limit: the oldest go, down to four fifths of it.
        for (i, age) in [(0u32, 300u64), (1, 200), (2, 100)] {
            let file = kept_path(&dir, template, 5, i, 0);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, vec![0u8; 1000]).unwrap();
            let when = SystemTime::now() - Duration::from_secs(age);
            std::fs::File::options().write(true).open(&file).unwrap().set_modified(when).unwrap();
        }
        prune(&dir, 2500);
        assert!(!kept_path(&dir, template, 5, 0, 0).exists() && !kept_path(&dir, template, 5, 1, 0).exists());
        assert!(kept_path(&dir, template, 5, 2, 0).exists() && path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
