use futures_util::{SinkExt, StreamExt};
use pv_media_runtime::{ports::*, JoinOptions, NativeError, SignalPayload};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{timeout, Instant},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    MaybeTlsStream, WebSocketStream,
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_WIRE_BYTES: usize = 65_536;
pub struct WebSocketFactory;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ServerMessage {
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
        payload: SignalPayload,
    },
    Pong,
    Error {
        code: String,
    },
}
impl SignalingFactory for WebSocketFactory {
    fn connect(&self, options: JoinOptions, events: EventSink) -> PortFuture<'_, Connected> {
        Box::pin(async move {
            let url = url::Url::parse(&options.signaling_url).map_err(|_| invalid_url())?;
            if !matches!(url.scheme(), "ws" | "wss")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
                || url.query().is_some()
            {
                return Err(invalid_url());
            }
            let config = WebSocketConfig::default()
                .max_message_size(Some(MAX_WIRE_BYTES))
                .max_frame_size(Some(MAX_WIRE_BYTES))
                .max_write_buffer_size(262_144);
            let (mut socket, _) = connect_async_with_config(url.as_str(), Some(config), false)
                .await
                .map_err(|_| signaling_error("Signaling connection failed"))?;
            let join = json!({ "type": "join", "deviceToken": options.device_token, "roomId": options.room_id, "roomToken": options.room_token });
            write(&mut socket, Message::Text(join.to_string().into())).await?;
            let (peer_id, peers) = loop {
                match socket.next().await {
                    Some(Ok(Message::Text(text))) => match parse(&text)? {
                        ServerMessage::Joined { peer_id, peers } => break (peer_id, peers),
                        ServerMessage::Error { code } => return Err(server_error(&code)),
                        _ => {
                            return Err(signaling_error("Unexpected signaling handshake response"))
                        }
                    },
                    Some(Ok(Message::Ping(bytes))) => {
                        write(&mut socket, Message::Pong(bytes)).await?
                    }
                    _ => return Err(signaling_error("Signaling closed before admission")),
                }
            };
            let (sender, receiver) = mpsc::channel(64);
            let (stop, stop_rx) = watch::channel(false);
            let task = tokio::spawn(drive(socket, receiver, stop_rx, events));
            Ok(Connected {
                peer_id,
                peers,
                signaling: Box::new(WebSocketPort {
                    sender,
                    stop,
                    task: Some(task),
                }),
            })
        })
    }
}
struct WebSocketPort {
    sender: mpsc::Sender<Message>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}
