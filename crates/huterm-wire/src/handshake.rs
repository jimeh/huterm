//! Version and feature negotiation: `Hello`, `Welcome`, and `Reject`.
//!
//! The client sends `Hello` first. Its message kind and its first two
//! fields, the major and minor versions, keep their position and encoding
//! in every major version, as do `Reject`'s kind, its reason tag, and the
//! `IncompatibleMajor` versions, so a peer of any major version reads a
//! clear version error. The server answers with `Welcome` or `Reject`.
//! Nothing negotiated outlives its connection: a reconnect, even to the same
//! incarnation after a handover, negotiates again.

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use huterm_protocol::{DigestProfile, ServerIncarnation};

use crate::caps::{
    Direction, MAX_DIAGNOSTIC_BYTES, MessageCaps, MessageKind,
    VIEWER_INPUT_BUDGET, reassembly_budget,
};
use crate::codec::{DecodeError, EncodeError, Reader, Writer};
use crate::frame::{DEFAULT_MAX_FRAME, MIN_MAX_FRAME};
use crate::stream::STREAM_IDLE_DEADLINE;
use crate::{MAJOR, MINOR, digest_profile};

/// How long a server waits for `Hello` before it closes the connection.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(5);

/// The most requests one connection may have outstanding.
pub const MAX_IN_FLIGHT_REQUESTS: u32 = 64;

/// The most updates a client may have outstanding per viewer.
pub const MAX_VIEWER_CREDIT: u32 = 2;

/// How long a server waits for a client that reads nothing, or stays over
/// its outbound budget, before it disconnects the client.
pub const WRITER_STALL_DEADLINE: Duration = Duration::from_secs(30);

/// How long a host effect at the head of its lane may wait for its first
/// frame to be written.
pub const HOST_EFFECT_DELIVERY_DEADLINE: Duration = Duration::from_secs(2);

/// The throughput a host effect's completion deadline assumes, in bytes per
/// second.
pub const HOST_EFFECT_MIN_THROUGHPUT: u64 = 8 << 20;

/// The most features one peer may name.
pub const MAX_FEATURES: usize = 64;

/// The longest socket name.
pub const MAX_SOCKET_NAME_BYTES: usize = 64;

/// The longest diagnostic message a `Reject` carries.
pub const MAX_REJECT_MESSAGE_BYTES: usize = 1024;

/// An optional behavior a peer may support within a minor version.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Feature(pub u32);

impl Feature {
    /// Rich terminal content and transfers, reserved for #88.
    pub const RICH_CONTENT: Self = Self(1);
}

/// What kind of client is connecting. This is policy, never
/// authentication.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClientKind {
    /// A desktop client.
    Desktop,
    /// A text-mode client.
    Text,
    /// A gateway that serves other clients, such as a browser gateway.
    Gateway,
    /// A command-line client.
    CommandLine,
}

/// A stable identity for a server, kept with the socket scope's saved state
/// so a picker or a saved layout can name the same server across restarts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ServerId(pub [u8; 16]);

/// The client's first message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hello {
    /// The client's protocol major version.
    pub major: u16,
    /// The highest minor version the client speaks.
    pub minor: u16,
    /// The features the client supports.
    pub features: BTreeSet<Feature>,
    /// What kind of client is connecting.
    pub client_kind: ClientKind,
    /// The largest frame the client accepts.
    pub max_frame: u32,
    /// The client's name, for diagnostics.
    pub name: String,
    /// The client's build version, for diagnostics.
    pub build_version: String,
}

impl Hello {
    /// A `Hello` at this build's versions and default frame size.
    #[must_use]
    pub fn new(
        client_kind: ClientKind,
        name: &str,
        build_version: &str,
    ) -> Self {
        Self {
            major: MAJOR,
            minor: MINOR,
            features: BTreeSet::new(),
            client_kind,
            max_frame: DEFAULT_MAX_FRAME,
            name: name.to_owned(),
            build_version: build_version.to_owned(),
        }
    }
}

