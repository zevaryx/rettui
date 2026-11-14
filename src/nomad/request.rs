//! What goes over a NomadNet Link: a request's path and data, and the name
//! a file comes back with.
//!
//! A page's request carries the form's values as a msgpack map of
//! `field_<name>` and `var_<name>` strings (nothing when there are none). A
//! picture's request (`/media`) carries `{"path": <name>, "key": nil}`. A
//! file or picture comes back as a Resource whose metadata is
//! `{"name": <name>}`.

use std::collections::BTreeMap;
use std::time::Duration;

use rmpv::Value;

use super::paths::{self, FILE_PREFIX, MEDIA_PATH, PAGE_PREFIX, PathError};

/// How long a request may take: finding the path, the Link, and the answer.
pub const TIMEOUT: Duration = Duration::from_secs(45);

/// The most request data read, and the most values taken from it.
const MAX_DATA_BYTES: usize = 64 * 1024;
const MAX_VALUES: usize = 64;
const MAX_KEY_BYTES: usize = 256;
const MAX_VALUE_BYTES: usize = 4 * 1024;
const MAX_DEPTH: usize = 8;

/// A request ready to send on a Link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub path: String,
    pub data: Vec<u8>,
}

/// A path with `prefix`, however it was written (`index.mu`, `/index.mu`,
/// `/page/index.mu`), and checked.
fn with_prefix(path: &str, prefix: &str) -> Result<String, PathError> {
    let path = path.trim();
    let path = if path.starts_with(prefix) { path.to_string() } else { format!("{prefix}{}", path.trim_start_matches('/')) };
    paths::check_name(&path[prefix.len()..])?;
    Ok(path)
}

/// The request for a page, with a form's values.
pub fn page(path: &str, fields: &BTreeMap<String, String>) -> Result<Request, PathError> {
    let path = match path.trim() {
        "" | "/" | "/page" => paths::INDEX_PATH.to_string(),
        path => with_prefix(path, PAGE_PREFIX)?,
    };
    Ok(Request { path, data: encode_fields(fields) })
}

/// The request for a file.
pub fn file(path: &str) -> Result<Request, PathError> {
    Ok(Request { path: with_prefix(path, FILE_PREFIX)?, data: Vec::new() })
}

/// The request for one of a page's pictures, by its name under `pages/`.
pub fn media(name: &str) -> Request {
    let map = Value::Map(vec![(Value::from("path"), Value::from(name)), (Value::from("key"), Value::Nil)]);
    Request { path: MEDIA_PATH.to_string(), data: encoded(&map) }
}

fn encoded(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    // Writing to a Vec can't fail.
    let _ = rmpv::encode::write_value(&mut out, value);
    out
}

/// A form's values as request data: nothing when there are none.
pub fn encode_fields(fields: &BTreeMap<String, String>) -> Vec<u8> {
    let kept: Vec<(Value, Value)> = fields
        .iter()
        .filter(|(key, value)| key.len() <= MAX_KEY_BYTES && value.len() <= MAX_VALUE_BYTES)
        .take(MAX_VALUES)
        .map(|(key, value)| (Value::from(key.as_str()), Value::from(value.as_str())))
        .collect();
    if kept.is_empty() { Vec::new() } else { encoded(&Value::Map(kept)) }
}

fn decoded(data: &[u8]) -> Option<Vec<(Value, Value)>> {
    if data.is_empty() || data.len() > MAX_DATA_BYTES {
        return None;
    }
    match rmpv::decode::read_value_with_max_depth(&mut &*data, MAX_DEPTH) {
        Ok(Value::Map(map)) => Some(map),
        _ => None,
    }
}

/// A key or value as text: clients send strings, some send bytes or numbers.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => s.as_str().map(str::to_owned),
        Value::Binary(b) => String::from_utf8(b.clone()).ok(),
        Value::Boolean(b) => Some(b.to_string()),
        Value::Integer(i) => Some(i.to_string()),
        Value::F64(f) => Some(f.to_string()),
        _ => None,
    }
}

/// The values a request carries. Anything that isn't a map of text is no
/// values at all: it comes from the network, so it's never an error.
pub fn decode_fields(data: &[u8]) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for (key, value) in decoded(data).unwrap_or_default().into_iter().take(MAX_VALUES) {
        if let (Some(key), Some(value)) = (text(&key), text(&value))
            && key.len() <= MAX_KEY_BYTES
            && value.len() <= MAX_VALUE_BYTES
        {
            fields.insert(key, value);
        }
    }
    fields
}

/// The name a `/media` request asks for: it must have a `path` string and
/// a `key` (which may be nil), as NomadNet sends them.
pub fn decode_media(data: &[u8]) -> Option<String> {
    let map = decoded(data)?;
    let named = |name: &str| map.iter().find(|(key, _)| key.as_str() == Some(name)).map(|(_, value)| value);
    named("key")?;
    named("path")?.as_str().map(str::to_owned)
}

