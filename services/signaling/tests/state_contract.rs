use std::time::{Duration, Instant};

use parentview_signaling::{
    config::Config,
    models::{ClientMessage, DeviceCredentials, ServerMessage, ServiceError},
    state::{AppState, Membership},
};
use serde_json::json;
use tokio::sync::{mpsc, watch};

type Joined = (
    Membership,
    mpsc::Receiver<ServerMessage>,
    watch::Receiver<bool>,
);

fn join(
    state: &AppState,
    device: &DeviceCredentials,
    room: &(String, String, String),
    now: Instant,
) -> Result<Joined, ServiceError> {
    let (tx, rx) = mpsc::channel(state.config.queue_capacity);
    let (stop_tx, stop_rx) = watch::channel(false);
    state
        .join(&device.device_token, &room.0, &room.1, tx, stop_tx, now)
        .map(|membership| (membership, rx, stop_rx))
}

#[test]
fn tokens_are_random_and_bounded_devices_expire() {
    let now = Instant::now();
    let state = AppState::new(Config {
        max_devices: 2,
        device_ttl: Duration::from_secs(1),
        ..Config::default()
    })
    .unwrap();
    let first = state.create_device(now).unwrap();
    let second = state.create_device(now).unwrap();
    assert_ne!(first.device_token, second.device_token);
    assert_ne!(first.peer_id, second.peer_id);
    assert!(first.device_token.len() >= 43);
    assert_eq!(
        state.create_device(now).unwrap_err(),
        ServiceError::Capacity
    );
    state.create_device(now + Duration::from_secs(1)).unwrap();
    assert_eq!(
        state
            .create_room(&first.device_token, now + Duration::from_secs(1))
            .unwrap_err(),
        ServiceError::Unauthorized
    );
}

#[test]
fn room_invitation_and_authenticated_device_are_both_required() {
    let now = Instant::now();
    let state = AppState::new(Config::default()).unwrap();
    let device = state.create_device(now).unwrap();
    assert_eq!(
        state.create_room(&device.peer_id, now).unwrap_err(),
        ServiceError::Unauthorized
    );
    let room = state.create_room(&device.device_token, now).unwrap();
    let bad_device = DeviceCredentials {
        peer_id: device.peer_id.clone(),
        device_token: device.peer_id.clone(),
    };
    assert_eq!(
        join(&state, &bad_device, &room, now).unwrap_err(),
        ServiceError::Unauthorized
    );
    let bad_room = (room.0.clone(), room.0.clone(), room.2.clone());
    assert_eq!(
        join(&state, &device, &bad_room, now).unwrap_err(),
        ServiceError::RoomUnavailable
    );
    let missing_room = ("missing".into(), room.1.clone(), room.2.clone());
    assert_eq!(
        join(&state, &device, &missing_room, now).unwrap_err(),
        ServiceError::RoomUnavailable
    );
}

#[test]
fn four_peers_max_and_one_membership_per_device() {
    let now = Instant::now();
    let state = AppState::new(Config::default()).unwrap();
    let devices: Vec<_> = (0..5).map(|_| state.create_device(now).unwrap()).collect();
    let room = state.create_room(&devices[0].device_token, now).unwrap();
    let other_room = state.create_room(&devices[0].device_token, now).unwrap();
    let mut joined = Vec::new();
    for device in devices.iter().take(4) {
        joined.push(join(&state, device, &room, now).unwrap());
    }
    assert_eq!(
        join(&state, &devices[0], &other_room, now).unwrap_err(),
        ServiceError::AlreadyJoined
    );
    assert_eq!(
        join(&state, &devices[4], &room, now).unwrap_err(),
        ServiceError::RoomFull
    );
    state.leave(&joined[0].0);
    join(&state, &devices[4], &room, now).unwrap();
}

#[test]
fn signals_use_membership_sender_and_cannot_cross_rooms() {
    let now = Instant::now();
    let state = AppState::new(Config::default()).unwrap();
    let a = state.create_device(now).unwrap();
    let b = state.create_device(now).unwrap();
    let c = state.create_device(now).unwrap();
    let room = state.create_room(&a.device_token, now).unwrap();
    let other = state.create_room(&c.device_token, now).unwrap();
    let (a_lease, _a_rx, _) = join(&state, &a, &room, now).unwrap();
    let (_b_lease, mut b_rx, _) = join(&state, &b, &room, now).unwrap();
    let (_c_lease, _c_rx, _) = join(&state, &c, &other, now).unwrap();
    assert!(matches!(
        b_rx.try_recv().unwrap(),
        ServerMessage::Joined { .. }
    ));
    let payload = json!({"type": "offer", "sdp": "opaque", "from": "untrusted payload data"});
    state
        .route_signal(&a_lease, &b.peer_id, payload.clone(), now)
        .unwrap();
    assert_eq!(
        b_rx.try_recv().unwrap(),
        ServerMessage::Signal {
            from: a.peer_id.clone(),
            payload
        }
    );
    assert_eq!(
        state.route_signal(&a_lease, &c.peer_id, json!({}), now),
        Err(ServiceError::PeerUnavailable)
    );
    state.leave(&a_lease);
    assert_eq!(
        state.route_signal(&a_lease, &b.peer_id, json!({}), now),
        Err(ServiceError::Unauthorized)
    );
}