/// The limits and deadlines that bind the client on one connection. A
/// remote transport changes the values, never the contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportProfile {
    /// The most requests the client may have outstanding.
    pub max_in_flight_requests: u32,
    /// The most updates the client may have outstanding per viewer.
    pub max_viewer_credit: u32,
    /// The most unacknowledged input bytes per viewer.
    pub viewer_input_budget: u64,
    /// The most bytes of unfinished client messages the server holds.
    pub reassembly_budget: u64,
    /// The cap of every kind the client may send.
    pub client_caps: Vec<(MessageKind, u64)>,
    /// How long a stream may go without a frame.
    pub stream_idle_deadline: Duration,
    /// How long the server waits on a client that stops reading.
    pub writer_stall_deadline: Duration,
    /// How long an effect at the head of its lane waits for its first
    /// frame.
    pub effect_delivery_deadline: Duration,
    /// The throughput, in bytes per second, effect completion deadlines
    /// assume.
    pub effect_min_throughput: u64,
    /// How long a broken connection's attachments and viewers wait to be
    /// resumed; zero on local IPC.
    pub suspend_grace: Duration,
}

impl TransportProfile {
    /// The local IPC profile at negotiated minor version `minor`.
    #[must_use]
    pub fn local(minor: u16) -> Self {
        let caps = MessageCaps::for_minor(minor);
        Self {
            max_in_flight_requests: MAX_IN_FLIGHT_REQUESTS,
            max_viewer_credit: MAX_VIEWER_CREDIT,
            viewer_input_budget: VIEWER_INPUT_BUDGET as u64,
            reassembly_budget: reassembly_budget(minor) as u64,
            client_caps: MessageKind::ALL
                .into_iter()
                .filter(|kind| kind.direction() == Direction::ClientToServer)
                .filter_map(|kind| Some((kind, caps.cap(kind)? as u64)))
                .collect(),
            stream_idle_deadline: STREAM_IDLE_DEADLINE,
            writer_stall_deadline: WRITER_STALL_DEADLINE,
            effect_delivery_deadline: HOST_EFFECT_DELIVERY_DEADLINE,
            effect_min_throughput: HOST_EFFECT_MIN_THROUGHPUT,
            suspend_grace: Duration::ZERO,
        }
    }
}

/// The server's acceptance of a `Hello`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Welcome {
    /// The server's major version, which equals the client's.
    pub major: u16,
    /// The negotiated minor version: the smaller of the two peers'.
    pub minor: u16,
    /// The negotiated features: those both peers support.
    pub features: BTreeSet<Feature>,
    /// The lifetime of the server's terminals that scopes every identity
    /// on this connection.
    pub incarnation: ServerIncarnation,
    /// The server's stable identity.
    pub server_id: ServerId,
    /// The server's host name, for diagnostics.
    pub host_name: String,
    /// The socket's name.
    pub socket_name: String,
    /// The server's build version, for diagnostics.
    pub build_version: String,
    /// The server's process ID, for diagnostics.
    pub process_id: u32,
    /// The negotiated frame limit: the smaller of the two peers' offers.
    pub max_frame: u32,
    /// The limits and deadlines that bind the client.
    pub profile: TransportProfile,
}

/// Why a server refused a `Hello`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectReason {
    /// The client speaks another major version.
    IncompatibleMajor {
        /// The server's major version.
        major: u16,
        /// The server's minor version.
        minor: u16,
    },
    /// The server holds as many connections as it serves.
    ServerBusy,
    /// The `Hello` did not decode or offered invalid values.
    Malformed,
}

/// The server's refusal of a `Hello`, after which it closes the connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reject {
    /// Why the server refused.
    pub reason: RejectReason,
    /// A diagnostic message, never parsed.
    pub message: String,
}

impl Reject {
    fn malformed(message: impl fmt::Display) -> Self {
        Self {
            reason: RejectReason::Malformed,
            message: message.to_string(),
        }
    }
}

/// What a server offers every client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerOffer {
    /// The highest minor version the server speaks.
    pub minor: u16,
    /// The features the server supports.
    pub features: BTreeSet<Feature>,
    /// The largest frame the server accepts.
    pub max_frame: u32,
    /// The server's incarnation.
    pub incarnation: ServerIncarnation,
    /// The server's stable identity.
    pub server_id: ServerId,
    /// The server's host name.
    pub host_name: String,
    /// The socket's name.
    pub socket_name: String,
    /// The server's build version.
    pub build_version: String,
    /// The server's process ID.
    pub process_id: u32,
}

/// What both peers derive from one `Welcome`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Negotiated {
    /// The negotiated minor version.
    pub minor: u16,
    /// The negotiated features.
    pub features: BTreeSet<Feature>,
    /// The negotiated frame limit.
    pub max_frame: u32,
    /// The caps at the negotiated minor version.
    pub caps: MessageCaps,
    /// The digest profile at the negotiated minor version.
    pub digest_profile: DigestProfile,
    /// The server's incarnation.
    pub incarnation: ServerIncarnation,
}