/// The name in the metadata a file or picture came with.
pub fn reply_file_name(metadata: Option<&[u8]>) -> Option<String> {
    let mut data = metadata.filter(|data| !data.is_empty() && data.len() <= MAX_DATA_BYTES)?;
    let Ok(Value::Map(map)) = rmpv::decode::read_value_with_max_depth(&mut data, MAX_DEPTH) else { return None };
    let (_, name) = map.into_iter().find(|(key, _)| key.as_str() == Some("name"))?;
    match name {
        Value::String(_) | Value::Binary(_) => text(&name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn requests_for_pages_files_and_pictures() {
        let none = BTreeMap::new();
        for path in ["", "/", "/page", "index.mu", "/index.mu", "/page/index.mu", " /page/index.mu "] {
            assert_eq!(page(path, &none).unwrap(), Request { path: "/page/index.mu".into(), data: Vec::new() }, "{path}");
        }
        assert_eq!(page("blog/one.mu", &none).unwrap().path, "/page/blog/one.mu");
        assert!(page("/page/../secret", &none).is_err() && page("/page/x.allowed", &none).is_err());
        assert_eq!(file("/file/manual.pdf").unwrap(), Request { path: "/file/manual.pdf".into(), data: Vec::new() });
        assert_eq!(file("manual.pdf").unwrap().path, "/file/manual.pdf");
        assert!(file("/file/").is_err() && file("").is_err());

        // A form's values go as a map of strings, as NomadNet sends them.
        let form = fields(&[("field_q", "hello"), ("var_mode", "search")]);
        let sent = page("/page/find.mu", &form).unwrap();
        assert_eq!(sent.data[0], 0x82);
        assert!(sent.data.windows(8).any(|w| w == b"\xa7field_q"));
        assert_eq!(decode_fields(&sent.data), form);

        // {"path": "images/logo.png", "key": nil}
        let picture = media("images/logo.png");
        assert_eq!(picture.path, "/media");
        assert_eq!(picture.data, b"\x82\xa4path\xafimages/logo.png\xa3key\xc0");
        assert_eq!(decode_media(&picture.data).as_deref(), Some("images/logo.png"));
    }

    #[test]
    fn values_from_the_network_are_read_with_care() {
        assert!(decode_fields(b"").is_empty());
        assert!(decode_fields(b"\xc1\xc1").is_empty());
        assert!(decode_fields(&encoded(&Value::from("text"))).is_empty());
        assert!(decode_fields(&vec![0x91; 100_000]).is_empty());
        // Bytes and numbers are taken as text; anything else is left out.
        let odd = Value::Map(vec![
            (Value::from("field_a"), Value::Binary(b"bytes".to_vec())),
            (Value::from("field_n"), Value::from(7)),
            (Value::from("field_yes"), Value::Boolean(true)),
            (Value::from("field_list"), Value::Array(vec![])),
            (Value::Nil, Value::from("x")),
            (Value::from("k".repeat(MAX_KEY_BYTES + 1)), Value::from("x")),
            (Value::from("field_big"), Value::from("v".repeat(MAX_VALUE_BYTES + 1))),
        ]);
        assert_eq!(decode_fields(&encoded(&odd)), fields(&[("field_a", "bytes"), ("field_n", "7"), ("field_yes", "true")]));
        // What's too long isn't sent either.
        let long = fields(&[("ok", "v"), ("field_big", &"v".repeat(MAX_VALUE_BYTES + 1))]);
        assert_eq!(decode_fields(&encode_fields(&long)), fields(&[("ok", "v")]));
        let many: BTreeMap<String, String> = (0..100).map(|n| (format!("field_{n:03}"), "v".to_string())).collect();
        assert_eq!(decode_fields(&encode_fields(&many)).len(), MAX_VALUES);
        // Nested too deep: nothing, and no panic.
        let mut deep = Value::Nil;
        for _ in 0..50 {
            deep = Value::Map(vec![(Value::from("a"), deep)]);
        }
        assert!(decode_fields(&encoded(&deep)).is_empty());
    }

    #[test]
    fn a_picture_request_needs_its_path_and_key() {
        let request = |pairs: Vec<(&str, Value)>| encoded(&Value::Map(pairs.into_iter().map(|(k, v)| (Value::from(k), v)).collect()));
        assert_eq!(decode_media(&request(vec![("path", Value::from("a.png")), ("key", Value::from("x"))])).as_deref(), Some("a.png"));
        assert_eq!(decode_media(&request(vec![("path", Value::from("a.png"))])), None);
        assert_eq!(decode_media(&request(vec![("key", Value::Nil)])), None);
        assert_eq!(decode_media(&request(vec![("path", Value::Binary(b"a.png".to_vec())), ("key", Value::Nil)])), None);
        assert_eq!(decode_media(b""), None);
        assert_eq!(decode_media(&encoded(&Value::from("a.png"))), None);
    }

    #[test]
    fn the_name_a_file_came_with() {
        let metadata = |name: Value| encoded(&Value::Map(vec![(Value::from("other"), Value::from(1)), (Value::from("name"), name)]));
        assert_eq!(reply_file_name(Some(&metadata(Value::Binary(b"manual.pdf".to_vec())))).as_deref(), Some("manual.pdf"));
        assert_eq!(reply_file_name(Some(&metadata(Value::from("notes.txt")))).as_deref(), Some("notes.txt"));
        assert_eq!(reply_file_name(Some(&metadata(Value::from(5)))), None);
        assert_eq!(reply_file_name(Some(&metadata(Value::Binary(vec![0xff, 0xfe])))), None);
        assert_eq!(reply_file_name(Some(b"")), None);
        assert_eq!(reply_file_name(Some(b"\xc1")), None);
        assert_eq!(reply_file_name(None), None);
        // As rsReticulum packs it for a file reply.
        let packed = rns_runtime::link_manager::pack_file_name_metadata("a/b.bin");
        assert_eq!(reply_file_name(Some(&packed)).as_deref(), Some("a/b.bin"));
    }
}
