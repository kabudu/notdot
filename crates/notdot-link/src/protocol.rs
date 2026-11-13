//! `link/1`: a typed, allowlisted, versioned phone API. There is no generic
//! invoke and no pass-through to the desktop's runtime commands. Every
//! operation is checked against the device's scopes, and every operation
//! acts on the device's own profile; a profile cannot be named in a request.
use serde::{Deserialize, Serialize};

pub const LINK_VERSION: u16 = 1;
/// ALPN for paired-device traffic.
pub const LINK_ALPN: &[u8] = b"notdot/link/1";
/// ALPN for the pairing exchange.
pub const PAIR_ALPN: &[u8] = b"notdot/pair/1";
/// Largest frame either side accepts, in bytes.
pub const MAX_FRAME: usize = 64 * 1024;
/// Largest note accepted, in bytes.
pub const MAX_NOTE_BYTES: usize = 4 * 1024;

/// Idempotency key chosen by the phone. Retrying with the same id after a
/// reconnect never repeats the side effect. On the wire it is 32 lower-case
/// hex digits, since JSON numbers this large are not portable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub u128);
impl RequestId {
    pub fn random() -> Self {
        Self(u128::from_be_bytes(crate::random_bytes()))
    }
}
impl Serialize for RequestId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:032x}", self.0))
    }
}
impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        if text.len() != 32 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(serde::de::Error::custom("request id must be 32 hex digits"));
        }
        u128::from_str_radix(&text, 16)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// Synthetic subset of the desktop capabilities, enough to exercise the
/// policy, idempotency, cancellation and backpressure paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    Ping {},
    AddNote {
        text: String,
    },
    ListNotes {},
    ReadSettings {},
    /// Compare-and-set against the desktop's settings revision.
    StageSetting {
        key: String,
        value: String,
        base_revision: u64,
    },
    /// A cancellable long-running task with one side effect at the end.
    RunTask {
        label: String,
        steps: u32,
    },
    /// Sensitive: needs an owner grant and a fresh step-up.
    ResetMemory {},
    /// Ask for a step-up challenge for the operation with this digest.
    StepUpChallenge {
        #[serde(with = "crate::b64_32")]
        digest: [u8; 32],
    },
    /// Ask for a single-use challenge to enrol a step-up key.
    StepUpEnrollChallenge {},
    /// Enrol (or replace) this phone's step-up key with platform evidence
    /// bound to an enrolment challenge. Replacing a key ends any grant.
    StepUpEnroll {
        #[serde(with = "crate::b64_32")]
        nonce: [u8; 32],
        evidence: crate::attest::Evidence,
    },
    /// Whether this phone has an enrolled key and a live owner grant.
    StepUpStatus {},
}