/// Why a client cannot accept a server's answer. Each one is a protocol
/// violation that closes the connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandshakeError {
    /// `Welcome` names another major version.
    MajorMismatch(u16),
    /// `Welcome` names a minor version above the client's.
    MinorAboveOwn(u16),
    /// `Welcome` names a feature the client did not offer.
    UnofferedFeature(Feature),
    /// `Welcome`'s frame limit is above the client's or below
    /// [`MIN_MAX_FRAME`].
    InvalidMaxFrame(u32),
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MajorMismatch(major) => {
                write!(f, "server speaks major version {major}")
            }
            Self::MinorAboveOwn(minor) => {
                write!(
                    f,
                    "server negotiated minor version {minor}, above the client's"
                )
            }
            Self::UnofferedFeature(feature) => {
                write!(
                    f,
                    "server negotiated feature {} the client did not offer",
                    feature.0
                )
            }
            Self::InvalidMaxFrame(max) => {
                write!(f, "invalid negotiated frame limit {max}")
            }
        }
    }
}

impl std::error::Error for HandshakeError {}

/// Answers a client's `Hello` message bytes: a `Welcome` with the smaller
/// minor version, the feature intersection, and the smaller frame limit,
/// or a `Reject`.
///
/// A client of another major version is refused with
/// [`RejectReason::IncompatibleMajor`] after reading only the frozen
/// leading fields, so the rest of its `Hello` may use any encoding.
///
/// # Errors
/// Returns the `Reject` to send before closing.
pub fn negotiate(hello: &[u8], offer: &ServerOffer) -> Result<Welcome, Reject> {
    let (major, _) = hello_versions(hello).map_err(Reject::malformed)?;
    if major != MAJOR {
        return Err(Reject {
            reason: RejectReason::IncompatibleMajor {
                major: MAJOR,
                minor: offer.minor,
            },
            message: format!(
                "this server speaks protocol {MAJOR}, the client {major}"
            ),
        });
    }
    let hello = decode_hello_message(hello).map_err(Reject::malformed)?;
    if hello.max_frame < MIN_MAX_FRAME {
        return Err(Reject::malformed(format_args!(
            "max_frame {} is below {MIN_MAX_FRAME}",
            hello.max_frame
        )));
    }
    let minor = hello.minor.min(offer.minor);
    Ok(Welcome {
        major: MAJOR,
        minor,
        features: hello
            .features
            .intersection(&offer.features)
            .copied()
            .collect(),
        incarnation: offer.incarnation,
        server_id: offer.server_id,
        host_name: offer.host_name.clone(),
        socket_name: offer.socket_name.clone(),
        build_version: offer.build_version.clone(),
        process_id: offer.process_id,
        max_frame: hello.max_frame.min(offer.max_frame),
        profile: TransportProfile::local(minor),
    })
}

/// Checks a server's `Welcome` against the `Hello` the client sent.
///
/// # Errors
/// Returns the protocol violation that makes the client close.
pub fn accept_welcome(
    hello: &Hello,
    welcome: &Welcome,
) -> Result<Negotiated, HandshakeError> {
    if welcome.major != hello.major {
        return Err(HandshakeError::MajorMismatch(welcome.major));
    }
    if welcome.minor > hello.minor {
        return Err(HandshakeError::MinorAboveOwn(welcome.minor));
    }
    if let Some(feature) = welcome.features.difference(&hello.features).next() {
        return Err(HandshakeError::UnofferedFeature(*feature));
    }
    if welcome.max_frame > hello.max_frame || welcome.max_frame < MIN_MAX_FRAME
    {
        return Err(HandshakeError::InvalidMaxFrame(welcome.max_frame));
    }
    Ok(Negotiated {
        minor: welcome.minor,
        features: welcome.features.clone(),
        max_frame: welcome.max_frame,
        caps: MessageCaps::for_minor(welcome.minor),
        digest_profile: digest_profile(welcome.minor),
        incarnation: welcome.incarnation,
    })
}

/// Reads only a `Hello` message's frozen leading fields: its kind and its
/// major and minor versions.
///
/// # Errors
/// Returns a decode error, or [`DecodeError::UnknownKind`] when the message
/// is not a `Hello`.
pub fn hello_versions(message: &[u8]) -> Result<(u16, u16), DecodeError> {
    let mut reader = Reader::new(message);
    let kind = reader.u64()?;
    if kind != MessageKind::Hello.number() {
        return Err(DecodeError::UnknownKind(kind));
    }
    reader.record(|r| Ok((r.u16()?, r.u16()?)))
}

