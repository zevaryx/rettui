//! Paper messages: LXMF messages that travel some other way than over
//! Reticulum, as an `lxm://` link or its QR code (printed, shown on a
//! screen, sent by any means). They're signed and encrypted for the
//! recipient like any message, so only they can read one, and they read it
//! in (in Sideband, NomadNet or rettui) to their conversation.
//!
//! The format is Python LXMF's: `lxm://` and base64url of the recipient's
//! address and the rest of the message, encrypted for their identity.

use lxmf_core::constants::DeliveryMethod;
use lxmf_core::message_api::{LxMessage, MessageError};
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::send::build_message;
use super::{DeliveryMode, Outgoing, Reply, inbound};
use crate::net::{Hash, Known, NetEvent, lookup};

/// A paper message to write: who to, what it says, when it was written, and
/// the message it answers.
#[derive(Debug)]
pub struct Paper {
    pub to: Hash,
    pub content: String,
    pub timestamp: f64,
    pub reply: Option<Reply>,
}

/// Write a paper message: its `lxm://` link, and its hash (what replies to
/// it name). Only the recipient's key is needed (from their announce), not
/// a path: they may well be offline.
pub async fn write(
    runtime: &ReticulumHandle,
    known: &Known,
    identity: &Identity,
    source: Hash,
    paper: Paper,
) -> Result<(String, [u8; 32]), String> {
    let Paper { to, content, timestamp, reply } = paper;
    let recipient = lookup(runtime, known, to)
        .await
        .map_err(|e| format!("{e}: a paper message needs the recipient's key, which their announce brings"))?;
    let outgoing = Outgoing { reply, ..Outgoing::text(to, content, Vec::new(), DeliveryMode::Paper, timestamp) };
    let mut message = build_message(identity, source, &outgoing, recipient.app_data.as_deref()).await?;
    message.method = DeliveryMethod::Paper;
    let hash = message.hash.ok_or("The message has no hash after signing")?;
    // A stamp, if the recipient asks for one; it can take a moment.
    let link = tokio::task::spawn_blocking(move || {
        message.get_stamp();
        message
            .to_paper_uri(|data| {
                recipient.identity.encrypt(data, None).map_err(|e| MessageError::PackFailed(e.to_string()))
            })
            .map_err(|e| match e {
                MessageError::PackFailed(e) if e.contains("maximum size") => {
                    "Too long for a paper message (about 1,800 characters fit)".to_string()
                }
                e => e.to_string(),
            })
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok((link, hash))
}

/// A paper message's link, checked: for this client (`lxmf_hash`), and
/// decrypted, as the complete message ([`inbound::parse_inbound`] reads it).
pub fn open(identity: &Identity, lxmf_hash: Hash, link: &str) -> Result<Vec<u8>, String> {
    let (to, encrypted) = LxMessage::decode_paper_uri(link.trim()).map_err(|_| "Not a paper message (an lxm:// link)")?;
    if to != lxmf_hash {
        return Err(format!("This paper message is for {}, not for you", hex::encode(to)));
    }
    let decrypted = identity
        .decrypt(&encrypted, None, false)
        .map_err(|_| "This paper message can't be decrypted with your key")?;
    let mut message = to.to_vec();
    message.extend_from_slice(&decrypted);
    Ok(message)
}

/// Read a paper message in: it arrives like one received over the network
/// (its signature checked), marked as from paper.
pub fn read(runtime: &ReticulumHandle, known: &Known, identity: &Identity, lxmf_hash: Hash, link: String, ev: &mpsc::UnboundedSender<NetEvent>) {
    let (runtime, known, identity, ev) = (runtime.clone(), known.clone(), identity.clone(), ev.clone());
    tokio::spawn(async move {
        let event = match open(&identity, lxmf_hash, &link) {
            Ok(data) => match inbound::parse_inbound(&runtime, &known, None, &data).await {
                Ok(mut message) => {
                    message.paper = true;
                    NetEvent::Message(Box::new(message))
                }
                Err(e) => NetEvent::Log(format!("Could not read the paper message: {e}")),
            },
            Err(e) => NetEvent::Log(format!("Could not read the paper message: {e}")),
        };
        let _ = ev.send(event);
    });
}

/// Pictures are made at most this wide (and high) to look for a QR code in
/// them: a photo is far bigger than one needs, and slower.
const SCAN_MAX_PIXELS: u32 = 2000;

/// The paper message in a picture of its QR code (a photo, a screenshot):
/// its link.
pub fn scan(picture: &[u8]) -> Result<String, String> {
    let picture = image::load_from_memory(picture)
        .map_err(|_| "Not a picture rettui can read (PNG, JPEG, WebP, GIF or BMP)".to_string())?;
    let picture = if picture.width().max(picture.height()) > SCAN_MAX_PIXELS {
        picture.thumbnail(SCAN_MAX_PIXELS, SCAN_MAX_PIXELS)
    } else {
        picture
    };
    let grey = picture.to_luma8();
    let (width, height) = (grey.width() as usize, grey.height() as usize);
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| grey.get_pixel(x as u32, y as u32)[0]);
    let texts: Vec<String> = prepared.detect_grids().iter().filter_map(|grid| grid.decode().ok()).map(|(_, text)| text).collect();
    match texts.iter().find(|text| text.starts_with("lxm://")) {
        Some(link) => Ok(link.clone()),
        None if texts.is_empty() => Err("No QR code found in the picture: try a sharper, closer one".into()),
        None => Err("That QR code isn't a paper message (an lxm:// link)".into()),
    }
}

/// Blank modules around a QR code, which scanners need to find it.
const QUIET_ZONE: usize = 2;

/// A QR code, for either UI: the terminal draws its modules, the web UI
/// its SVG.
pub struct Qr {
    /// Modules across (and down), without the quiet zone.
    pub width: usize,
    dark: Vec<bool>,
}

impl Qr {
    /// The QR code of `text`, at the lowest error correction (as Python LXMF
    /// makes them): the largest paper message just fits.
    pub fn new(text: &str) -> Result<Self, String> {
        let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
            .map_err(|_| "Too long for a QR code".to_string())?;
        let dark = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
        Ok(Self { width: code.width(), dark })
    }

    /// Its size with the quiet zone, in modules.
    pub fn size(&self) -> usize {
        self.width + 2 * QUIET_ZONE
    }

    /// Whether the module at `x`, `y` (counting the quiet zone) is dark.
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        let (Some(x), Some(y)) = (x.checked_sub(QUIET_ZONE), y.checked_sub(QUIET_ZONE)) else {
            return false;
        };
        x < self.width && y < self.width && self.dark[y * self.width + x]
    }

    /// As SVG, a module a unit: one path of each row's dark runs (the
    /// crate's renderer draws each module, several times the size).
    pub fn svg(&self) -> String {
        use std::fmt::Write;
        let size = self.size();
        let mut path = String::new();
        for y in 0..size {
            let mut x = 0;
            while x < size {
                if !self.is_dark(x, y) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < size && self.is_dark(x, y) {
                    x += 1;
                }
                let _ = write!(path, "M{start} {y}h{}v1H{start}z", x - start);
            }
        }
        let pixels = size * 4;
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixels}\" height=\"{pixels}\" viewBox=\"0 0 {size} {size}\" \
             shape-rendering=\"crispEdges\"><rect width=\"{size}\" height=\"{size}\" fill=\"#fff\"/>\
             <path fill=\"#000\" d=\"{path}\"/></svg>"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_paper_message_fits_a_qr_code() {
        // What a QR code holds (version 40, low error correction), which
        // is what LXMF allows a paper message.
        let link = format!("lxm://{}", "a".repeat(2953 - 6));
        let qr = Qr::new(&link).unwrap();
        assert_eq!(qr.width, 177, "version 40");
        assert!(Qr::new(&format!("{link}aaaa")).is_err());
        // The quiet zone is light, the finder pattern's corner dark.
        assert!(!qr.is_dark(0, 0) && !qr.is_dark(QUIET_ZONE - 1, QUIET_ZONE));
        assert!(qr.is_dark(QUIET_ZONE, QUIET_ZONE));
        let svg = qr.svg();
        assert!(svg.starts_with("<svg") && svg.contains("viewBox=\"0 0 181 181\""));
        assert!(svg.len() < 150_000, "{} bytes", svg.len());
    }

    /// `text`'s QR code as a PNG, `scale` pixels a module.
    fn picture(text: &str, scale: usize) -> Vec<u8> {
        let qr = Qr::new(text).unwrap();
        let side = (qr.size() * scale) as u32;
        let image = image::GrayImage::from_fn(side, side, |x, y| {
            let dark = qr.is_dark(x as usize / scale, y as usize / scale);
            image::Luma([if dark { 0 } else { 255 }])
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.into_inner()
    }

    #[test]
    fn a_picture_of_the_qr_code_reads_back() {
        let (alice, bob) = (Identity::new(), Identity::new());
        let link = paper(&alice, &bob, [2; 16], "meet at noon");
        assert_eq!(scan(&picture(&link, 4)).unwrap(), link);
        assert!(scan(&picture("https://example.com", 4)).unwrap_err().contains("isn't a paper message"));
        let mut blank = std::io::Cursor::new(Vec::new());
        image::GrayImage::from_pixel(64, 64, image::Luma([255])).write_to(&mut blank, image::ImageFormat::Png).unwrap();
        assert!(scan(blank.get_ref()).unwrap_err().starts_with("No QR code"));
        assert!(scan(b"not a picture").unwrap_err().starts_with("Not a picture"));
    }

    /// A paper message from `sender` to `recipient`, as `write` makes it
    /// (without the network for the recipient's key).
    fn paper(sender: &Identity, recipient: &Identity, to: Hash, content: &str) -> String {
        let mut message = LxMessage::new(to, [7; 16], "", content, DeliveryMethod::Paper);
        message.sign(&sender.get_signing_key().unwrap()).unwrap();
        message
            .to_paper_uri(|data| recipient.encrypt(data, None).map_err(|e| MessageError::PackFailed(e.to_string())))
            .unwrap()
    }

    #[test]
    fn only_the_recipient_opens_a_paper_message() {
        let (alice, bob, eve) = (Identity::new(), Identity::new(), Identity::new());
        let bob_address = [2; 16];
        let link = paper(&alice, &bob, bob_address, "meet at noon");
        assert!(link.starts_with("lxm://"));
        let opened = open(&bob, bob_address, &format!("  {link}\n")).unwrap();
        let message = LxMessage::unpack(&opened).unwrap();
        assert_eq!(message.content, "meet at noon");
        assert_eq!(message.source_hash, [7; 16]);
        // For someone else, or someone else's key.
        assert!(open(&eve, [3; 16], &link).unwrap_err().contains("not for you"));
        assert!(open(&eve, bob_address, &link).unwrap_err().contains("can't be decrypted"));
        assert!(open(&bob, bob_address, "https://example.com").unwrap_err().starts_with("Not a paper message"));
    }

    #[test]
    fn long_messages_dont_fit_on_paper() {
        let (alice, bob) = (Identity::new(), Identity::new());
        let mut message = LxMessage::new([2; 16], [7; 16], "", &"x".repeat(3000), DeliveryMethod::Paper);
        message.sign(&alice.get_signing_key().unwrap()).unwrap();
        let result = message.to_paper_uri(|data| bob.encrypt(data, None).map_err(|e| MessageError::PackFailed(e.to_string())));
        assert!(matches!(result, Err(MessageError::PackFailed(e)) if e.contains("maximum size")));
    }
}