impl SignalingPort for WebSocketPort {
    fn send(&mut self, to: &str, payload: SignalPayload) -> Result<(), NativeError> {
        let value = json!({ "type": "signal", "to": to, "payload": payload }).to_string();
        if value.len() > MAX_WIRE_BYTES {
            return Err(signaling_error("Signaling payload exceeds the byte limit"));
        }
        self.sender
            .try_send(Message::Text(value.into()))
            .map_err(|_| {
                NativeError::new(
                    "signaling-backpressure",
                    "Signaling write queue is unavailable",
                )
            })
    }
    fn close(&mut self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.stop.send_replace(true);
            // Keep the handle owned by self while awaiting: cancellation then Drop
            // can still abort it, rather than detach a socket task.
            let result = if let Some(task) = self.task.as_mut() {
                match timeout(Duration::from_millis(1500), &mut *task).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(_)) => Err(NativeError::new(
                        "cleanup-failed",
                        "Signaling task failed during cleanup",
                    )),
                    Err(_) => {
                        task.abort();
                        let _ = task.await;
                        Err(NativeError::new(
                            "cleanup-timeout",
                            "Signaling task required cancellation during cleanup",
                        ))
                    }
                }
            } else {
                Ok(())
            };
            self.task.take();
            result
        })
    }
}
impl Drop for WebSocketPort {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
async fn drive(
    mut socket: Socket,
    mut writes: mpsc::Receiver<Message>,
    mut stop: watch::Receiver<bool>,
    events: EventSink,
) {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut last_pong = Instant::now();
    let result: Result<(), NativeError> = async {
        loop {
            let outbound = tokio::select! {
                biased;
                _ = stop.changed() => break,
                message = socket.next() => {
                    match message {
                        Some(Ok(Message::Text(text))) => { match parse(&text)? {
                            ServerMessage::PeerJoined { peer_id } => { if !events.send(Input::Signaling(SignalingEvent::PeerJoined(peer_id))) { break; } }
                            ServerMessage::PeerLeft { peer_id } => { if !events.send(Input::Signaling(SignalingEvent::PeerLeft(peer_id))) { break; } }
                            ServerMessage::Signal { from, payload } => { if !events.send(Input::Signaling(SignalingEvent::Signal { from, payload })) { break; } }
                            ServerMessage::Pong => last_pong = Instant::now(),
                            ServerMessage::Error { code } => { if !events.send(Input::Signaling(SignalingEvent::Error(server_error(&code)))) { break; } }
                            ServerMessage::Joined { .. } => return Err(signaling_error("Duplicate signaling admission")),
                        }; None },
                        Some(Ok(Message::Ping(bytes))) => { Some(Message::Pong(bytes)) }
                        Some(Ok(Message::Pong(_))) => { last_pong = Instant::now(); None }
                        Some(Ok(Message::Close(_))) | None => return Err(NativeError::new("disconnected", "Signaling disconnected")),
                        _ => return Err(signaling_error("Invalid signaling frame")),
                    }
                }
                outbound = writes.recv() => match outbound { Some(message) => Some(message), None => break },
                _ = heartbeat.tick() => {
                    if last_pong.elapsed() >= Duration::from_secs(45) { return Err(NativeError::new("heartbeat-timeout", "Signaling heartbeat timed out")); }
                    Some(Message::Text("{\"type\":\"ping\"}".into()))
                }
            };
            if let Some(outbound) = outbound {
                tokio::select! { biased; _ = stop.changed() => break, result = write(&mut socket, outbound) => result? }
            }
        }
        Ok(())
    }.await;
    if let Err(error) = result {
        events.send(Input::Signaling(SignalingEvent::Error(error)));
    }
    let _ = timeout(Duration::from_secs(1), async {
        let _ = socket.close(None).await;
        while let Some(Ok(message)) = socket.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
}
async fn write(socket: &mut Socket, message: Message) -> Result<(), NativeError> {
    timeout(WRITE_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| signaling_error("Signaling write timed out"))?
        .map_err(|_| signaling_error("Signaling write failed"))
}
fn parse(text: &str) -> Result<ServerMessage, NativeError> {
    if text.len() > MAX_WIRE_BYTES {
        return Err(signaling_error("Signaling frame exceeds limits"));
    }
    serde_json::from_str(text).map_err(|_| signaling_error("Invalid signaling message"))
}
fn server_error(code: &str) -> NativeError {
    let code = match code {
        "unauthorized" | "room-unavailable" | "room-full" | "already-joined"
        | "peer-unavailable" | "capacity" | "slow-consumer" => code,
        _ => "signaling-rejected",
    };
    NativeError::new(code, "Signaling rejected the request")
}
fn signaling_error(message: &str) -> NativeError {
    NativeError::new("signaling", message)
}
fn invalid_url() -> NativeError {
    NativeError::new(
        "invalid-options",
        "Expected a ws/wss endpoint without user information, query, or fragment",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn malformed_wire_and_untrusted_server_errors_stay_safe() {
        assert!(parse(r#"{"type":"signal","from":"a","payload":{"type":"ice","candidate":{"candidate":true}}}"#).is_err());
        assert!(parse(r#"{"type":"unknown"}"#).is_err());
        assert!(parse(&" ".repeat(MAX_WIRE_BYTES + 1)).is_err());
        let error = server_error("secret-from-untrusted-server");
        assert_eq!(error.code, "signaling-rejected");
        assert!(!error.message.contains("secret"));
    }

    #[tokio::test]
    async fn socket_task_timeout_aborts_and_joins_before_reporting_cleanup_failure() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = Guard(flag);
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        let (sender, _receiver) = mpsc::channel(1);
        let (stop, _) = watch::channel(false);
        let mut port = WebSocketPort {
            sender,
            stop,
            task: Some(task),
        };
        assert_eq!(port.close().await.unwrap_err().code, "cleanup-timeout");
        assert!(dropped.load(Ordering::SeqCst));
        assert!(port.task.is_none());
        port.close().await.unwrap();
    }

    #[tokio::test]
    async fn socket_task_join_failure_is_not_reported_as_success() {
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        let (sender, _receiver) = mpsc::channel(1);
        let (stop, _) = watch::channel(false);
        let mut port = WebSocketPort {
            sender,
            stop,
            task: Some(task),
        };
        assert_eq!(port.close().await.unwrap_err().code, "cleanup-failed");
        assert!(port.task.is_none());
    }
}