fn decode_hello_message(message: &[u8]) -> Result<Hello, DecodeError> {
    let mut reader = Reader::new(message);
    reader.u64()?;
    let hello = decode_hello(&mut reader)?;
    reader.finish()?;
    Ok(hello)
}

fn features(
    w: &mut Writer,
    features: &BTreeSet<Feature>,
) -> Result<(), EncodeError> {
    w.seq(features.iter(), MAX_FEATURES, "features", |w, feature| {
        w.u32(feature.0);
        Ok(())
    })
}

/// Reads a feature set, which travels in ascending order without repeats.
fn read_features(r: &mut Reader<'_>) -> Result<BTreeSet<Feature>, DecodeError> {
    let list = r.seq(MAX_FEATURES, "features", |r| r.u32().map(Feature))?;
    if list.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(DecodeError::Invalid("feature set order"));
    }
    Ok(list.into_iter().collect())
}

fn client_kind_tag(kind: ClientKind) -> u64 {
    match kind {
        ClientKind::Desktop => 0,
        ClientKind::Text => 1,
        ClientKind::Gateway => 2,
        ClientKind::CommandLine => 3,
    }
}

/// Encodes a `Hello` record.
///
/// # Errors
/// Refuses diagnostics or feature sets past their limits.
pub fn encode_hello(w: &mut Writer, hello: &Hello) -> Result<(), EncodeError> {
    let Hello {
        major,
        minor,
        features: offered,
        client_kind,
        max_frame,
        name,
        build_version,
    } = hello;
    w.record(|w| {
        w.u16(*major);
        w.u16(*minor);
        features(w, offered)?;
        w.tag(client_kind_tag(*client_kind));
        w.u32(*max_frame);
        w.str(name, MAX_DIAGNOSTIC_BYTES, "client name")?;
        w.str(build_version, MAX_DIAGNOSTIC_BYTES, "build version")
    })
}

/// Decodes a `Hello` record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_hello(r: &mut Reader<'_>) -> Result<Hello, DecodeError> {
    r.record(|r| {
        Ok(Hello {
            major: r.u16()?,
            minor: r.u16()?,
            features: read_features(r)?,
            client_kind: match r.tag()? {
                0 => ClientKind::Desktop,
                1 => ClientKind::Text,
                2 => ClientKind::Gateway,
                3 => ClientKind::CommandLine,
                tag => {
                    return Err(DecodeError::UnknownTag {
                        field: "client kind",
                        tag,
                    });
                }
            },
            max_frame: r.u32()?,
            name: r.string(MAX_DIAGNOSTIC_BYTES, "client name")?,
            build_version: r.string(MAX_DIAGNOSTIC_BYTES, "build version")?,
        })
    })
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn encode_profile(
    w: &mut Writer,
    profile: &TransportProfile,
) -> Result<(), EncodeError> {
    let TransportProfile {
        max_in_flight_requests,
        max_viewer_credit,
        viewer_input_budget,
        reassembly_budget,
        client_caps,
        stream_idle_deadline,
        writer_stall_deadline,
        effect_delivery_deadline,
        effect_min_throughput,
        suspend_grace,
    } = profile;
    w.record(|w| {
        w.u32(*max_in_flight_requests);
        w.u32(*max_viewer_credit);
        w.u64(*viewer_input_budget);
        w.u64(*reassembly_budget);
        w.seq(
            client_caps.iter(),
            MessageKind::ALL.len(),
            "client caps",
            |w, (kind, cap)| {
                w.u64(kind.number());
                w.u64(*cap);
                Ok(())
            },
        )?;
        w.u64(millis(*stream_idle_deadline));
        w.u64(millis(*writer_stall_deadline));
        w.u64(millis(*effect_delivery_deadline));
        w.u64(*effect_min_throughput);
        w.u64(millis(*suspend_grace));
        Ok(())
    })
}

