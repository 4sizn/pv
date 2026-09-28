use std::time::{Duration, Instant};

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use futures_util::{SinkExt, StreamExt};
use parentview_signaling::{
    config::Config,
    models::{DeviceCredentials, ServerMessage},
    router, AppState,
};
use serde_json::{json, Value};
use tokio::{net::TcpStream, task::JoinHandle, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
use tower::ServiceExt;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Server {
    state: AppState,
    url: String,
    task: JoinHandle<()>,
}
impl Server {
    async fn start(config: Config) -> Self {
        let state = AppState::new(config).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        let app = router(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { state, url, task }
    }
    fn device(&self) -> DeviceCredentials {
        self.state.create_device(Instant::now()).unwrap()
    }
    fn room(&self, device: &DeviceCredentials) -> (String, String, String) {
        self.state
            .create_room(&device.device_token, Instant::now())
            .unwrap()
    }
    async fn join(
        &self,
        device: &DeviceCredentials,
        room: &(String, String, String),
    ) -> (Socket, ServerMessage) {
        let (mut socket, _) = connect_async(&self.url).await.unwrap();
        transmit(&mut socket, json!({"type":"join","deviceToken":device.device_token,"roomId":room.0,"roomToken":room.1})).await;
        let response = receive(&mut socket).await;
        (socket, response)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.disconnect_all();
        self.task.abort();
    }
}

async fn transmit(socket: &mut Socket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}
async fn receive(socket: &mut Socket) -> ServerMessage {
    let message = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}
fn assert_error(message: ServerMessage, expected: &str) {
    assert!(matches!(message, ServerMessage::Error { code, .. } if code == expected));
}

#[tokio::test]
async fn http_device_authentication_and_explicit_cors() {
    let state = AppState::new(Config::default()).unwrap();
    let app = router(state);
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/rooms")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let evil = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/devices")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(evil.status(), StatusCode::FORBIDDEN);
    let device = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/devices")
                .header("origin", "http://localhost:1420")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(device.status(), StatusCode::CREATED);
    assert_eq!(
        device.headers()["access-control-allow-origin"],
        "http://localhost:1420"
    );
    let credentials: DeviceCredentials =
        serde_json::from_slice(&to_bytes(device.into_body(), 4096).await.unwrap()).unwrap();
    let room = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/rooms")
                .header(
                    "authorization",
                    format!("Bearer {}", credentials.device_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(room.status(), StatusCode::CREATED);
    let body: Value =
        serde_json::from_slice(&to_bytes(room.into_body(), 4096).await.unwrap()).unwrap();
    assert!(body["roomToken"].as_str().unwrap().len() >= 43);
    assert_eq!(body["iceServers"], json!([]));
}

#[tokio::test]
async fn websocket_rejects_credentials_and_wrong_origin() {
    let server = Server::start(Config::default()).await;
    let device = server.device();
    let mut room = server.room(&device);
    room.1 = "wrong-token".into();
    let (mut socket, response) = server.join(&device, &room).await;
    assert_error(response, "room-unavailable");
    assert!(matches!(socket.next().await, Some(Ok(Message::Close(_)))));
    let mut request = server.url.clone().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    assert!(connect_async(request).await.is_err());
    let bad = DeviceCredentials {
        peer_id: device.peer_id.clone(),
        device_token: device.peer_id,
    };
    assert_error(server.join(&bad, &room).await.1, "unauthorized");
}

#[tokio::test]
async fn real_sockets_route_authoritative_sender_reject_spoof_and_cleanup() {
    let server = Server::start(Config::default()).await;
    let a = server.device();
    let b = server.device();
    let room = server.room(&a);
    let (mut a_socket, a_joined) = server.join(&a, &room).await;
    assert_eq!(
        a_joined,
        ServerMessage::Joined {
            peer_id: a.peer_id.clone(),
            peers: vec![]
        }
    );
    let (mut b_socket, b_joined) = server.join(&b, &room).await;
    assert_eq!(
        b_joined,
        ServerMessage::Joined {
            peer_id: b.peer_id.clone(),
            peers: vec![a.peer_id.clone()]
        }
    );
    assert_eq!(
        receive(&mut a_socket).await,
        ServerMessage::PeerJoined {
            peer_id: b.peer_id.clone()
        }
    );
    transmit(
        &mut a_socket,
        json!({"type":"signal","to":b.peer_id,"payload":{"sdp":"opaque"}}),
    )
    .await;
    assert_eq!(
        receive(&mut b_socket).await,
        ServerMessage::Signal {
            from: a.peer_id.clone(),
            payload: json!({"sdp":"opaque"})
        }
    );
    transmit(&mut a_socket, json!({"type":"ping"})).await;
    assert_eq!(receive(&mut a_socket).await, ServerMessage::Pong);
    transmit(
        &mut a_socket,
        json!({"type":"signal","to":b.peer_id,"from":"forged","payload":{}}),
    )
    .await;
    assert_error(receive(&mut a_socket).await, "invalid-message");
    assert_eq!(
        receive(&mut b_socket).await,
        ServerMessage::PeerLeft {
            peer_id: a.peer_id.clone()
        }
    );
    let (_rejoined, message) = server.join(&a, &room).await;
    assert!(matches!(message, ServerMessage::Joined { .. }));
}

#[tokio::test]
async fn real_sockets_enforce_four_peer_capacity_and_global_duplicate_membership() {
    let server = Server::start(Config::default()).await;
    let devices: Vec<_> = (0..5).map(|_| server.device()).collect();
    let room = server.room(&devices[0]);
    let mut sockets = Vec::new();
    for device in devices.iter().take(4) {
        let (socket, response) = server.join(device, &room).await;
        assert!(matches!(response, ServerMessage::Joined { .. }));
        sockets.push(socket);
    }
    assert_error(server.join(&devices[4], &room).await.1, "room-full");
    let other = server.room(&devices[0]);
    assert_error(server.join(&devices[0], &other).await.1, "already-joined");
}

#[tokio::test]
async fn join_deadline_idle_deadline_and_connection_limit_are_enforced() {
    let config = Config {
        max_connections: 1,
        join_timeout: Duration::from_millis(80),
        heartbeat_timeout: Duration::from_millis(80),
        ..Config::default()
    };
    let server = Server::start(config).await;
    let (mut unjoined, _) = connect_async(&server.url).await.unwrap();
    assert!(connect_async(&server.url).await.is_err());
    assert_error(receive(&mut unjoined).await, "join-required");
    // The server frees the permit after sending the bounded close frame.
    while let Some(message) = unjoined.next().await {
        if matches!(message, Ok(Message::Close(_))) {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let device = server.device();
    let room = server.room(&device);
    let (mut socket, _) = server.join(&device, &room).await;
    assert!(matches!(
        timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap(),
        Some(Ok(Message::Close(_)))
    ));
}

#[tokio::test]
async fn oversized_websocket_message_disconnects_and_cleans_membership() {
    let server = Server::start(Config {
        max_message_bytes: 512,
        ..Config::default()
    })
    .await;
    let a = server.device();
    let b = server.device();
    let room = server.room(&a);
    let (mut a_socket, _) = server.join(&a, &room).await;
    let (mut b_socket, _) = server.join(&b, &room).await;
    receive(&mut a_socket).await;
    transmit(
        &mut a_socket,
        json!({"type":"signal","to":b.peer_id,"payload":"x".repeat(1024)}),
    )
    .await;
    assert_eq!(
        receive(&mut b_socket).await,
        ServerMessage::PeerLeft { peer_id: a.peer_id }
    );
}

#[tokio::test]
async fn packaged_tauri_origins_receive_explicit_http_cors_and_authenticated_room_access() {
    let app = router(AppState::new(Config::default()).unwrap());
    for origin in ["tauri://localhost", "http://tauri.localhost"] {
        let preflight = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/rooms")
                    .header("origin", origin)
                    .header("access-control-request-method", "POST")
                    .header("access-control-request-headers", "authorization")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preflight.headers()["access-control-allow-origin"], origin);
        assert_eq!(
            preflight.headers()["access-control-allow-headers"],
            "authorization,content-type"
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/devices")
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()["access-control-allow-origin"], origin);
        let device: DeviceCredentials =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let room = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rooms")
                    .header("origin", origin)
                    .header("authorization", format!("Bearer {}", device.device_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(room.status(), StatusCode::CREATED);
        assert_eq!(room.headers()["access-control-allow-origin"], origin);
    }
    for origin in [
        "tauri://evil.example",
        "custom://localhost",
        "http://tauri.localhost.evil.example",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/devices")
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
    }
}

#[tokio::test]
async fn packaged_tauri_websockets_still_require_credentials_and_reject_lookalike_origins() {
    let server = Server::start(Config::default()).await;
    for origin in ["tauri://localhost", "http://tauri.localhost"] {
        let device = server.device();
        let room = server.room(&device);
        let mut request = server.url.clone().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        transmit(&mut socket, json!({"type":"join","deviceToken":device.device_token,"roomId":room.0,"roomToken":room.1})).await;
        assert!(
            matches!(receive(&mut socket).await, ServerMessage::Joined { peer_id, .. } if peer_id == device.peer_id)
        );
        socket.close(None).await.unwrap();

        let mut request = server.url.clone().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        transmit(
            &mut socket,
            json!({"type":"join","deviceToken":"invalid","roomId":room.0,"roomToken":room.1}),
        )
        .await;
        assert_error(receive(&mut socket).await, "unauthorized");
    }
    for origin in [
        "tauri://evil.example",
        "custom://localhost",
        "http://tauri.localhost.evil.example",
    ] {
        let mut request = server.url.clone().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        assert!(connect_async(request).await.is_err());
    }
}
