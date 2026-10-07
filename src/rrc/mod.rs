//! RRC (Reticulum Relay Chat): the client side of the protocol.
//!
//! Every RRC message is a CBOR map with small integer keys, sent as a plain
//! packet over an identified Link to the hub's `rrc.hub` destination.
//! References: NomadNet's client (`nomadnet/RRC.py`) and the reference hub
//! rrcd (<https://github.com/kc1awv/rrcd>).
//!
//! - [`envelope`]: the wire format.
//! - [`hub`]: handshake bodies and the hub's text replies.
//! - [`text`]: room and nick rules, mentions, message splitting and links.
//! - [`session`]: one live hub connection, run by the network actor.

mod envelope;
mod hub;
pub mod session;
mod text;

pub use envelope::{DEFAULT_ASPECT, Envelope, now_ms, t};
pub use hub::{
    Limits, ResourceAnnouncement, Welcome, hello_body, parse_resource_envelope, parse_room_info, parse_room_list, parse_welcome, parse_who,
};
pub use text::{complete_names, mention_prefix, mention_ranges, normalize_nick, normalize_room, parse_link, split_message, user_mentions};
