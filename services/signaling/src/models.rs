//! Wire data only. Owns no runtime resources; depends only on serialization.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCredentials {
    pub peer_id: String,
    pub device_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomCredentials {
    pub room_id: String,
    pub room_token: String,
    pub ice_servers: Vec<IceServer>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ClientMessage {
    #[serde(rename_all = "camelCase")]
    Join {
        device_token: String,
        room_id: String,
        room_token: String,
    },
    Signal {
        to: String,
        payload: Value,
    },
    Ping,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerMessage {
    #[serde(rename_all = "camelCase")]
    Joined {
        peer_id: String,
        peers: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    PeerJoined {
        peer_id: String,
    },
    #[serde(rename_all = "camelCase")]
    PeerLeft {
        peer_id: String,
    },
    Signal {
        from: String,
        payload: Value,
    },
    Pong,
    Error {
        code: String,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceError {
    Unauthorized,
    RoomUnavailable,
    RoomFull,
    AlreadyJoined,
    PeerUnavailable,
    Capacity,
    SlowConsumer,
}

impl ServiceError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::RoomUnavailable => "room-unavailable",
            Self::RoomFull => "room-full",
            Self::AlreadyJoined => "already-joined",
            Self::PeerUnavailable => "peer-unavailable",
            Self::Capacity => "capacity",
            Self::SlowConsumer => "slow-consumer",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Unauthorized => "Valid device credentials are required.",
            Self::RoomUnavailable => "The room invitation is invalid or expired.",
            Self::RoomFull => "This room already has four participants.",
            Self::AlreadyJoined => "This device already has an active membership.",
            Self::PeerUnavailable => "The recipient is not an active member of this room.",
            Self::Capacity => "The server has reached its configured capacity.",
            Self::SlowConsumer => "The recipient cannot receive more messages.",
        }
    }

    pub fn wire(self) -> ServerMessage {
        ServerMessage::Error {
            code: self.code().into(),
            message: self.message().into(),
        }
    }
}
