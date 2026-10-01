//! Whether the guide's entry points answer. Each is tried when the guide
//! opens (a TCP connection, closed at once), so one that's down, or
//! blocked from here, isn't offered. The tries run on threads of their
//! own, since a name lookup or a connection can take seconds.

use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long an address gets to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// Tries this recent aren't repeated when the guide opens again.
const FRESH: Duration = Duration::from_secs(60);

/// What trying an entry point found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Being tried.
    Checking,
    /// It accepted a connection, this quickly.
    Up(Duration),
    /// It didn't: why, in a word or two.
    Down(&'static str),
}

impl Reach {
    pub fn is_down(self) -> bool {
        matches!(self, Reach::Down(_))
    }

    /// For a list: "checking…", "up · 120 ms", "down · refused".
    pub fn label(self) -> String {
        match self {
            Reach::Checking => "checking…".into(),
            Reach::Up(took) => format!("up · {} ms", took.as_millis()),
            Reach::Down(why) => format!("down · {why}"),
        }
    }
}

/// The latest tries, by entry point.
#[derive(Debug, Default)]
pub struct Reachability {
    results: Arc<Mutex<Vec<Reach>>>,
    started: Option<Instant>,
}

impl Reachability {
    /// Try each of `targets`, unless they were tried a moment ago.
    pub fn check(&mut self, targets: &[(&'static str, u16)]) {
        let tried = self.results.lock().map(|results| results.len()).unwrap_or(0);
        if tried == targets.len() && self.started.is_some_and(|at| at.elapsed() < FRESH) {
            return;
        }
        // A new list: tries still running from before fill in the old one.
        let results = Arc::new(Mutex::new(vec![Reach::Checking; targets.len()]));
        self.results = Arc::clone(&results);
        self.started = Some(Instant::now());
        // Tests don't reach out to the internet (they set results instead).
        if cfg!(test) {
            return;
        }
        for (i, &(host, port)) in targets.iter().enumerate() {
            let results = Arc::clone(&results);
            let _ = std::thread::Builder::new().name("rettui-reach".into()).spawn(move || {
                let reach = probe(host, port);
                if let Ok(mut results) = results.lock() {
                    results[i] = reach;
                }
            });
        }
    }

    /// What's known of each (all "checking" before the first try).
    pub fn get(&self, count: usize) -> Vec<Reach> {
        let results = self.results.lock().map(|results| results.clone()).unwrap_or_default();
        if results.len() == count { results } else { vec![Reach::Checking; count] }
    }

    #[cfg(test)]
    pub fn set(&mut self, results: Vec<Reach>) {
        self.results = Arc::new(Mutex::new(results));
        self.started = Some(Instant::now());
    }
}

/// Connect to `host` at `port`, and hang up.
pub fn probe(host: &str, port: u16) -> Reach {
    let addresses: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(addresses) => addresses.collect(),
        Err(_) => return Reach::Down("name not found"),
    };
    // Of an address that refused, one that didn't answer, and one that
    // can't be reached from here, the first says the most.
    const WHY: [&str; 3] = ["refused", "no answer", "unreachable"];
    let mut why: Option<usize> = None;
    for address in addresses {
        let start = Instant::now();
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(_) => return Reach::Up(start.elapsed()),
            Err(e) => {
                let this = match e.kind() {
                    ErrorKind::ConnectionRefused => 0,
                    ErrorKind::TimedOut => 1,
                    _ => 2,
                };
                why = Some(why.map_or(this, |why| why.min(this)));
            }
        }
    }
    why.map_or(Reach::Down("name not found"), |why| Reach::Down(WHY[why]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_point_that_answers_is_up_and_one_that_refuses_is_down() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap().port();
        assert!(matches!(probe("127.0.0.1", open), Reach::Up(_)));
        assert!(matches!(probe("localhost", open), Reach::Up(_)), "by name, over whichever address answers");
        // A port nothing listens on any more.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(probe("127.0.0.1", closed), Reach::Down("refused"));
        assert_eq!(probe("no-such-host.invalid", 4242), Reach::Down("name not found"));
        assert_eq!(Reach::Up(Duration::from_millis(382)).label(), "up · 382 ms");
        assert_eq!(Reach::Down("refused").label(), "down · refused");
    }

    #[test]
    fn tries_are_kept_a_minute() {
        let mut reach = Reachability::default();
        assert_eq!(reach.get(2), [Reach::Checking; 2], "before the first try");
        reach.set(vec![Reach::Down("refused"), Reach::Checking]);
        // Opening the guide again soon after doesn't try again.
        reach.check(&[("a.example", 1), ("b.example", 2)]);
        assert_eq!(reach.get(2), [Reach::Down("refused"), Reach::Checking]);
        // A while later it does.
        reach.started = Some(Instant::now() - FRESH);
        reach.check(&[("a.example", 1), ("b.example", 2)]);
        assert_eq!(reach.get(2), [Reach::Checking; 2]);
    }
}
