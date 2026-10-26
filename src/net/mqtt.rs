//! Telemetry passed on to an MQTT server, as Sideband does (*Telemetry to
//! MQTT*): each place heard, and this station's, published under
//! `lxmf/telemetry/<LXMF address>/<sensor>/<value>` with the values as
//! text, so home automation and dashboards that read Sideband's can read
//! rettui's. A connection for each batch (MQTT 3.1.1, QoS 1), closed
//! once the server has taken it, as Sideband connects.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::lxmf::Location;

/// The server's port when none is given.
const PORT: u16 = 1883;
/// Longest to wait for the server at each step.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Most topics in one batch.
const MAX_TOPICS: usize = 64;

/// Where to publish: `mqtt://[user[:password]@]host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<String>,
}

impl Server {
    pub fn parse(text: &str) -> Result<Server, String> {
        let wrong = || "an MQTT server as mqtt://host, mqtt://host:1883 or mqtt://user:password@host".to_string();
        let rest = text.trim().strip_prefix("mqtt://").unwrap_or(text.trim());
        let (login, place) = match rest.rsplit_once('@') {
            Some((login, place)) => (Some(login), place),
            None => (None, rest),
        };
        let (user, password) = match login.map(|l| l.split_once(':').map_or((l, None), |(u, p)| (u, Some(p)))) {
            Some((user, password)) => (Some(user.to_string()).filter(|u| !u.is_empty()), password.map(str::to_string)),
            None => (None, None),
        };
        // An IPv6 address is in brackets: [::1]:1883.
        let (host, port) = match place.strip_prefix('[').and_then(|p| p.split_once(']')) {
            Some((host, "")) => (host, None),
            Some((host, port)) => (host, Some(port.strip_prefix(':').ok_or_else(wrong)?)),
            None => match place.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (place, None),
            },
        };
        let port = port.map_or(Ok(PORT), |p| p.parse().map_err(|_| wrong()))?;
        if host.is_empty() || host.contains(['/', ' ']) || port == 0 {
            return Err(wrong());
        }
        Ok(Server { host: host.to_string(), port, user, password })
    }
}

/// A number as Python writes it (Sideband's values are `str()` of them).
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{value:.1}") } else { value.to_string() }
}

/// The topics for a place `source` was at `at` (Unix seconds), with their
/// values, as Sideband publishes its time and location sensors. A value it
/// doesn't have is empty.
pub fn topics(source: &str, location: &Location, at: f64) -> Vec<(String, String)> {
    let root = format!("lxmf/telemetry/{source}");
    let optional = |value: Option<f64>| value.map(number).unwrap_or_default();
    let mut topics = vec![
        (format!("{root}/time/name"), "Timestamp".to_string()),
        (format!("{root}/time/icon"), "clock-time-ten-outline".to_string()),
        (format!("{root}/time/utc"), (at as i64).to_string()),
        (format!("{root}/location/name"), "Location".to_string()),
        (format!("{root}/location/icon"), "map-marker".to_string()),
        (format!("{root}/location/latitude"), number(location.latitude)),
        (format!("{root}/location/longitude"), number(location.longitude)),
    ];
    let more = [
        ("altitude", location.altitude),
        ("speed", location.speed),
        ("heading", location.bearing),
        ("accuracy", location.accuracy),
        ("updated", location.updated.map(|u| u as f64)),
    ];
    topics.extend(more.into_iter().map(|(name, value)| (format!("{root}/location/{name}"), optional(value))));
    topics
}

/// MQTT's variable length: 7 bits a byte, the high bit saying more follow.
fn remaining_length(mut length: usize, out: &mut Vec<u8>) {
    loop {
        let byte = (length % 128) as u8;
        length /= 128;
        out.push(if length > 0 { byte | 0x80 } else { byte });
        if length == 0 {
            break;
        }
    }
}

fn string(text: &[u8], out: &mut Vec<u8>) {
    out.extend((text.len().min(u16::MAX as usize) as u16).to_be_bytes());
    out.extend(&text[..text.len().min(u16::MAX as usize)]);
}

