//! Printing a paper message's QR code from the terminal UI (`p` on it), as
//! Sideband prints them: a PNG of the code is handed to the `print_command`
//! setting's program, or else to `lp` (CUPS) or `lpr`. The web UI prints
//! with the browser instead, so it never runs a program.

use std::path::Path;
use std::process::{Command, Stdio};

use super::App;
use super::recorder::words;
use crate::lxmf::paper::Qr;

/// Pixels a module: a code of the largest message (177 modules) is about
/// 15 cm across at 300 dpi.
const SCALE: u32 = 6;

/// The printers tried, in order, when the setting is empty.
fn printers() -> Vec<Vec<String>> {
    if cfg!(windows) { Vec::new() } else { vec![vec!["lp".into()], vec!["lpr".into()]] }
}

/// A QR code as a PNG, black on white, with its quiet zone.
pub fn png(qr: &Qr) -> Result<Vec<u8>, String> {
    let size = qr.size() as u32;
    let image = image::GrayImage::from_fn(size * SCALE, size * SCALE, |x, y| {
        image::Luma([if qr.is_dark((x / SCALE) as usize, (y / SCALE) as usize) { 0 } else { 255 }])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(bytes.into_inner())
}

/// Hand `file` to the printer: `command` (the setting, with `{}` where the
/// file goes, else at the end), or the first printer found. What ran.
pub fn print(command: Option<&str>, file: &Path) -> Result<String, String> {
    let custom = command.map(words).filter(|w| !w.is_empty());
    let list = match &custom {
        Some(words) => vec![words.clone()],
        None => printers(),
    };
    let file = file.display().to_string();
    for mut args in list {
        match args.iter_mut().find(|a| a.contains("{}")) {
            Some(arg) => *arg = arg.replace("{}", &file),
            None => args.push(file.clone()),
        }
        let started = Command::new(&args[0]).args(&args[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        match started {
            Ok(_) => return Ok(args[0].clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && custom.is_none() => continue,
            Err(e) => return Err(format!("Couldn't run {}: {e}", args[0])),
        }
    }
    Err(if cfg!(windows) {
        "Set Print command in Settings (Messages) to the program that prints a picture".into()
    } else {
        "No printer program found: install CUPS (lp), or set Print command in Settings (Messages)".into()
    })
}

impl App {
    /// Print the paper message's QR code shown (`p` on it).
    pub(super) fn print_paper_qr(&mut self) {
        let Some(view) = &self.paper_view else { return };
        let png = match view.qr.as_ref().map_err(String::clone).and_then(png) {
            Ok(png) => png,
            Err(e) => return self.warn(e),
        };
        let file = self.paths.root.join("print").join("qr-code.png");
        let written = std::fs::create_dir_all(file.parent().unwrap_or(&self.paths.root))
            .map_err(anyhow::Error::from)
            .and_then(|()| crate::config::write_atomic(&file, &png));
        if let Err(e) = written {
            return self.fail(format!("Could not make the picture to print: {e}"));
        }
        match print(self.settings.print_command.as_deref(), &file) {
            Ok(program) => self.notify(format!("Sent the QR code to the printer with {program}")),
            Err(e) => self.fail(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qr_code_as_a_picture_to_print() {
        let qr = Qr::new("lxm://hello").unwrap();
        let png = png(&qr).unwrap();
        let picture = image::load_from_memory(&png).unwrap().to_luma8();
        let size = qr.size() as u32 * SCALE;
        assert_eq!(picture.dimensions(), (size, size));
        // White around it, and the finder pattern's dark corner where it
        // starts.
        assert_eq!(picture.get_pixel(0, 0).0, [255]);
        let corner = 2 * SCALE;
        assert_eq!(picture.get_pixel(corner, corner).0, [0]);
    }

    #[test]
    fn printed_with_the_program_set() {
        let dir = std::env::temp_dir().join(format!("rettui-print-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("qr code.png");
        std::fs::write(&file, b"png").unwrap();
        let missing = print(Some("no-such-printer-program -d office"), &file).unwrap_err();
        assert!(missing.contains("Couldn't run no-such-printer-program"), "{missing}");
        // One that runs: the file goes where {} is, or at the end.
        let ok = if cfg!(windows) { "cmd /c rem {}" } else { "true {}" };
        assert_eq!(print(Some(ok), &file).unwrap(), if cfg!(windows) { "cmd" } else { "true" });
        let _ = std::fs::remove_dir_all(dir);
    }
}
