//! Authoritative bounded credential/room membership store. No network I/O or media policy.
//! Short synchronous critical sections own all membership changes; socket owners remove
//! their lease on teardown, and periodic cleanup expires credentials and rooms.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, watch, Semaphore};

use crate::{
    config::Config,
    models::{DeviceCredentials, ServerMessage, ServiceError},
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub connection_slots: Arc<Semaphore>,
    store: Arc<Mutex<Store>>,
}

#[derive(Default)]
struct Store {
    devices: HashMap<[u8; 32], Device>,
    rooms: HashMap<String, Room>,
    memberships: HashMap<String, Membership>,
    next_expiry: Option<Instant>,
}

struct Device {
    peer_id: String,
    expires_at: Instant,
}
struct Room {
    token_hash: [u8; 32],
    expires_at: Instant,
    members: HashMap<String, Member>,
}
struct Member {
    sender: mpsc::Sender<ServerMessage>,
    stop: watch::Sender<bool>,
}

#[derive(Clone, Debug)]
pub struct Membership {
    pub peer_id: String,
    pub room_id: String,
    connection_id: String,
}

impl AppState {
    pub fn new(config: Config) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            connection_slots: Arc::new(Semaphore::new(config.max_connections)),
            config: Arc::new(config),
            store: Arc::new(Mutex::new(Store::default())),
        })
    }

    pub fn create_device(&self, now: Instant) -> Result<DeviceCredentials, ServiceError> {
        let mut store = self.store.lock().expect("credential store poisoned");
        store.cleanup(now);
        if store.devices.len() >= self.config.max_devices {
            return Err(ServiceError::Capacity);
        }
        let credentials = DeviceCredentials {
            peer_id: format!("p_{}", random_secret(16)),
            device_token: random_secret(32),
        };
        store.record_expiry(now + self.config.device_ttl);
        store.devices.insert(
            hash(&credentials.device_token),
            Device {
                peer_id: credentials.peer_id.clone(),
                expires_at: now + self.config.device_ttl,
            },
        );
        Ok(credentials)
    }

    pub fn create_room(
        &self,
        device_token: &str,
        now: Instant,
    ) -> Result<(String, String, String), ServiceError> {
        let mut store = self.store.lock().expect("credential store poisoned");
        store.cleanup(now);
        let peer_id = store.authenticate(device_token)?.to_owned();
        if store.rooms.len() >= self.config.max_rooms {
            return Err(ServiceError::Capacity);
        }
        let room_id = format!("r_{}", random_secret(16));
        let room_token = random_secret(32);
        store.record_expiry(now + self.config.room_ttl);
        store.rooms.insert(
            room_id.clone(),
            Room {
                token_hash: hash(&room_token),
                expires_at: now + self.config.room_ttl,
                members: HashMap::new(),
            },
        );
        Ok((room_id, room_token, peer_id))
    }

    pub fn join(
        &self,
        device_token: &str,
        room_id: &str,
        room_token: &str,
        sender: mpsc::Sender<ServerMessage>,
        stop: watch::Sender<bool>,
        now: Instant,
    ) -> Result<Membership, ServiceError> {
        let mut store = self.store.lock().expect("credential store poisoned");
        store.cleanup(now);
        let peer_id = store.authenticate(device_token)?.to_owned();
        let room = store
            .rooms
            .get(room_id)
            .ok_or(ServiceError::RoomUnavailable)?;
        if !bool::from(room.token_hash.ct_eq(&hash(room_token))) {
            return Err(ServiceError::RoomUnavailable);
        }
        if store.memberships.contains_key(&peer_id) {
            return Err(ServiceError::AlreadyJoined);
        }
        if room.members.len() >= 4 {
            return Err(ServiceError::RoomFull);
        }
        let mut peers: Vec<String> = room.members.keys().cloned().collect();
        peers.sort();
        sender
            .try_send(ServerMessage::Joined {
                peer_id: peer_id.clone(),
                peers,
            })
            .map_err(|_| ServiceError::SlowConsumer)?;
        let membership = Membership {
            peer_id: peer_id.clone(),
            room_id: room_id.to_owned(),
            connection_id: random_secret(16),
        };
        let room = store
            .rooms
            .get_mut(room_id)
            .expect("room checked in same lock");
        room.broadcast(ServerMessage::PeerJoined {
            peer_id: peer_id.clone(),
        });
        room.members
            .insert(peer_id.clone(), Member { sender, stop });
        store.memberships.insert(peer_id, membership.clone());
        Ok(membership)
    }

    pub fn route_signal(
        &self,
        membership: &Membership,
        to: &str,
        payload: serde_json::Value,
        now: Instant,
    ) -> Result<(), ServiceError> {
        let mut store = self.store.lock().expect("credential store poisoned");
        store.cleanup(now);
        if !store.is_current(membership) {
            return Err(ServiceError::Unauthorized);
        }
        let room = store
            .rooms
            .get(&membership.room_id)
            .ok_or(ServiceError::RoomUnavailable)?;
        let recipient = room.members.get(to).ok_or(ServiceError::PeerUnavailable)?;
        recipient.send(ServerMessage::Signal {
            from: membership.peer_id.clone(),
            payload,
        })
    }

    pub fn leave(&self, membership: &Membership) {
        let mut store = self.store.lock().expect("credential store poisoned");
        if store.is_current(membership) {
            store.remove_member(&membership.peer_id);
        }
    }

    pub fn cleanup(&self, now: Instant) {
        self.store
            .lock()
            .expect("credential store poisoned")
            .cleanup(now);
    }

    pub fn disconnect_all(&self) {
        let mut store = self.store.lock().expect("credential store poisoned");
        for room in store.rooms.values() {
            for member in room.members.values() {
                member.stop.send_replace(true);
            }
        }
        store.memberships.clear();
        store.rooms.clear();
        store.devices.clear();
        store.next_expiry = None;
    }
}

