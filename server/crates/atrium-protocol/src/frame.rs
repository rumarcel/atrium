//! Framing: a `u32` big-endian length, then that many bytes of UTF-8 JSON.
//!
//! This module is pure. The two processes read a header, ask
//! [`body_length`] whether to go on, allocate exactly the length it returns,
//! read that many bytes and hand them to a decoder here. The bound is
//! checked on the four header bytes, before any allocation, so an attacker
//! declaring 4 GiB costs Agent four bytes of reading and no memory.
//!
//! Decoding is strict: [`decode_core_frame`] accepts only the exact types in
//! [`crate::messages`]. When strict decoding fails, a second, read-only look
//! at the same bytes picks the most useful [`DecodeError`] for the journal
//! and the error frame. That look *classifies* a failure and nothing more:
//! no input that fails strict decoding is ever acted on.

use serde::Serialize;

use crate::messages::{AgentFrame, AgentOp, CoreFrame};
use crate::values::{RequestId, Version};
use crate::{AGENT_PROTOCOL_VERSION, MAX_FRAME_BYTES};

/// Size of the length prefix.
pub const HEADER_BYTES: usize = 4;

/// Why a length prefix was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthError {
    /// Zero. Every frame is a JSON object, so an empty body is never valid.
    Empty,
    /// Over [`MAX_FRAME_BYTES`]. The declared value is kept for the journal.
    TooLarge {
        /// What the peer declared.
        declared: u32,
    },
}

/// Validates a length prefix and returns the body length to allocate.
///
/// # Errors
///
/// [`LengthError`] for zero or for anything over [`MAX_FRAME_BYTES`].
pub fn body_length(header: [u8; HEADER_BYTES]) -> Result<usize, LengthError> {
    let declared = u32::from_be_bytes(header);
    if declared == 0 {
        return Err(LengthError::Empty);
    }
    match usize::try_from(declared) {
        Ok(length) if length <= MAX_FRAME_BYTES => Ok(length),
        _ => Err(LengthError::TooLarge { declared }),
    }
}