/// Biometric step-up attached to a sensitive operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepUpProof {
    #[serde(with = "crate::b64_32")]
    pub nonce: [u8; 32],
    #[serde(with = "crate::b64")]
    pub signature: Vec<u8>,
    /// An App Attest assertion over the same message, for iPhones.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::attest::b64_object_opt"
    )]
    pub assertion: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Hello {
        versions: Vec<u16>,
    },
    Call {
        id: RequestId,
        op: Op,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_up: Option<StepUpProof>,
    },
    /// Cancel a call. A network drop never cancels; only this frame does.
    Cancel {
        id: RequestId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum OpError {
    /// The device's scopes do not include this operation.
    Forbidden,
    /// Too many calls in flight; retry later with the same id.
    Busy,
    Conflict {
        revision: u64,
    },
    NeedsStepUp,
    StepUpRejected,
    /// The step-up challenge expired before the proof arrived.
    StepUpExpired,
    /// This phone has no enrolled step-up key.
    StepUpNotEnrolled,
    /// The enrolment evidence did not verify; `reason` is plain language.
    AttestationRejected {
        reason: String,
    },
    BadRequest {
        reason: String,
    },
    UnsupportedVersion {
        supported: Vec<u16>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum ServerFrame {
    Welcome {
        version: u16,
    },
    /// Final: the operation committed. `result` is operation-specific.
    Done {
        id: RequestId,
        result: serde_json::Value,
    },
    /// Final: nothing was committed.
    Failed {
        id: RequestId,
        error: OpError,
    },
    /// Final: cancelled before commit; nothing was committed.
    Cancelled {
        id: RequestId,
    },
    /// Reply to a Cancel frame: whether the call was found in flight.
    CancelAck {
        id: RequestId,
        found: bool,
    },
}
impl ServerFrame {
    pub fn id(&self) -> Option<RequestId> {
        match self {
            Self::Done { id, .. }
            | Self::Failed { id, .. }
            | Self::Cancelled { id }
            | Self::CancelAck { id, .. } => Some(*id),
            Self::Welcome { .. } => None,
        }
    }
}

/// Pairing exchange frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum PairFrame {
    /// Desktop: request accepted, compare this code and approve.
    AwaitingApproval {
        sas: crate::pairing::Sas,
    },
    Approved {
        profile: String,
        scopes: Vec<crate::pairing::Scope>,
    },
    Denied,
    Rejected {
        reason: String,
    },
}

/// Length-prefixed JSON framing over any byte stream.
pub mod frame {
    use super::MAX_FRAME;
    use crate::LinkError;
    use serde::{Serialize, de::DeserializeOwned};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    pub async fn write<T: Serialize>(
        out: &mut (impl AsyncWrite + Unpin),
        value: &T,
    ) -> Result<(), LinkError> {
        let bytes = serde_json::to_vec(value).map_err(|_| LinkError::BadRequest("encode"))?;
        if bytes.len() > MAX_FRAME {
            return Err(LinkError::FrameTooLarge);
        }
        out.write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .map_err(LinkError::io)?;
        out.write_all(&bytes).await.map_err(LinkError::io)
    }

    /// Read one frame; `Ok(None)` on a clean end of stream.
    pub async fn read<T: DeserializeOwned>(
        input: &mut (impl AsyncRead + Unpin),
    ) -> Result<Option<T>, LinkError> {
        let mut len = [0u8; 4];
        match input.read_exact(&mut len).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(LinkError::io(e)),
        }
        let len = u32::from_be_bytes(len) as usize;
        if len > MAX_FRAME {
            return Err(LinkError::FrameTooLarge);
        }
        let mut bytes = vec![0; len];
        input.read_exact(&mut bytes).await.map_err(LinkError::io)?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| LinkError::BadRequest("malformed frame"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::protocol::{ClientFrame, Op, RequestId};

        #[tokio::test]
        async fn frames_round_trip_and_oversized_frames_are_refused() {
            let (mut a, mut b) = tokio::io::duplex(1 << 20);
            let call = ClientFrame::Call {
                id: RequestId(7),
                op: Op::Ping {},
                step_up: None,
            };
            write(&mut a, &call).await.unwrap();
            assert_eq!(read::<ClientFrame>(&mut b).await.unwrap(), Some(call));
            a.write_all(&(MAX_FRAME as u32 + 1).to_be_bytes())
                .await
                .unwrap();
            assert!(matches!(
                read::<ClientFrame>(&mut b).await,
                Err(LinkError::FrameTooLarge)
            ));
            let (mut a, mut b) = tokio::io::duplex(64);
            a.write_all(&[0, 0, 0, 2, b'{', b'}']).await.unwrap();
            drop(a);
            assert!(read::<ClientFrame>(&mut b).await.is_err());
            assert_eq!(read::<ClientFrame>(&mut b).await.unwrap(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_operations_and_injected_profile_fields_are_rejected() {
        for text in [
            r#"{"frame":"call","id":"00000000000000000000000000000001","op":{"op":"run_shell","cmd":"rm"}}"#,
            r#"{"frame":"call","id":"00000000000000000000000000000001","op":{"op":"list_notes","profile":"owner"}}"#,
            r#"{"frame":"call","id":"00000000000000000000000000000001","op":{"op":"ping"},"as_profile":"owner"}"#,
            r#"{"frame":"call","id":1,"op":{"op":"ping"}}"#,
            r#"{"frame":"call","id":"0000000000000000000000000000000G","op":{"op":"ping"}}"#,
        ] {
            assert!(serde_json::from_str::<ClientFrame>(text).is_err(), "{text}");
        }
        let ok: ClientFrame = serde_json::from_str(
            r#"{"frame":"call","id":"000000000000000000000000000000ff","op":{"op":"list_notes"}}"#,
        )
        .unwrap();
        assert!(matches!(
            ok,
            ClientFrame::Call {
                id: RequestId(255),
                op: Op::ListNotes {},
                ..
            }
        ));
    }
}