impl Store {
    fn record_expiry(&mut self, expires_at: Instant) {
        self.next_expiry = Some(
            self.next_expiry
                .map_or(expires_at, |next| next.min(expires_at)),
        );
    }

    fn authenticate(&self, token: &str) -> Result<&str, ServiceError> {
        self.devices
            .get(&hash(token))
            .map(|device| device.peer_id.as_str())
            .ok_or(ServiceError::Unauthorized)
    }

    fn is_current(&self, membership: &Membership) -> bool {
        self.memberships
            .get(&membership.peer_id)
            .is_some_and(|current| {
                current.connection_id == membership.connection_id
                    && current.room_id == membership.room_id
            })
    }

    fn remove_member(&mut self, peer_id: &str) {
        if let Some(membership) = self.memberships.remove(peer_id) {
            if let Some(room) = self.rooms.get_mut(&membership.room_id) {
                if let Some(member) = room.members.remove(peer_id) {
                    member.stop.send_replace(true);
                }
                room.broadcast(ServerMessage::PeerLeft {
                    peer_id: peer_id.to_owned(),
                });
            }
        }
    }

    fn cleanup(&mut self, now: Instant) {
        // The hot signaling path need not scan all devices/rooms until an expiry is due.
        if self.next_expiry.is_none_or(|next| now < next) {
            return;
        }
        let expired_peers: Vec<String> = self
            .devices
            .values()
            .filter(|device| device.expires_at <= now)
            .map(|device| device.peer_id.clone())
            .collect();
        self.devices.retain(|_, device| device.expires_at > now);
        for peer_id in expired_peers {
            self.remove_member(&peer_id);
        }
        let expired_rooms: Vec<String> = self
            .rooms
            .iter()
            .filter(|(_, room)| room.expires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for room_id in expired_rooms {
            if let Some(room) = self.rooms.remove(&room_id) {
                for (peer_id, member) in room.members {
                    member.stop.send_replace(true);
                    self.memberships.remove(&peer_id);
                }
            }
        }
        self.next_expiry = self
            .devices
            .values()
            .map(|device| device.expires_at)
            .chain(self.rooms.values().map(|room| room.expires_at))
            .min();
    }
}

impl Room {
    fn broadcast(&self, message: ServerMessage) {
        for member in self.members.values() {
            let _ = member.send(message.clone());
        }
    }
}

impl Member {
    fn send(&self, message: ServerMessage) -> Result<(), ServiceError> {
        self.sender.try_send(message).map_err(|_| {
            self.stop.send_replace(true);
            ServiceError::SlowConsumer
        })
    }
}

fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}
fn random_secret(length: usize) -> String {
    let mut bytes = vec![0; length];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
