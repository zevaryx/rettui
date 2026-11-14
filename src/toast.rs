//! Windows toasts with buttons and a box to write a reply in, for the
//! terminal UI and the desktop app alike.
//!
//! A program that isn't installed can't have an identity of its own for
//! Windows' notifications, so they're shown as Windows PowerShell's (its
//! app id), as they always were. What's done with one comes back to the
//! process that showed it, on the toast's own events, for as long as it
//! runs: a click, a button, or a reply written in it.

use std::collections::HashMap;
use std::sync::Mutex;

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::UI::Notifications::{ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{HSTRING, IInspectable, Interface, h};

/// Windows PowerShell's app id.
const APP_ID: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";
/// The group of rettui's toasts, with their tags (so one is replaced, or
/// taken down, by its tag).
const GROUP: &str = "rettui";

/// A button on a toast, besides clicking the toast itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// A box to write in, and a button to send it.
    Reply,
    MarkRead,
    Accept,
    Decline,
}

/// What was done with a toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    /// Clicked (or Reply with nothing written).
    Open,
    Reply(String),
    MarkRead,
    Accept,
    Decline,
}

/// A toast to show.
#[derive(Debug, Clone)]
pub struct Toast {
    pub title: String,
    pub body: String,
    /// What it's about: a newer toast with the same tag replaces it.
    pub tag: String,
    pub buttons: Vec<Button>,
}

/// The toasts on screen or in the notification centre, kept so that the
/// events they report keep coming.
static LIVE: Mutex<Option<HashMap<String, ToastNotification>>> = Mutex::new(None);

/// Show `toast`; `activated` is told what's done with it (on another
/// thread), for as long as the process runs.
pub fn show(toast: &Toast, activated: impl Fn(Activation) + Send + Sync + 'static) -> Result<(), String> {
    // WinRT wants its thread started (an error if it already was).
    // SAFETY: plain initialisation of this thread's apartment.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let document = XmlDocument::new().map_err(error)?;
    document.LoadXml(&HSTRING::from(xml(toast))).map_err(error)?;
    let notification = ToastNotification::CreateToastNotification(&document).map_err(error)?;
    let tag = short_tag(&toast.tag);
    notification.SetTag(&HSTRING::from(tag.as_str())).map_err(error)?;
    notification.SetGroup(&HSTRING::from(GROUP)).map_err(error)?;
    notification
        .Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
            activated(activation(&args));
            Ok(())
        }))
        .map_err(error)?;
    // Gone from the screen and the centre: nothing more to report.
    let gone = tag.clone();
    notification
        .Dismissed(&TypedEventHandler::new(move |_, _| {
            forget(&gone);
            Ok(())
        }))
        .map_err(error)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID)).map_err(error)?.Show(&notification).map_err(error)?;
    if let Ok(mut live) = LIVE.lock() {
        live.get_or_insert_with(HashMap::new).insert(tag, notification);
    }
    Ok(())
}

/// Take down the toast with `tag` (what it was about was read elsewhere).
pub fn withdraw(tag: &str) {
    let tag = short_tag(tag);
    forget(&tag);
    // SAFETY: as in `show`.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupedTagWithId(&HSTRING::from(tag.as_str()), &HSTRING::from(GROUP), &HSTRING::from(APP_ID));
    }
}

fn forget(tag: &str) {
    if let Ok(mut live) = LIVE.lock()
        && let Some(live) = live.as_mut()
    {
        live.remove(tag);
    }
}

fn error(e: windows::core::Error) -> String {
    e.message()
}

/// Tags are at most 64 characters: a longer one (a room's, with a long
/// name) is shortened to a hash of it, the same way every time.
fn short_tag(tag: &str) -> String {
    if tag.chars().count() <= 64 {
        return tag.to_string();
    }
    let hash = tag.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3));
    format!("h{hash:016x}")
}

/// What an event of a toast says was done.
fn activation(args: &Option<IInspectable>) -> Activation {
    let Some(args) = args.as_ref().and_then(|args| args.cast::<ToastActivatedEventArgs>().ok()) else {
        return Activation::Open;
    };
    match args.Arguments().map(|arguments| arguments.to_string()).unwrap_or_default().as_str() {
        "reply" => match written(&args) {
            Some(text) if !text.trim().is_empty() => Activation::Reply(text),
            // Nothing written: there's the chat to write it in.
            _ => Activation::Open,
        },
        "read" => Activation::MarkRead,
        "accept" => Activation::Accept,
        "decline" => Activation::Decline,
        _ => Activation::Open,
    }
}

/// What was written in the toast's reply box.
fn written(args: &ToastActivatedEventArgs) -> Option<String> {
    let input = args.UserInput().ok()?;
    let value = input.Lookup(h!("reply")).ok()?;
    value.cast::<IPropertyValue>().ok()?.GetString().ok().map(|text| text.to_string())
}

