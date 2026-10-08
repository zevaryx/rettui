//! Offline map tiles from an MBTiles file: tiles in an SQLite database, as
//! MOBAC, QGIS, TileMill and others make them, for a map with no internet
//! at all. Opened read only; only raster tiles (pictures) are shown.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::tiles::{Tile, picture_kind};

/// The most zoom levels a file's tiles are looked for at.
const MAX_ZOOM: u32 = 24;
/// How many levels closer than its own a tile is enlarged to (past the
/// file's closest, the map shows its tiles larger rather than nothing).
const ENLARGED: u32 = 6;

/// The file open now, kept open between tiles, with what it credits.
static OPEN: Mutex<Option<(PathBuf, Connection, Option<String>)>> = Mutex::new(None);

fn open(path: &Path) -> Result<Connection, String> {
    let fail = |e: rusqlite::Error| format!("{} isn't an MBTiles file rettui can read: {e}", path.display());
    if !path.is_file() {
        return Err(format!("There's no file {}", path.display()));
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(fail)?;
    // Vector tiles are drawn by the browser from data; rettui's map shows
    // pictures.
    if let Some(format) = metadata(&connection, "format")
        && !["png", "jpg", "jpeg", "webp"].contains(&format.to_lowercase().as_str())
    {
        return Err(format!(
            "{} has {format} tiles: rettui's map shows pictures (png, jpg or webp tiles), not vector tiles",
            path.display()
        ));
    }
    connection.query_row("SELECT 1 FROM tiles LIMIT 1", [], |_| Ok(())).optional().map_err(fail)?;
    Ok(connection)
}

fn metadata(connection: &Connection, name: &str) -> Option<String> {
    connection.query_row("SELECT value FROM metadata WHERE name = ?1", [name], |row| row.get(0)).ok()
}

/// Run `read` with the file at `path` open (opening it, if another or none
/// is).
fn with<T>(path: &Path, read: impl FnOnce(&Connection, &Option<String>) -> T) -> Result<T, String> {
    let mut open_now = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    if open_now.as_ref().is_none_or(|(open, ..)| open != path) {
        *open_now = None;
        let connection = open(path)?;
        let credit = metadata(&connection, "attribution").map(|credit| plain(&credit));
        *open_now = Some((path.to_path_buf(), connection, credit));
    }
    let (_, connection, credit) = open_now.as_ref().expect("opened");
    Ok(read(connection, credit))
}

/// What a file says of itself, to check it when it's chosen: its name and
/// zoom levels. It's opened only for this (a file kept open can't be
/// deleted or replaced on Windows, and one checked may not be used).
pub fn describe(path: &Path) -> Result<String, String> {
    let connection = open(path)?;
    let name = metadata(&connection, "name").filter(|n| !n.trim().is_empty());
    let zooms: Option<(u32, u32)> =
        connection.query_row("SELECT MIN(zoom_level), MAX(zoom_level) FROM tiles", [], |row| Ok((row.get(0)?, row.get(1)?))).ok();
    let name = name.unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    Ok(match zooms {
        Some((least, most)) => format!("{name}, zoom {least} to {most}"),
        None => format!("{name}, with no tiles"),
    })
}

/// Close the file kept open, if any: another is chosen, or none.
pub fn forget() {
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Whether `path` is the file kept open.
#[cfg(test)]
fn is_open(path: &Path) -> bool {
    OPEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|(open, ..)| open == path)
}

/// What the file credits for its tiles (its `attribution`, as text), if
/// anything.
pub fn credit(path: &Path) -> Option<String> {
    with(path, |_, credit| credit.clone()).ok().flatten()
}

/// Tile `z`/`x`/`y` (counted from the top, as the map does) from the file,
/// if it has it. Blocking: not for the async threads.
pub fn tile(path: &Path, z: u32, x: u32, y: u32) -> Result<Option<Tile>, String> {
    let across = 1u32.checked_shl(z).filter(|_| z <= MAX_ZOOM).ok_or("There's no such tile")?;
    if x >= across || y >= across {
        return Err("There's no such tile".into());
    }
    // MBTiles count rows from the bottom.
    let row = across - 1 - y;
    let data: Option<Vec<u8>> = with(path, |connection, _| {
        connection
            .query_row("SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3", [z, x, row], |row| {
                row.get(0)
            })
            .optional()
    })?
    .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    Ok(data.and_then(|data| picture_kind(&data).map(|kind| Tile { data, kind })))
}

/// Tile `z`/`x`/`y` made from the closest one the file has further out
/// (up to [`ENLARGED`] levels), the part of it that's this tile, enlarged:
/// for a map zoomed in past what the file goes to.
pub fn enlarged(path: &Path, z: u32, x: u32, y: u32) -> Result<Option<Tile>, String> {
    for levels in 1..=ENLARGED.min(z) {
        let Some(outer) = tile(path, z - levels, x >> levels, y >> levels)? else { continue };
        let picture = image::load_from_memory(&outer.data).map_err(|e| format!("Couldn't read a map tile: {e}"))?;
        let size = picture.width().min(picture.height());
        let part = (size >> levels).max(1);
        let mask = (1 << levels) - 1;
        let piece = picture.crop_imm((x & mask) * part, (y & mask) * part, part, part);
        let piece = piece.resize_exact(size, size, image::imageops::FilterType::CatmullRom);
        let mut data = Vec::new();
        piece
            .write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png)
            .map_err(|e| format!("Couldn't make a map tile: {e}"))?;
        return Ok(Some(Tile { data, kind: "image/png" }));
    }
    Ok(None)
}

