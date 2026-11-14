//! Windows: toasts with a box to write a reply in and buttons (Mark read,
//! Accept, Decline), made by rettui itself (`rettui::toast`, the same code
//! the terminal UI uses). A click opens what it's about, and the window
//! comes forward.

use rettui::toast::{self, Activation, Button, Toast};

use super::{Action, Backend, Event, Request, Sink};

pub struct Notifications {
    sink: Sink,
}

impl Notifications {
    pub fn start(sink: Sink) -> Self {
        Self { sink }
    }
}

impl Backend for Notifications {
    fn show(&self, request: Request) {
        let sink = self.sink.clone();
        // Off the caller's thread: talking to Windows can take a moment, and
        // WinRT wants a thread started for it.
        let _ = std::thread::Builder::new().name("rettui-toast".into()).spawn(move || {
            let buttons = request
                .actions
                .iter()
                .filter_map(|action| match action {
                    Action::Reply => Some(Button::Reply),
                    Action::Read => Some(Button::MarkRead),
                    Action::Accept => Some(Button::Accept),
                    Action::Decline => Some(Button::Decline),
                    // Always there: clicking the toast.
                    Action::Open => None,
                })
                .collect();
            let toast = Toast { title: request.title, body: request.body, tag: request.tag.clone(), buttons };
            let (tag, target) = (request.tag, request.target);
            let shown = toast::show(&toast, move |activation| {
                let (action, text) = match activation {
                    Activation::Open => (Action::Open, None),
                    Activation::Reply(text) => (Action::Reply, Some(text)),
                    Activation::MarkRead => (Action::Read, None),
                    Activation::Accept => (Action::Accept, None),
                    Activation::Decline => (Action::Decline, None),
                };
                sink(Event { action, tag: tag.clone(), target: target.clone(), text });
            });
            if let Err(e) = shown {
                eprintln!("Couldn't show a notification: {e}");
            }
        });
    }

    fn close(&self, tag: &str) {
        let tag = tag.to_string();
        let _ = std::thread::Builder::new().name("rettui-toast".into()).spawn(move || toast::withdraw(&tag));
    }
}
