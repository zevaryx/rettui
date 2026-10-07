//! Pictures made smaller before they're sent (the `picture_size` setting),
//! as Sideband, Columba and MeshChat do: a photo straight from a phone is
//! megabytes, which takes minutes over a radio link, if it gets there at
//! all (propagation nodes usually take 256 KB at most).

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use super::App;
use super::files::unique_path;

/// The sizes pictures can be sent at.
pub const PICTURE_SIZES: &[&str] = &["small", "medium", "large", "original"];

/// A size's longest side, in pixels, and its JPEG quality; none for the
/// original.
fn limits(size: &str) -> Option<(u32, u8)> {
    match size {
        "small" => Some((480, 50)),
        "medium" => Some((1024, 70)),
        "large" => Some((2048, 85)),
        _ => None,
    }
}

/// Whether the file at `path` is a picture that's made smaller to send at
/// `size` (if that makes it smaller). A GIF isn't: it may move.
pub fn shrinks(path: &Path, size: &str) -> bool {
    let gif = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    limits(size).is_some() && crate::lxmf::is_image_path(path) && !gif
}

/// A picture made smaller.
pub struct Shrunk {
    pub data: Vec<u8>,
    /// Its file name's extension: `jpg`, or `png` for one that's partly
    /// see-through.
    pub extension: &'static str,
}

/// `data`, a picture, made to fit `size`: its longest side at most so many
/// pixels, turned the way the camera noted, and written again as a JPEG
/// (a PNG if it's partly see-through), which leaves out its metadata, such
/// as where a photo was taken. None if it's to go as it is: at the original
/// size, a GIF (which may move), not a picture that can be read, or when
/// that isn't smaller.
pub fn shrink(data: &[u8], size: &str) -> Option<Shrunk> {
    let (longest, quality) = limits(size)?;
    let reader = ImageReader::new(Cursor::new(data)).with_guessed_format().ok()?;
    if matches!(reader.format(), None | Some(ImageFormat::Gif)) {
        return None;
    }
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut picture = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        picture.apply_orientation(orientation);
    }
    if picture.width().max(picture.height()) > longest {
        picture = picture.resize(longest, longest, FilterType::Triangle);
    }
    let see_through = picture.color().has_alpha() && picture.to_rgba8().pixels().any(|p| p.0[3] < 255);
    let mut out = Vec::new();
    let extension = if see_through {
        picture.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).ok()?;
        "png"
    } else {
        picture.to_rgb8().write_with_encoder(JpegEncoder::new_with_quality(&mut out, quality)).ok()?;
        "jpg"
    };
    (out.len() < data.len()).then_some(Shrunk { data: out, extension })
}

/// `files` with their pictures made smaller to fit `size`. A smaller one is
/// written to `uploads` (so it goes when its message is deleted), and
/// replaces one that was uploaded; a file of yours from elsewhere is left
/// as it is. Slow with big photos (a quarter of a second for one from a
/// phone), so the web UI does it away from the app.
pub fn shrink_files(files: Vec<PathBuf>, size: &str, uploads: &Path) -> Vec<PathBuf> {
    if limits(size).is_none() {
        return files;
    }
    files
        .into_iter()
        .map(|path| {
            if !shrinks(&path, size) {
                return path;
            }
            let Some(shrunk) = std::fs::read(&path).ok().and_then(|data| shrink(&data, size)) else { return path };
            let stem = path.file_stem().map_or_else(|| "picture".into(), |s| s.to_string_lossy().into_owned());
            let smaller = unique_path(uploads, &format!("{stem}.{}", shrunk.extension));
            match std::fs::create_dir_all(uploads).and_then(|()| std::fs::write(&smaller, &shrunk.data)) {
                Ok(()) => {
                    // An uploaded one is replaced, by its name if that's free now.
                    let named = uploads.join(format!("{stem}.{}", shrunk.extension));
                    if crate::store::remove_owned(&path, &[uploads.to_path_buf()])
                        && !named.exists()
                        && std::fs::rename(&smaller, &named).is_ok()
                    {
                        return named;
                    }
                    smaller
                }
                Err(e) => {
                    tracing::warn!("couldn't write a smaller {}: {e}", path.display());
                    path
                }
            }
        })
        .collect()
}

