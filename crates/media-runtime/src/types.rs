use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_DATA_BYTES: usize = 16_384;
pub const EVENT_BATCH_LIMIT: usize = 32;
pub const PUBLIC_EVENT_CAPACITY: usize = 64;
pub const INPUT_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Camera,
    Screen,
    Microphone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSlot {
    pub kind: SourceKind,
    pub mid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDescriptor {
    pub id: String,
    pub kind: SourceKind,
}

/// One active publication mapped onto a negotiated native slot. Contains no frame/track handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBinding {
    pub id: String,
    pub kind: SourceKind,
    pub mid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSourceDescriptor {
    pub peer_id: String,
    pub id: String,
    pub kind: SourceKind,
    pub mid: String,
}

/// Sampled native receive observations. A signature does not identify a publication frame boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaObservation {
    pub kind: SourceKind,
    pub mid: String,
    pub frames_decoded: u64,
    pub total_samples_received: u64,
    pub bytes_received: u64,
    pub observed_frames: u64,
    pub content_signature: u64,
    pub audio_energy: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerMediaStats {
    pub peer_id: String,
    pub media: Vec<MediaObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Idle,
    Joining,
    Joined,
    Leaving,
    Destroyed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
    pub state: State,
    pub peer_id: Option<String>,
    pub peers: Vec<String>,
    pub ready_peers: Vec<String>,
    pub local_sources: Vec<SourceDescriptor>,
    pub remote_sources: Vec<RemoteSourceDescriptor>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            state: State::Idle,
            peer_id: None,
            peers: vec![],
            ready_peers: vec![],
            local_sources: vec![],
            remote_sources: vec![],
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JoinOptions {
    pub signaling_url: String,
    pub room_id: String,
    pub room_token: String,
    pub device_token: String,
    #[serde(default)]
    pub ice_servers: Vec<IceServer>,
}
// Credentials deliberately have no Debug/Serialize implementation.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeError {
    pub code: String,
    pub message: String,
}
impl NativeError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn cancelled() -> Self {
        Self::new("cancelled", "The operation was cancelled")
    }
    pub fn destroyed() -> Self {
        Self::new("destroyed", "The client has been destroyed")
    }
}
impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for NativeError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Event {
    Message {
        #[serde(rename = "peerId")]
        peer_id: String,
        data: String,
    },
    Error {
        error: NativeError,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventBatch {
    pub snapshot: Snapshot,
    pub events: Vec<Event>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendFailure {
    pub peer_id: String,
    pub message: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub accepted_peer_ids: Vec<String>,
    pub failures: Vec<SendFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DescriptionType {
    Offer,
    Answer,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Description {
    pub r#type: DescriptionType,
    pub sdp: String,
    #[serde(default)]
    pub slots: Vec<MediaSlot>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_m_line_index: Option<u16>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SignalPayload {
    Description {
        description: Description,
    },
    Ice {
        candidate: Candidate,
    },
    Sources {
        revision: u64,
        sources: Vec<SourceBinding>,
    },
    // The browser laboratory's individual track metadata is not a native source manifest.
    Track {
        id: String,
        kind: String,
        mid: String,
    },
    TrackRemoved {
        id: String,
    },
}