/// The toast as XML: its text, the reply box and buttons, and the
/// instant-message sound.
fn xml(toast: &Toast) -> String {
    let mut xml = format!(
        r#"<toast launch="open"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>"#,
        escape(&toast.title),
        escape(&toast.body)
    );
    if !toast.buttons.is_empty() {
        xml.push_str("<actions>");
        if toast.buttons.contains(&Button::Reply) {
            xml.push_str(r#"<input id="reply" type="text" placeHolderContent="Write a reply"/>"#);
        }
        for button in &toast.buttons {
            xml.push_str(match button {
                // Beside the box.
                Button::Reply => r#"<action content="Reply" arguments="reply" hint-inputId="reply"/>"#,
                Button::MarkRead => r#"<action content="Mark read" arguments="read"/>"#,
                Button::Accept => r#"<action content="Accept" arguments="accept"/>"#,
                Button::Decline => r#"<action content="Decline" arguments="decline"/>"#,
            });
        }
        xml.push_str("</actions>");
    }
    xml.push_str(r#"<audio src="ms-winsoundevent:Notification.IM"/></toast>"#);
    xml
}

/// Text as XML character data and attribute text: nothing in it can be
/// markup, nor a character XML doesn't allow.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t' | '\n' | '\r' => escaped.push(' '),
            c if c.is_control() => {}
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(buttons: Vec<Button>) -> Toast {
        Toast { title: "Ann & <Bo>".into(), body: "say \"hi\"\u{7}\nnow".into(), tag: "lxmf:aa".into(), buttons }
    }

    #[test]
    fn the_toast_is_valid_xml_whatever_it_says() {
        for buttons in [vec![], vec![Button::Reply, Button::MarkRead], vec![Button::Accept, Button::Decline]] {
            let document = XmlDocument::new().unwrap();
            document.LoadXml(&HSTRING::from(xml(&toast(buttons)))).expect("valid");
        }
        let text = xml(&toast(vec![]));
        assert!(text.contains("Ann &amp; &lt;Bo&gt;") && text.contains("say &quot;hi&quot; now"), "{text}");
    }

    #[test]
    fn buttons_and_the_reply_box() {
        let text = xml(&toast(vec![Button::Reply, Button::MarkRead]));
        assert!(text.contains(r#"<input id="reply" type="text""#), "{text}");
        assert!(text.contains(r#"arguments="reply" hint-inputId="reply""#) && text.contains(r#"arguments="read""#), "{text}");
        assert!(!xml(&toast(vec![Button::Accept])).contains("<input"));
        assert!(!xml(&toast(vec![])).contains("<actions>"));
    }

    /// Shows a real toast (it appears on screen for a moment): checks that
    /// Windows has it, with its tag, and that withdrawing takes it away.
    /// `cargo test -- --ignored shows_and_withdraws`.
    #[test]
    #[ignore = "shows a toast on the screen"]
    fn shows_and_withdraws() {
        let toast = Toast {
            title: "rettui test".into(),
            body: "A toast from the tests".into(),
            tag: "test:toast".into(),
            buttons: vec![Button::Reply, Button::MarkRead],
        };
        show(&toast, |_| {}).unwrap();
        let history = ToastNotificationManager::History().unwrap();
        let has = |tag: &str| {
            history
                .GetHistoryWithId(&HSTRING::from(APP_ID))
                .unwrap()
                .into_iter()
                .any(|toast| toast.Tag().map(|t| t.to_string()).unwrap_or_default() == tag)
        };
        assert!(has("test:toast"));
        withdraw("test:toast");
        assert!(!has("test:toast"));
    }

    /// By hand: shows a toast with a reply box and buttons, and says what
    /// comes of it when it's clicked, a reply is written in it, or a button
    /// is pressed (it waits a minute). `cargo test -- --ignored --nocapture
    /// click_the_toast`
    #[test]
    #[ignore = "waits for someone to click the toast"]
    fn click_the_toast() {
        let (sent, got) = std::sync::mpsc::channel();
        let toast = Toast {
            title: "rettui test".into(),
            body: "Write a reply, press a button, or click this".into(),
            tag: "test:by-hand".into(),
            buttons: vec![Button::Reply, Button::MarkRead],
        };
        show(&toast, move |activation| {
            let _ = sent.send(activation);
        })
        .unwrap();
        let done = got.recv_timeout(std::time::Duration::from_secs(60)).expect("nothing was done with the toast in a minute");
        println!("Activated: {done:?}");
        withdraw("test:by-hand");
    }

    #[test]
    fn long_tags_are_shortened_the_same_way_every_time() {
        assert_eq!(short_tag("lxmf:aa"), "lxmf:aa");
        let long = format!("rrc:{}:{}", "ab".repeat(16), "room".repeat(30));
        assert_eq!(short_tag(&long), short_tag(&long));
        assert!(short_tag(&long).len() <= 64);
        assert_ne!(short_tag(&long), short_tag(&format!("{long}x")));
    }
}