#[test]
fn spoofed_envelope_identity_is_rejected() {
    assert!(serde_json::from_value::<ClientMessage>(
        json!({"type":"signal","to":"peer","from":"forged","payload":{}})
    )
    .is_err());
    assert!(serde_json::from_value::<ClientMessage>(json!({"type":"join","deviceToken":"token","roomId":"id","roomToken":"secret","peerId":"forged"})).is_err());
}

#[test]
fn leave_notifies_remaining_peer_and_old_lease_cannot_remove_rejoin() {
    let now = Instant::now();
    let state = AppState::new(Config::default()).unwrap();
    let a = state.create_device(now).unwrap();
    let b = state.create_device(now).unwrap();
    let room = state.create_room(&a.device_token, now).unwrap();
    let (old_lease, _a_rx, a_stop) = join(&state, &a, &room, now).unwrap();
    let (_, mut b_rx, _) = join(&state, &b, &room, now).unwrap();
    b_rx.try_recv().unwrap();
    state.leave(&old_lease);
    assert!(*a_stop.borrow());
    assert_eq!(
        b_rx.try_recv().unwrap(),
        ServerMessage::PeerLeft {
            peer_id: a.peer_id.clone()
        }
    );
    let (new_lease, _rx, _) = join(&state, &a, &room, now).unwrap();
    state.leave(&old_lease);
    state
        .route_signal(&new_lease, &b.peer_id, json!({}), now)
        .unwrap();
}

#[test]
fn expired_room_disconnects_members_and_releases_capacity() {
    let now = Instant::now();
    let state = AppState::new(Config {
        max_rooms: 1,
        room_ttl: Duration::from_secs(1),
        ..Config::default()
    })
    .unwrap();
    let a = state.create_device(now).unwrap();
    let room = state.create_room(&a.device_token, now).unwrap();
    let (lease, _rx, stop) = join(&state, &a, &room, now).unwrap();
    assert_eq!(
        state.create_room(&a.device_token, now).unwrap_err(),
        ServiceError::Capacity
    );
    state.cleanup(now + Duration::from_secs(1));
    assert!(*stop.borrow());
    assert_eq!(
        state.route_signal(&lease, &a.peer_id, json!({}), now + Duration::from_secs(1)),
        Err(ServiceError::Unauthorized)
    );
    state
        .create_room(&a.device_token, now + Duration::from_secs(1))
        .unwrap();
}

#[test]
fn expired_device_disconnects_and_notifies_room_peer() {
    let now = Instant::now();
    let state = AppState::new(Config {
        device_ttl: Duration::from_secs(2),
        ..Config::default()
    })
    .unwrap();
    let a = state.create_device(now).unwrap();
    let room = state.create_room(&a.device_token, now).unwrap();
    let (lease, _rx, stop) = join(&state, &a, &room, now).unwrap();
    let b = state.create_device(now + Duration::from_secs(1)).unwrap();
    let (_, mut b_rx, _) = join(&state, &b, &room, now + Duration::from_secs(1)).unwrap();
    b_rx.try_recv().unwrap();
    state.cleanup(now + Duration::from_secs(2));
    assert!(*stop.borrow());
    assert_eq!(
        b_rx.try_recv().unwrap(),
        ServerMessage::PeerLeft {
            peer_id: lease.peer_id
        }
    );
}

#[test]
fn full_outbound_queue_disconnects_slow_consumer() {
    let now = Instant::now();
    let state = AppState::new(Config {
        queue_capacity: 2,
        ..Config::default()
    })
    .unwrap();
    let a = state.create_device(now).unwrap();
    let room = state.create_room(&a.device_token, now).unwrap();
    let (lease, _rx, stop) = join(&state, &a, &room, now).unwrap();
    state
        .route_signal(&lease, &a.peer_id, json!({}), now)
        .unwrap();
    assert_eq!(
        state.route_signal(&lease, &a.peer_id, json!({}), now),
        Err(ServiceError::SlowConsumer)
    );
    assert!(*stop.borrow());
}