/// Why a frame could not be encoded.
#[derive(Debug)]
pub enum EncodeError {
    /// serde_json refused, which the types in this crate cannot cause.
    Json(serde_json::Error),
    /// The encoded body is over [`MAX_FRAME_BYTES`].
    TooLarge(usize),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "could not encode a frame: {error}"),
            Self::TooLarge(length) => write!(
                formatter,
                "an encoded frame of {length} bytes is over the {MAX_FRAME_BYTES}-byte limit"
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Encodes one frame: length prefix and body.
///
/// # Errors
///
/// [`EncodeError`] when the body would be over the limit.
pub fn encode<T: Serialize>(frame: &T) -> Result<Vec<u8>, EncodeError> {
    let body = serde_json::to_vec(frame).map_err(EncodeError::Json)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(EncodeError::TooLarge(body.len()));
    }
    let length = u32::try_from(body.len()).map_err(|_| EncodeError::TooLarge(body.len()))?;
    let mut framed = Vec::with_capacity(HEADER_BYTES + body.len());
    framed.extend_from_slice(&length.to_be_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

/// Why a body was refused. A closed set; each maps to one journal reason and
/// one [`crate::messages::ErrorCode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Not a frame this protocol defines.
    Malformed,
    /// A `hello` for another protocol version.
    ProtocolMismatch {
        /// The version the peer declared, saturated to `u32`.
        found: u32,
    },
    /// A `hello` whose `request_id` is not valid.
    InvalidRequestId,
    /// A `hello` whose `core_version` is not valid.
    InvalidCoreVersion,
    /// A `call` naming an operation this protocol does not define.
    UnknownOperation,
}

/// Decodes a frame from Core.
///
/// A strictly valid `hello` for another protocol version decodes
/// successfully; comparing versions is the caller's job, because it is the
/// caller that must refuse.
///
/// # Errors
///
/// [`DecodeError`], classified as precisely as the bytes allow.
pub fn decode_core_frame(body: &[u8]) -> Result<CoreFrame, DecodeError> {
    match serde_json::from_slice::<CoreFrame>(body) {
        Ok(frame) => Ok(frame),
        Err(_) => Err(classify_core_failure(body)),
    }
}

/// Decodes a frame from Agent. Core does not need a finer classification
/// than "not a frame": any of it is a protocol error.
///
/// # Errors
///
/// [`DecodeError::Malformed`].
pub fn decode_agent_frame(body: &[u8]) -> Result<AgentFrame, DecodeError> {
    serde_json::from_slice(body).map_err(|_| DecodeError::Malformed)
}

/// Picks the reason a Core frame failed strict decoding. Read-only: the
/// result is a reason code, never a frame.
fn classify_core_failure(body: &[u8]) -> DecodeError {
    use serde_json::Value;

    // serde_json bounds nesting at 128 levels, and the body is at most
    // 64 KiB, so this second parse is bounded in both stack and memory.
    let Ok(Value::Object(outer)) = serde_json::from_slice::<Value>(body) else {
        return DecodeError::Malformed;
    };
    if outer.len() != 1 {
        return DecodeError::Malformed;
    }
    let Some((tag, inner)) = outer.iter().next() else {
        return DecodeError::Malformed;
    };
    let Value::Object(fields) = inner else {
        return DecodeError::Malformed;
    };
    match tag.as_str() {
        "hello" => {
            // The protocol number decides first: a hello from another
            // version is a mismatch whatever else it carries, so a future
            // Core with a different hello still gets a clear refusal.
            if let Some(protocol) = fields.get("protocol").and_then(Value::as_u64) {
                if protocol != u64::from(AGENT_PROTOCOL_VERSION) {
                    return DecodeError::ProtocolMismatch {
                        found: u32::try_from(protocol).unwrap_or(u32::MAX),
                    };
                }
            }
            if let Some(Value::String(id)) = fields.get("request_id") {
                if RequestId::parse(id).is_err() {
                    return DecodeError::InvalidRequestId;
                }
            }
            if let Some(Value::String(version)) = fields.get("core_version") {
                if Version::parse(version).is_err() {
                    return DecodeError::InvalidCoreVersion;
                }
            }
            DecodeError::Malformed
        }
        "call" => {
            let only_op = fields.len() == 1;
            match fields.get("op") {
                Some(Value::String(name))
                    if only_op && !AgentOp::ALL.iter().any(|op| op.as_str() == name) =>
                {
                    DecodeError::UnknownOperation
                }
                _ => DecodeError::Malformed,
            }
        }
        _ => DecodeError::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::*;
    use crate::values::*;

    fn hello() -> CoreFrame {
        CoreFrame::Hello(Hello {
            protocol: AGENT_PROTOCOL_VERSION,
            core_version: Version::parse("0.1.0").expect("version"),
            request_id: RequestId::parse("3f1c").expect("id"),
        })
    }

    fn body(frame: &[u8]) -> &[u8] {
        let length = body_length(frame[..4].try_into().expect("header")).expect("length");
        assert_eq!(length, frame.len() - 4);
        &frame[4..]
    }

    #[test]
    fn frame_length_prefix_round_trip() {
        let framed = encode(&hello()).expect("encode");
        let decoded = decode_core_frame(body(&framed)).expect("decode");
        assert_eq!(decoded, hello());
        assert_eq!(
            &framed[4..],
            br#"{"hello":{"protocol":1,"core_version":"0.1.0","request_id":"3f1c"}}"#
        );
    }

    #[test]
    fn frame_over_64kib_rejected_before_alloc() {
        // The decision is made on the four header bytes alone.
        assert_eq!(body_length(65_536u32.to_be_bytes()), Ok(65_536));
        assert_eq!(
            body_length(65_537u32.to_be_bytes()),
            Err(LengthError::TooLarge { declared: 65_537 })
        );
        assert_eq!(
            body_length(u32::MAX.to_be_bytes()),
            Err(LengthError::TooLarge { declared: u32::MAX })
        );
        assert_eq!(body_length([0; 4]), Err(LengthError::Empty));
    }

    #[test]
    fn encoding_refuses_an_oversized_body() {
        #[derive(serde::Serialize)]
        struct Big {
            filler: String,
        }
        let big = Big {
            filler: "x".repeat(MAX_FRAME_BYTES),
        };
        assert!(matches!(encode(&big), Err(EncodeError::TooLarge(_))));
    }

    #[test]
    fn unknown_field_rejected() {
        for text in [
            r#"{"hello":{"protocol":1,"core_version":"0.1.0","request_id":"3f1c","uid":0}}"#,
            r#"{"call":{"op":"agent_info","path":"/etc/shadow"}}"#,
            r#"{"call":{"op":"agent_info"},"extra":1}"#,
            r#"{"hello":{"protocol":1,"core_version":"0.1.0","request_id":"3f1c"},"call":{"op":"agent_info"}}"#,
        ] {
            assert_eq!(
                decode_core_frame(text.as_bytes()),
                Err(DecodeError::Malformed),
                "{text}"
            );
        }
    }

    #[test]
    fn a_claimed_uid_is_not_a_field() {
        // Identity comes from SO_PEERCRED and nowhere else. A frame that
        // tries to claim one is malformed, whatever the value.
        let text =
            r#"{"hello":{"protocol":1,"core_version":"0.1.0","request_id":"3f1c","peer_uid":999}}"#;
        assert_eq!(
            decode_core_frame(text.as_bytes()),
            Err(DecodeError::Malformed)
        );
    }

    #[test]
    fn truncated_frame() {
        let framed = encode(&hello()).expect("encode");
        let full = &framed[4..];
        for cut in 1..full.len() {
            assert!(
                decode_core_frame(&full[..cut]).is_err(),
                "a body cut at {cut} must not decode"
            );
        }
    }

    #[test]
    fn non_utf8_payload() {
        let mut bytes =
            br#"{"hello":{"protocol":1,"core_version":"0.1.0","request_id":"3f1c"}}"#.to_vec();
        bytes[30] = 0xff;
        assert_eq!(decode_core_frame(&bytes), Err(DecodeError::Malformed));
        assert_eq!(
            decode_core_frame(&[0xc3, 0x28]),
            Err(DecodeError::Malformed)
        );
    }

    #[test]
    fn version_mismatch_is_terminal() {
        // A strictly valid hello for another version decodes, so the caller
        // sees the number and refuses; a hello a future version shaped
        // differently still classifies as a mismatch rather than as noise.
        let valid_v2 = r#"{"hello":{"protocol":2,"core_version":"0.2.0","request_id":"ab"}}"#;
        match decode_core_frame(valid_v2.as_bytes()) {
            Ok(CoreFrame::Hello(hello)) => assert_eq!(hello.protocol, 2),
            other => panic!("expected a hello, got {other:?}"),
        }
        let reshaped_v2 = r#"{"hello":{"protocol":2,"features":["x"]}}"#;
        assert_eq!(
            decode_core_frame(reshaped_v2.as_bytes()),
            Err(DecodeError::ProtocolMismatch { found: 2 })
        );
        let huge = r#"{"hello":{"protocol":99999999999,"x":1}}"#;
        assert_eq!(
            decode_core_frame(huge.as_bytes()),
            Err(DecodeError::ProtocolMismatch { found: u32::MAX })
        );
    }

    #[test]
    fn an_invalid_request_id_is_named_before_it_is_used() {
        for id in ["", "ABC", "3f1c\n", "\"; rm -rf /", &"a".repeat(65)] {
            let text = serde_json::json!({"hello": {
                "protocol": 1, "core_version": "0.1.0", "request_id": id}})
            .to_string();
            assert_eq!(
                decode_core_frame(text.as_bytes()),
                Err(DecodeError::InvalidRequestId),
                "{id:?}"
            );
        }
        let text = r#"{"hello":{"protocol":1,"core_version":"v1","request_id":"ab"}}"#;
        assert_eq!(
            decode_core_frame(text.as_bytes()),
            Err(DecodeError::InvalidCoreVersion)
        );
    }

    #[test]
    fn unknown_operation_is_named() {
        for op in ["execute", "command", "shell", "AgentInfo", "agent_info "] {
            let text = serde_json::json!({"call": {"op": op}}).to_string();
            assert_eq!(
                decode_core_frame(text.as_bytes()),
                Err(DecodeError::UnknownOperation),
                "{op:?}"
            );
        }
        // An operation with a parameter is not an unknown operation; it is a
        // shape this protocol does not have.
        for text in [
            r#"{"call":{"op":{"agent_info":{}}}}"#,
            r#"{"call":{"op":{"agent_info":null}}}"#,
            r#"{"call":{"op":"runtime_probe","socket":"/tmp/x.sock"}}"#,
            r#"{"call":{"op":["agent_info"]}}"#,
        ] {
            assert_eq!(
                decode_core_frame(text.as_bytes()),
                Err(DecodeError::Malformed),
                "{text}"
            );
        }
    }

    #[test]
    fn duplicate_keys_are_refused() {
        for text in [
            r#"{"call":{"op":"agent_info","op":"runtime_probe"}}"#,
            r#"{"hello":{"protocol":1,"protocol":1,"core_version":"0.1.0","request_id":"ab"}}"#,
        ] {
            assert!(decode_core_frame(text.as_bytes()).is_err(), "{text}");
        }
    }

    #[test]
    fn deep_nesting_is_refused_without_overflow() {
        let text = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
        assert_eq!(
            decode_core_frame(text.as_bytes()),
            Err(DecodeError::Malformed)
        );
        let object = format!(
            "{{\"call\":{}1{}}}",
            "{\"a\":".repeat(5_000),
            "}".repeat(5_000)
        );
        assert_eq!(
            decode_core_frame(object.as_bytes()),
            Err(DecodeError::Malformed)
        );
    }

    #[test]
    fn operations_carry_no_parameters() {
        for op in AgentOp::ALL {
            let text = serde_json::to_string(&CoreFrame::Call(Call { op })).expect("encode");
            assert_eq!(text, format!("{{\"call\":{{\"op\":\"{}\"}}}}", op.as_str()));
            assert_eq!(
                decode_core_frame(text.as_bytes()),
                Ok(CoreFrame::Call(Call { op }))
            );
        }
    }

    #[test]
    fn agent_frames_round_trip_and_stay_strict() {
        let probe = AgentFrame::RuntimeProbe(RuntimeProbe {
            runtime: Some(Runtime::Docker),
            socket: Some(RuntimeSocket::DockerVarRun),
            reachable: true,
            version: NotProbed,
            version_reason: VersionReason::RuntimeVersionProbeNotInM1,
            also_present: vec![RuntimeSocket::PodmanRun],
        });
        let framed = encode(&probe).expect("encode");
        let text = std::str::from_utf8(&framed[4..]).expect("utf-8");
        assert!(text.contains("\"version\":null"), "{text}");
        assert!(
            text.contains("\"version_reason\":\"runtime_version_probe_not_in_m1\""),
            "{text}"
        );
        assert!(!text.contains('/'), "no path crosses the boundary: {text}");
        assert_eq!(decode_agent_frame(&framed[4..]), Ok(probe));

        let with_path = r#"{"runtime_probe":{"runtime":"docker","socket":"/tmp/any.sock","reachable":true,"version":null,"version_reason":"runtime_version_probe_not_in_m1","also_present":[]}}"#;
        assert_eq!(
            decode_agent_frame(with_path.as_bytes()),
            Err(DecodeError::Malformed)
        );
        let with_version = r#"{"runtime_probe":{"runtime":"docker","socket":"docker_run","reachable":true,"version":"27.0","version_reason":"runtime_version_probe_not_in_m1","also_present":[]}}"#;
        assert_eq!(
            decode_agent_frame(with_version.as_bytes()),
            Err(DecodeError::Malformed)
        );
    }

    /// A small, fixed-seed generator: the property below runs the same way
    /// on every machine, and needs no dependency.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn parse_never_panics_on_arbitrary_or_mutated_bytes() {
        let seeds: Vec<Vec<u8>> = [
            encode(&hello()).expect("encode")[4..].to_vec(),
            br#"{"call":{"op":"runtime_probe"}}"#.to_vec(),
            br#"{"hello":{"protocol":2}}"#.to_vec(),
        ]
        .into();
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        for round in 0..20_000 {
            let mut bytes = if round % 4 == 0 {
                let length = usize::try_from(rng.next() % 256).expect("small");
                (0..length)
                    .map(|_| u8::try_from(rng.next() & 0xff).expect("byte"))
                    .collect()
            } else {
                let seed = usize::try_from(rng.next() % 3).expect("small");
                seeds[seed].clone()
            };
            if !bytes.is_empty() {
                for _ in 0..(rng.next() % 4) {
                    let at = usize::try_from(rng.next()).expect("usize") % bytes.len();
                    bytes[at] = u8::try_from(rng.next() & 0xff).expect("byte");
                }
            }
            let _ = decode_core_frame(&bytes);
            let _ = decode_agent_frame(&bytes);
        }
    }
}