fn decode_profile(r: &mut Reader<'_>) -> Result<TransportProfile, DecodeError> {
    r.record(|r| {
        Ok(TransportProfile {
            max_in_flight_requests: r.u32()?,
            max_viewer_credit: r.u32()?,
            viewer_input_budget: r.u64()?,
            reassembly_budget: r.u64()?,
            client_caps: r.seq(MessageKind::ALL.len(), "client caps", |r| {
                let number = r.u64()?;
                let kind = MessageKind::from_number(number)
                    .filter(|kind| {
                        kind.direction() == Direction::ClientToServer
                    })
                    .ok_or(DecodeError::UnknownKind(number))?;
                Ok((kind, r.u64()?))
            })?,
            stream_idle_deadline: Duration::from_millis(r.u64()?),
            writer_stall_deadline: Duration::from_millis(r.u64()?),
            effect_delivery_deadline: Duration::from_millis(r.u64()?),
            effect_min_throughput: r.u64()?,
            suspend_grace: Duration::from_millis(r.u64()?),
        })
    })
}

/// Encodes a `Welcome` record.
///
/// # Errors
/// Refuses diagnostics or feature sets past their limits.
pub fn encode_welcome(
    w: &mut Writer,
    welcome: &Welcome,
) -> Result<(), EncodeError> {
    let Welcome {
        major,
        minor,
        features: negotiated,
        incarnation,
        server_id,
        host_name,
        socket_name,
        build_version,
        process_id,
        max_frame,
        profile,
    } = welcome;
    w.record(|w| {
        w.u16(*major);
        w.u16(*minor);
        features(w, negotiated)?;
        w.fixed(&incarnation.to_bytes());
        w.fixed(&server_id.0);
        w.str(host_name, MAX_DIAGNOSTIC_BYTES, "host name")?;
        w.str(socket_name, MAX_SOCKET_NAME_BYTES, "socket name")?;
        w.str(build_version, MAX_DIAGNOSTIC_BYTES, "build version")?;
        w.u32(*process_id);
        w.u32(*max_frame);
        encode_profile(w, profile)
    })
}

/// Decodes a `Welcome` record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_welcome(r: &mut Reader<'_>) -> Result<Welcome, DecodeError> {
    r.record(|r| {
        Ok(Welcome {
            major: r.u16()?,
            minor: r.u16()?,
            features: read_features(r)?,
            incarnation: ServerIncarnation::from_bytes(r.fixed()?),
            server_id: ServerId(r.fixed()?),
            host_name: r.string(MAX_DIAGNOSTIC_BYTES, "host name")?,
            socket_name: r.string(MAX_SOCKET_NAME_BYTES, "socket name")?,
            build_version: r.string(MAX_DIAGNOSTIC_BYTES, "build version")?,
            process_id: r.u32()?,
            max_frame: r.u32()?,
            profile: decode_profile(r)?,
        })
    })
}

/// Encodes a `Reject` record. A message past its limit is cut at a
/// character boundary rather than refused.
///
/// # Errors
/// Never fails today; the signature matches the other encoders.
pub fn encode_reject(
    w: &mut Writer,
    reject: &Reject,
) -> Result<(), EncodeError> {
    let Reject { reason, message } = reject;
    w.record(|w| {
        match reason {
            RejectReason::IncompatibleMajor { major, minor } => {
                w.tag(0);
                w.record(|w| {
                    w.u16(*major);
                    w.u16(*minor);
                    Ok(())
                })?;
            }
            RejectReason::ServerBusy => {
                w.tag(1);
                w.record(|_| Ok(()))?;
            }
            RejectReason::Malformed => {
                w.tag(2);
                w.record(|_| Ok(()))?;
            }
        }
        w.str(
            truncated(message, MAX_REJECT_MESSAGE_BYTES),
            MAX_REJECT_MESSAGE_BYTES,
            "reject message",
        )
    })
}

/// Decodes a `Reject` record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_reject(r: &mut Reader<'_>) -> Result<Reject, DecodeError> {
    r.record(|r| {
        let reason = match r.tag()? {
            0 => r.record(|r| {
                Ok(RejectReason::IncompatibleMajor {
                    major: r.u16()?,
                    minor: r.u16()?,
                })
            })?,
            1 => r.record(|_| Ok(RejectReason::ServerBusy))?,
            2 => r.record(|_| Ok(RejectReason::Malformed))?,
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "reject reason",
                    tag,
                });
            }
        };
        Ok(Reject {
            reason,
            message: r.string(MAX_REJECT_MESSAGE_BYTES, "reject message")?,
        })
    })
}

/// `text` cut to at most `max` bytes at a character boundary.
pub(crate) fn truncated(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max)]
}

#[cfg(test)]
mod tests;