/// Text from an attribution, which is often a little HTML (a link).
fn plain(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.replace("&copy;", "©").replace("&amp;", "&").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
pub(crate) fn sample(path: &Path, format: &str) {
    let _ = std::fs::remove_file(path);
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE metadata (name TEXT, value TEXT);
             CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);",
        )
        .unwrap();
    for (name, value) in [
        ("name", "Hilltop"),
        ("format", format),
        ("attribution", "<a href=\"https://www.openstreetmap.org/copyright\">&copy; OpenStreetMap contributors</a>"),
    ] {
        connection.execute("INSERT INTO metadata VALUES (?1, ?2)", [name, value]).unwrap();
    }
    // Zoom 1: the top left tile is row 1 from the bottom.
    connection.execute("INSERT INTO tiles VALUES (1, 0, 1, ?1)", [b"\x89PNG\r\n\x1a\ntop left".to_vec()]).unwrap();
    connection.execute("INSERT INTO tiles VALUES (2, 3, 0, ?1)", [b"\x89PNG\r\n\x1a\nbottom right".to_vec()]).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_come_from_the_file_counted_from_the_top() {
        let dir = std::env::temp_dir().join(format!("rettui-mbtiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hilltop.mbtiles");
        sample(&file, "png");
        // Checked, it's closed again (Windows can't delete a file open).
        assert_eq!(describe(&file).unwrap(), "Hilltop, zoom 1 to 2");
        assert!(!is_open(&file));
        assert_eq!(credit(&file).as_deref(), Some("© OpenStreetMap contributors"));
        assert_eq!(tile(&file, 1, 0, 0).unwrap().unwrap().data, b"\x89PNG\r\n\x1a\ntop left");
        assert_eq!(tile(&file, 2, 3, 3).unwrap().unwrap().kind, "image/png");
        // One it doesn't have, and one there isn't.
        assert_eq!(tile(&file, 1, 1, 1).unwrap(), None);
        assert!(tile(&file, 1, 2, 0).is_err() && tile(&file, 40, 0, 0).is_err());
        // Vector tiles, and what isn't one, are refused when chosen.
        let vector = dir.join("vector.mbtiles");
        sample(&vector, "pbf");
        assert!(describe(&vector).unwrap_err().contains("not vector tiles"));
        let text = dir.join("notes.txt");
        std::fs::write(&text, "not a database").unwrap();
        assert!(describe(&text).unwrap_err().contains("isn't an MBTiles file"));
        assert!(describe(&dir.join("missing.mbtiles")).unwrap_err().contains("There's no file"));
        // The first is read again once the others were tried, and closed
        // once it's forgotten. (Whether it's open in between isn't asked:
        // tests changing the setting meanwhile forget it too.)
        assert!(tile(&file, 1, 0, 0).unwrap().is_some());
        forget();
        assert!(!is_open(&file));
        // Closer than it goes: the part of one further out, enlarged.
        let real = dir.join("real.mbtiles");
        let connection = Connection::open(&real).unwrap();
        connection
            .execute_batch("CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);")
            .unwrap();
        // Zoom 0: its left half red, its right half blue.
        let mut picture = image::RgbImage::new(256, 256);
        for (px, _, pixel) in picture.enumerate_pixels_mut() {
            *pixel = if px < 128 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) };
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(picture).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        connection.execute("INSERT INTO tiles VALUES (0, 0, 0, ?1)", [png]).unwrap();
        drop(connection);
        assert_eq!(tile(&real, 2, 3, 1).unwrap(), None);
        let right = enlarged(&real, 2, 3, 1).unwrap().unwrap();
        let right = image::load_from_memory(&right.data).unwrap().to_rgb8();
        assert_eq!((right.width(), right.get_pixel(128, 128).0), (256, [0, 0, 255]));
        let left = image::load_from_memory(&enlarged(&real, 1, 0, 1).unwrap().unwrap().data).unwrap().to_rgb8();
        assert_eq!(left.get_pixel(200, 10).0, [255, 0, 0]);
        // Too far in, or none further out: none.
        assert_eq!(enlarged(&real, 7, 0, 0).unwrap(), None);
        assert_eq!(enlarged(&file, 0, 0, 0).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