impl App {
    /// [`shrink_files`] with the `picture_size` setting.
    pub(super) fn shrink_pictures(&self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        shrink_files(files, &self.settings.picture_size, &self.paths.uploads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};

    /// A noisy picture (so it doesn't compress to nothing), as a PNG.
    fn photo(width: u32, height: u32) -> Vec<u8> {
        let picture = RgbImage::from_fn(width, height, |x, y| {
            let n = x.wrapping_mul(2_654_435_761).wrapping_add(y.wrapping_mul(40_503)) as u8;
            Rgb([n, n.wrapping_mul(3), (x + y) as u8])
        });
        let mut png = Vec::new();
        picture.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        png
    }

    #[test]
    fn pictures_shrink_to_fit_the_size() {
        let big = photo(3000, 2000);
        let shrunk = shrink(&big, "medium").unwrap();
        assert_eq!(shrunk.extension, "jpg");
        let picture = image::load_from_memory(&shrunk.data).unwrap();
        assert_eq!(picture.dimensions(), (1024, 683));
        let small = shrink(&big, "small").unwrap();
        assert!(small.data.len() < shrunk.data.len());
        assert_eq!(image::load_from_memory(&small.data).unwrap().dimensions(), (480, 320));
        // Originals go as they are, and so does what isn't a picture.
        assert!(shrink(&big, "original").is_none());
        assert!(shrink(b"not a picture", "medium").is_none());
    }

    #[test]
    fn see_through_stays_png_and_gifs_and_tiny_ones_stay() {
        let mut clear = RgbaImage::from_pixel(1500, 1500, Rgba([0, 0, 0, 0]));
        clear.put_pixel(3, 3, Rgba([255, 0, 0, 255]));
        let mut png = Vec::new();
        // Written without compressing, so it can be made smaller.
        image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::NoFilter,
        )
        .write_image(clear.as_raw(), 1500, 1500, image::ExtendedColorType::Rgba8)
        .unwrap();
        let shrunk = shrink(&png, "medium").unwrap();
        assert_eq!(shrunk.extension, "png");
        assert_eq!(image::load_from_memory(&shrunk.data).unwrap().dimensions(), (1024, 1024));

        let mut gif = Vec::new();
        DynamicImage::new_rgb8(2000, 2000).write_to(&mut Cursor::new(&mut gif), ImageFormat::Gif).unwrap();
        assert!(shrink(&gif, "small").is_none());
        // Already small: as small as it gets.
        let mut tiny = Vec::new();
        DynamicImage::new_rgb8(4, 4).write_to(&mut Cursor::new(&mut tiny), ImageFormat::Png).unwrap();
        assert!(shrink(&tiny, "small").is_none());
    }

    #[test]
    fn a_photo_turned_by_its_camera_is_sent_upright() {
        // A JPEG 40 wide and 20 high, noted as turned a quarter (EXIF
        // orientation 6): it shows 20 wide and 40 high.
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 100);
        let exif: &[u8] = &[
            b'M', b'M', 0, 42, 0, 0, 0, 8, // TIFF header, big-endian, first directory at 8
            0, 1, // one entry
            0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, // Orientation (SHORT) = 6
            0, 0, 0, 0, // no next directory
        ];
        encoder.set_exif_metadata(exif.to_vec()).unwrap();
        let noise = photo(40, 20);
        let picture = image::load_from_memory(&noise).unwrap().to_rgb8();
        encoder.encode_image(&picture).unwrap();
        let shrunk = shrink(&jpeg, "large").unwrap();
        assert_eq!(image::load_from_memory(&shrunk.data).unwrap().dimensions(), (20, 40));
    }

    #[test]
    fn sending_shrinks_an_uploaded_picture_in_place() {
        let dir = std::env::temp_dir().join(format!("rettui-shrink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        std::fs::create_dir_all(&app.paths.uploads).unwrap();
        let uploaded = app.paths.uploads.join("photo.png");
        std::fs::write(&uploaded, photo(2000, 1500)).unwrap();
        // A picture of yours from elsewhere is left where it is.
        let yours = dir.join("mine.png");
        std::fs::write(&yours, photo(2000, 1500)).unwrap();
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "text").unwrap();

        let snap = app.paths.uploads.join("snap.jpg");
        let mut jpeg = Vec::new();
        image::load_from_memory(&photo(2000, 1500))
            .unwrap()
            .write_with_encoder(JpegEncoder::new_with_quality(&mut jpeg, 100))
            .unwrap();
        std::fs::write(&snap, &jpeg).unwrap();

        let files = app.shrink_pictures(vec![uploaded.clone(), yours.clone(), notes.clone(), snap.clone()]);
        assert_eq!(files[0], app.paths.uploads.join("photo.jpg"));
        assert!(!uploaded.exists());
        // Keeping its name.
        assert_eq!(files[3], snap);
        assert!(std::fs::metadata(&snap).unwrap().len() < jpeg.len() as u64);
        assert!(files[1].starts_with(&app.paths.uploads) && files[1].extension().unwrap() == "jpg");
        assert!(yours.exists());
        assert_eq!(files[2], notes);

        app.settings.picture_size = "original".into();
        assert_eq!(app.shrink_pictures(vec![yours.clone()]), [yours]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_not_sent_keeps_the_pictures_picked() {
        let dir = std::env::temp_dir().join(format!("rettui-shrink-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        let key = "ab".repeat(16);
        app.store.conversations.insert(key.clone(), Default::default());
        app.active_conversation = Some(key.clone());
        let yours = dir.join("mine.png");
        std::fs::write(&yours, photo(2000, 1500)).unwrap();
        app.attachments = vec![yours.clone()];
        // Propagated, with no propagation node: refused.
        app.delivery_mode = crate::lxmf::DeliveryMode::Propagated;
        app.send_compose();
        assert_eq!(app.attachments, std::slice::from_ref(&yours));
        assert!(std::fs::read_dir(&app.paths.uploads).map_or(true, |mut d| d.next().is_none()), "no copy left behind");
        // Sent: it's the smaller copy that goes.
        app.delivery_mode = crate::lxmf::DeliveryMode::Direct;
        app.send_compose();
        let sent = &app.store.conversations[&key].messages.last().unwrap().attachments[0];
        assert!(sent.path.starts_with(&app.paths.uploads) && sent.name == "mine.jpg" && sent.size < std::fs::metadata(&yours).unwrap().len(), "{sent:?}");
        assert!(yours.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