fn packet(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    remaining_length(body.len(), &mut out);
    out.extend(body);
    out
}

fn connect(server: &Server, client: &str) -> Vec<u8> {
    let mut body = Vec::new();
    string(b"MQTT", &mut body);
    body.push(4); // 3.1.1
    let flags = 0x02 | if server.user.is_some() { 0x80 } else { 0 } | if server.password.is_some() { 0x40 } else { 0 };
    body.push(flags);
    body.extend(60u16.to_be_bytes());
    string(client.as_bytes(), &mut body);
    if let Some(user) = &server.user {
        string(user.as_bytes(), &mut body);
    }
    if let Some(password) = &server.password {
        string(password.as_bytes(), &mut body);
    }
    packet(0x10, &body)
}

fn publish(id: u16, topic: &str, value: &str) -> Vec<u8> {
    let mut body = Vec::new();
    string(topic.as_bytes(), &mut body);
    body.extend(id.to_be_bytes());
    body.extend(value.as_bytes());
    packet(0x32, &body)
}

/// One packet from the server: its type byte and body (small: CONNACK and
/// PUBACK are 2 bytes).
async fn read_packet(stream: &mut TcpStream) -> Result<(u8, Vec<u8>), String> {
    let gone = |e: std::io::Error| format!("the server closed the connection ({e})");
    let kind = stream.read_u8().await.map_err(gone)?;
    let mut length = 0usize;
    for shift in 0..4 {
        let byte = stream.read_u8().await.map_err(gone)?;
        length |= ((byte & 0x7f) as usize) << (7 * shift);
        if byte & 0x80 == 0 {
            break;
        }
    }
    if length > 1024 {
        return Err("the server sent something that isn't MQTT".into());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.map_err(gone)?;
    Ok((kind, body))
}

async fn send(server: &Server, client: &str, topics: &[(String, String)]) -> Result<(), String> {
    let address = (server.host.as_str(), server.port);
    let mut stream = TcpStream::connect(address).await.map_err(|e| format!("couldn't connect to {}:{}: {e}", server.host, server.port))?;
    let wrote = |e: std::io::Error| format!("the connection broke ({e})");
    stream.write_all(&connect(server, client)).await.map_err(wrote)?;
    match read_packet(&mut stream).await? {
        (0x20, body) if body.get(1) == Some(&0) => {}
        (0x20, body) => {
            return Err(match body.get(1) {
                Some(4) | Some(5) => "the server refused the user name or password".into(),
                code => format!("the server refused the connection (code {})", code.copied().unwrap_or(0)),
            });
        }
        _ => return Err("the server didn't answer as MQTT does".into()),
    }
    let topics = &topics[..topics.len().min(MAX_TOPICS)];
    for (i, (topic, value)) in topics.iter().enumerate() {
        stream.write_all(&publish(i as u16 + 1, topic, value)).await.map_err(wrote)?;
    }
    let mut taken = 0;
    while taken < topics.len() {
        match read_packet(&mut stream).await? {
            (0x40, _) => taken += 1,
            _ => return Err("the server didn't take what was sent".into()),
        }
    }
    let _ = stream.write_all(&[0xe0, 0]).await;
    Ok(())
}

/// Publish `topics` to `server`, waiting until it has taken them all.
pub async fn send_topics(server: &Server, topics: &[(String, String)]) -> Result<(), String> {
    let client = format!("rettui-{:08x}", rand::random::<u32>());
    match tokio::time::timeout(TIMEOUT, send(server, &client, topics)).await {
        Ok(result) => result,
        Err(_) => Err(format!("{}:{} didn't answer in time", server.host, server.port)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn servers_as_written() {
        let plain = Server::parse("mqtt://broker.local").unwrap();
        assert_eq!(plain, Server { host: "broker.local".into(), port: 1883, user: None, password: None });
        let full = Server::parse("mqtt://ann:s3cr:t@10.0.0.2:8883").unwrap();
        assert_eq!(full, Server { host: "10.0.0.2".into(), port: 8883, user: Some("ann".into()), password: Some("s3cr:t".into()) });
        assert_eq!(Server::parse("[::1]:1884").unwrap(), Server { host: "::1".into(), port: 1884, user: None, password: None });
        assert_eq!(Server::parse("[::1]").unwrap().port, 1883);
        assert_eq!(Server::parse("bob@host").unwrap().user.as_deref(), Some("bob"));
        for wrong in ["", "mqtt://", "host:port", "host:0", "http://x/y", "a b", "[::1]x"] {
            assert!(Server::parse(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn topics_as_sideband_publishes_them() {
        let place = Location { altitude: Some(12.0), accuracy: Some(4.5), ..Location::at(51.5074, -0.1278) };
        let topics = topics("ab12", &place, 1_800_000_000.7);
        let value = |name: &str| topics.iter().find(|(t, _)| t == &format!("lxmf/telemetry/ab12/{name}")).map(|(_, v)| v.as_str());
        assert_eq!(value("time/utc"), Some("1800000000"));
        assert_eq!(value("location/latitude"), Some("51.5074"));
        assert_eq!(value("location/longitude"), Some("-0.1278"));
        assert_eq!(value("location/altitude"), Some("12.0"));
        assert_eq!(value("location/accuracy"), Some("4.5"));
        assert_eq!(value("location/speed"), Some(""));
        assert_eq!(value("location/name"), Some("Location"));
        let mut length = Vec::new();
        remaining_length(321, &mut length);
        assert_eq!(length, [0xc1, 0x02]);
    }

    /// A server that takes a connection, says `code` to it, and acks each
    /// publish; what it was sent comes back.
    async fn broker(code: u8) -> (u16, tokio::task::JoinHandle<Vec<(u8, Vec<u8>)>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut got = Vec::new();
            while let Ok(kind) = stream.read_u8().await {
                let mut length = 0usize;
                for shift in 0..4 {
                    let byte = stream.read_u8().await.unwrap();
                    length |= ((byte & 0x7f) as usize) << (7 * shift);
                    if byte & 0x80 == 0 {
                        break;
                    }
                }
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                match kind {
                    0x10 => stream.write_all(&[0x20, 2, 0, code]).await.unwrap(),
                    0x32 => {
                        let topic = u16::from_be_bytes([body[0], body[1]]) as usize;
                        stream.write_all(&[0x40, 2, body[2 + topic], body[3 + topic]]).await.unwrap();
                    }
                    _ => {}
                }
                got.push((kind, body));
                if kind == 0xe0 {
                    break;
                }
            }
            got
        });
        (port, task)
    }

    #[tokio::test]
    async fn published_to_a_server_that_takes_them() {
        let (port, server) = broker(0).await;
        let to = Server { host: "127.0.0.1".into(), port, user: Some("ann".into()), password: Some("pw".into()) };
        let sent = topics("ab12", &Location::at(1.5, 2.5), 10.0);
        send_topics(&to, &sent).await.unwrap();
        let got = server.await.unwrap();
        // Connected with the login, every topic published, then gone.
        let (kind, connect) = &got[0];
        assert_eq!(*kind, 0x10);
        assert_eq!(connect[7] & 0xc0, 0xc0);
        assert!(connect.ends_with(b"\x00\x03ann\x00\x02pw"));
        assert_eq!(got.iter().filter(|(k, _)| *k == 0x32).count(), sent.len());
        let topic = b"lxmf/telemetry/ab12/location/latitude";
        let latitude = got.iter().find(|(_, b)| b.len() > 2 + topic.len() && &b[2..2 + topic.len()] == topic).unwrap();
        assert!(latitude.1.ends_with(b"1.5"));
        assert_eq!(got.last().unwrap().0, 0xe0);

        let (port, server) = broker(5).await;
        let refused = send_topics(&Server { port, ..to.clone() }, &sent).await.unwrap_err();
        assert!(refused.contains("user name or password"), "{refused}");
        let _ = server.await;
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
        assert!(send_topics(&Server { port: closed, ..to }, &sent).await.unwrap_err().contains("couldn't connect"));
    }
}
