//! One socket lifecycle. Owns read/write deadlines, bounded outgoing queue and a membership
//! lease whose drop removes the member. No media interpretation or product authorization.
use std::time::Instant;

use axum::extract::ws::{Message, WebSocket};
use futures_util::SinkExt;
use tokio::{
    sync::{mpsc, watch},
    time::{sleep_until, timeout, Instant as TokioInstant},
};

use crate::{
    models::{ClientMessage, ServerMessage},
    state::{AppState, Membership},
};

struct MembershipLease {
    state: AppState,
    membership: Membership,
}
impl Drop for MembershipLease {
    fn drop(&mut self) {
        self.state.leave(&self.membership);
    }
}

pub async fn serve(mut socket: WebSocket, state: AppState) {
    let (sender, mut outgoing) = mpsc::channel(state.config.queue_capacity);
    let (stop, mut stopped) = watch::channel(false);
    let first = timeout(state.config.join_timeout, socket.recv()).await;
    let join = match first {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<ClientMessage>(&text).ok(),
        _ => None,
    };
    let Some(ClientMessage::Join {
        device_token,
        room_id,
        room_token,
    }) = join
    else {
        reject(
            &mut socket,
            &state,
            "join-required",
            "A valid join must be the first message before the deadline.",
        )
        .await;
        return;
    };
    let membership = match state.join(
        &device_token,
        &room_id,
        &room_token,
        sender,
        stop,
        Instant::now(),
    ) {
        Ok(membership) => membership,
        Err(error) => {
            let _ = send(&mut socket, &state, error.wire()).await;
            let _ = timeout(state.config.write_timeout, socket.close()).await;
            return;
        }
    };
    let lease = MembershipLease {
        state: state.clone(),
        membership,
    };
    let mut read_deadline = TokioInstant::now() + state.config.heartbeat_timeout;
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            _ = sleep_until(read_deadline) => break,
            outgoing_message = outgoing.recv() => {
                let Some(message) = outgoing_message else { break; };
                if !send(&mut socket, &state, message).await { break; }
            },
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break; };
                read_deadline = TokioInstant::now() + state.config.heartbeat_timeout;
                match message {
                    Message::Text(text) => {
                        if !handle_text(&mut socket, &state, &lease.membership, &text).await { break; }
                    },
                    Message::Ping(_) | Message::Pong(_) => {},
                    Message::Close(_) => break,
                    Message::Binary(_) => {
                        reject(&mut socket, &state, "invalid-message", "Only JSON text messages are supported.").await;
                        break;
                    },
                }
            }
        }
    }
    drop(lease);
    let _ = timeout(state.config.write_timeout, socket.close()).await;
}

async fn handle_text(
    socket: &mut WebSocket,
    state: &AppState,
    membership: &Membership,
    text: &str,
) -> bool {
    match serde_json::from_str::<ClientMessage>(text) {
        Ok(ClientMessage::Signal { to, payload }) => {
            match state.route_signal(membership, &to, payload, Instant::now()) {
                Ok(()) => true,
                Err(error) => send(socket, state, error.wire()).await,
            }
        }
        Ok(ClientMessage::Ping) => send(socket, state, ServerMessage::Pong).await,
        _ => {
            reject(
                socket,
                state,
                "invalid-message",
                "Invalid message envelope; join is only accepted once.",
            )
            .await;
            false
        }
    }
}

async fn reject(socket: &mut WebSocket, state: &AppState, code: &str, message: &str) {
    let _ = send(
        socket,
        state,
        ServerMessage::Error {
            code: code.into(),
            message: message.into(),
        },
    )
    .await;
    let _ = timeout(state.config.write_timeout, socket.close()).await;
}

async fn send(socket: &mut WebSocket, state: &AppState, message: ServerMessage) -> bool {
    let Ok(json) = serde_json::to_string(&message) else {
        return false;
    };
    matches!(
        timeout(
            state.config.write_timeout,
            socket.send(Message::Text(json.into()))
        )
        .await,
        Ok(Ok(()))
    )
}
