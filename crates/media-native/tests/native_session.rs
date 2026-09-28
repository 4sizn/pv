//! Actual native DTLS/SCTP integration over the authenticated signaling service.
use parentview_signaling::{config::Config, AppState};
use pv_media_native::{create_client, Event, JoinOptions, NativeDataClient, State};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};

struct Server {
    state: AppState,
    url: String,
    task: JoinHandle<()>,
}
impl Server {
    async fn start() -> Self {
        let state = AppState::new(Config::default()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        let app = parentview_signaling::router(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { state, url, task }
    }
    fn options(&self, count: usize) -> Vec<JoinOptions> {
        let credentials: Vec<_> = (0..count)
            .map(|_| self.state.create_device(Instant::now()).unwrap())
            .collect();
        let (room_id, room_token, _) = self
            .state
            .create_room(&credentials[0].device_token, Instant::now())
            .unwrap();
        credentials
            .into_iter()
            .map(|device| JoinOptions {
                signaling_url: self.url.clone(),
                room_id: room_id.clone(),
                room_token: room_token.clone(),
                device_token: device.device_token,
                ice_servers: vec![],
            })
            .collect()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.disconnect_all();
        self.task.abort();
    }
}

async fn ready(client: &NativeDataClient, count: usize) {
    timeout(Duration::from_secs(15), async {
        loop {
            let snapshot = client.snapshot();
            if snapshot.peers.len() == count && snapshot.ready_peers.len() == count {
                return;
            }
            let batch = client.read_batch().await.unwrap();
            for event in batch.events {
                if let Event::Error { error } = event {
                    panic!("native readiness failed: {error}");
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("native readiness deadline: {:?}", client.snapshot()));
}
async fn message_set(client: &NativeDataClient, count: usize) -> BTreeSet<String> {
    timeout(Duration::from_secs(10), async {
        let mut messages = BTreeSet::new();
        while messages.len() < count {
            for event in client.read_batch().await.unwrap().events {
                match event {
                    Event::Message { data, .. } => {
                        messages.insert(data);
                    }
                    Event::Error { error } => panic!("native data failed: {error}"),
                }
            }
        }
        messages
    })
    .await
    .expect("native message deadline")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn four_native_peers_exchange_utf8_and_rejoin_without_browser_media() {
    timeout(Duration::from_secs(60), async {
        let server = Server::start().await;
        let options = server.options(4);
        let clients: Vec<_> = (0..4).map(|_| create_client().unwrap()).collect();
        for (client, options) in clients.iter().zip(&options) {
            assert_eq!(
                client.join(options.clone()).await.unwrap().state,
                State::Joined
            );
        }
        for client in &clients {
            ready(client, 3).await;
        }
        for (index, client) in clients.iter().enumerate() {
            let sent = client.send(format!("native-{index}-한글")).await.unwrap();
            assert_eq!(sent.accepted_peer_ids.len(), 3);
            assert!(sent.failures.is_empty());
        }
        for (index, client) in clients.iter().enumerate() {
            let expected = (0..4)
                .filter(|i| *i != index)
                .map(|i| format!("native-{i}-한글"))
                .collect();
            assert_eq!(message_set(client, 3).await, expected);
        }
        clients[3].leave().await.unwrap();
        clients[3].leave().await.unwrap();
        assert_eq!(clients[3].snapshot().state, State::Idle);
        for client in &clients[..3] {
            ready(client, 2).await;
        }
        clients[3].join(options[3].clone()).await.unwrap();
        for client in &clients {
            ready(client, 3).await;
        }
        assert_eq!(
            clients[3]
                .send("rejoined".into())
                .await
                .unwrap()
                .accepted_peer_ids
                .len(),
            3
        );
        for client in &clients[..3] {
            assert_eq!(
                message_set(client, 1).await,
                BTreeSet::from(["rejoined".into()])
            );
        }
        for client in &clients {
            assert_eq!(client.destroy().await.unwrap().state, State::Destroyed);
            assert_eq!(client.destroy().await.unwrap().state, State::Destroyed);
            assert_eq!(
                client.send("after shutdown".into()).await.unwrap_err().code,
                "destroyed"
            );
        }
    })
    .await
    .expect("native mesh test deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_credentials_release_native_client_for_a_valid_join() {
    timeout(Duration::from_secs(20), async {
        let server = Server::start().await;
        let options = server.options(1).pop().unwrap();
        let mut invalid = options.clone();
        invalid.device_token = "invalid-test-token".into();
        let client = create_client().unwrap();
        assert_eq!(client.join(invalid).await.unwrap_err().code, "unauthorized");
        assert_eq!(client.snapshot().state, State::Idle);
        assert_eq!(client.join(options).await.unwrap().state, State::Joined);
        client.destroy().await.unwrap();
    })
    .await
    .expect("admission test deadline");
}
