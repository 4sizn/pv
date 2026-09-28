//! macOS process-level native resource regression. This standalone binary has
//! exactly one test, so other integration tests cannot skew its OS counters.
#![cfg(target_os = "macos")]
// Each integration binary uses a different subset of the shared test fixtures.
#[allow(dead_code)]
mod support;

use libwebrtc::{
    media_stream_track::MediaStreamTrack,
    peer_connection_factory::{
        native::PeerConnectionFactoryExt, PeerConnectionFactory, RtcConfiguration,
    },
    video_source::{native::NativeVideoSource, VideoResolution},
    video_stream::native::NativeVideoStream,
};
use pv_media_libwebrtc::{I420Buffer, LibWebRtcFactory, VideoFrame, VideoRotation};
use pv_media_native::{create_client_with_engine, SourceKind, State};
use std::{
    collections::BTreeSet,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use support::media::{decoded, publish_all, ready, synthetic, Server};
use tokio::{
    net::TcpListener,
    time::{sleep, timeout, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Resources {
    threads: usize,
    fds: usize,
}

fn resources() -> Resources {
    let pid = std::process::id().to_string();
    let threads = Command::new("/bin/ps")
        .args(["-M", "-p", &pid])
        .env("LC_ALL", "C")
        .output()
        .expect("macOS ps -M");
    assert!(threads.status.success(), "ps failed: {:?}", threads.stderr);
    let threads = String::from_utf8(threads.stdout)
        .unwrap()
        .lines()
        .filter(|line| {
            // ps prints USER only on the first thread row; continuation rows
            // start with PID after whitespace is split.
            let mut fields = line.split_whitespace();
            fields.next() == Some(pid.as_str()) || fields.next() == Some(pid.as_str())
        })
        .count();
    assert!(threads > 0, "ps must enumerate this process's threads");
    let files = Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &pid, "-d", "0-999999", "-Ff"])
        .env("LC_ALL", "C")
        .output()
        .expect("macOS lsof");
    assert!(files.status.success(), "lsof failed: {:?}", files.stderr);
    let fds = String::from_utf8(files.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            line.strip_prefix('f')
                .and_then(|fd| fd.parse::<usize>().ok())
        })
        .collect::<BTreeSet<_>>()
        .len();
    assert!(fds >= 3, "lsof must enumerate numeric descriptors");
    Resources { threads, fds }
}

async fn stable_baseline() -> Resources {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut previous = resources();
    let mut stable = 0;
    loop {
        sleep(Duration::from_millis(100)).await;
        let current = resources();
        stable = if current == previous { stable + 1 } else { 0 };
        if stable >= 2 {
            return current;
        }
        assert!(
            Instant::now() < deadline,
            "unstable warm baseline: {previous:?} -> {current:?}"
        );
        previous = current;
    }
}

async fn settled(baseline: Resources, label: &str) -> Resources {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stable = 0;
    loop {
        let current = resources();
        // One platform/dispatch helper thread and two descriptor transients are
        // tolerated. A retained native runtime owns at least three threads.
        if current.threads <= baseline.threads + 1 && current.fds <= baseline.fds + 2 {
            stable += 1;
            if stable >= 3 {
                return current;
            }
        } else {
            stable = 0;
        }
        assert!(
            Instant::now() < deadline,
            "native resource retention after {label}: baseline={baseline:?}, current={current:?}"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

async fn successful_use() -> Resources {
    let server = Server::start().await;
    let options = server.options();
    let factory = Arc::new(LibWebRtcFactory::default());
    let a = create_client_with_engine(factory.clone()).unwrap();
    let b = create_client_with_engine(factory.clone()).unwrap();
    a.join(options[0].clone()).await.unwrap();
    b.join(options[1].clone()).await.unwrap();
    ready(&a, 1).await;
    ready(&b, 1).await;
    let closed = Arc::new(AtomicUsize::new(0));
    publish_all(&a, &factory, "resources", &closed, 50).await;
    decoded(&b, None).await; // actual RTP decode activates source, codec and sink paths
    let active = resources();
    a.destroy().await.unwrap();
    b.destroy().await.unwrap();
    assert_eq!(closed.load(Ordering::SeqCst), 3);
    drop(a);
    drop(b);
    drop(factory);
    drop(server);
    active
}

async fn rejected_use() -> Resources {
    let server = Server::start().await;
    let mut options = server.options().remove(0);
    options.device_token = "deliberately-invalid-resource-test-token".into();
    let factory = Arc::new(LibWebRtcFactory::default());
    let client = create_client_with_engine(factory.clone()).unwrap();
    assert_eq!(client.join(options).await.unwrap_err().code, "unauthorized");
    let closed = Arc::new(AtomicUsize::new(0));
    let source = synthetic(&factory, SourceKind::Camera, 60, closed.clone());
    assert!(client.publish("rejected".into(), source).await.is_err());
    assert_eq!(
        closed.load(Ordering::SeqCst),
        1,
        "rejected source producer joined"
    );
    let active = resources();
    client.destroy().await.unwrap();
    drop(client);
    drop(factory);
    drop(server);
    active
}

async fn cancelled_use() -> Resources {
    let server = Server::start().await;
    let mut options = server.options().remove(0);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    options.signaling_url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let factory = Arc::new(LibWebRtcFactory::default());
    let client = create_client_with_engine(factory.clone()).unwrap();
    let joining_client = client.clone();
    let joining = tokio::spawn(async move { joining_client.join(options).await });
    // Keep a real accepted socket open without completing the WebSocket
    // handshake, then cancel the owned in-flight connector through destroy.
    let (pending_socket, _) = timeout(Duration::from_secs(3), listener.accept())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(client.snapshot().state, State::Joining);
    let active = resources();
    client.destroy().await.unwrap();
    assert!(joining.await.unwrap().is_err());
    drop(pending_socket);
    drop(listener);
    drop(client);
    drop(factory);
    drop(server);
    active
}

// Returns only a sink. All explicit factories, peers, tracks and sources are
// dropped. The sink's native track still retains RtcRuntime: this is a positive
// control demonstrating the resource probe would detect that ownership leak.
fn retained_sink_after_partial_initialization() -> NativeVideoStream {
    let factory = PeerConnectionFactory::default();
    let peer = factory
        .create_peer_connection(RtcConfiguration::default())
        .unwrap();
    let source = NativeVideoSource::new_without_keepalive(
        VideoResolution {
            width: 320,
            height: 180,
        },
        false,
    );
    let track = factory.create_video_track("resource-partial-track", source.clone());
    let stream = NativeVideoStream::new(track.clone());
    let sender = peer
        .add_track(MediaStreamTrack::Video(track.clone()), &["resources"])
        .unwrap();
    let frame = VideoFrame::new(
        VideoRotation::VideoRotation0,
        I420Buffer::new_black(320, 180),
    );
    source.capture_frame(&frame);
    // Abort before SDP or ICE completes; exercise Drop cleanup of the native
    // peer graph rather than relying solely on successful-session destroy.
    drop(sender);
    drop(peer);
    drop(track);
    drop(source);
    drop(factory);
    stream
}

async fn partial_use() -> Resources {
    let sink = retained_sink_after_partial_initialization();
    let active = resources();
    drop(sink); // Drop must synchronously unregister the observer and release the track
    active
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_native_lifetimes_release_threads_and_descriptors() {
    timeout(Duration::from_secs(90), async {
        // Warm the engine, codecs, process globals and each failure path once.
        successful_use().await;
        rejected_use().await;
        cancelled_use().await;
        partial_use().await;
        let baseline = stable_baseline().await;
        eprintln!("native resource warm baseline: {baseline:?}");

        let mut retained = retained_sink_after_partial_initialization();
        let held = resources();
        // Sampling a second time proves retention, rather than destructor lag.
        sleep(Duration::from_millis(150)).await;
        let held_again = resources();
        retained.close();
        drop(retained);
        let released = settled(baseline, "retained-sink positive control").await;
        assert!(held.threads.min(held_again.threads) >= released.threads + 3, "probe did not see retained native sink/runtime: held={held:?}, held_again={held_again:?}, released={released:?}");
        eprintln!("retained sink control: active={held:?}, held_again={held_again:?}, released={released:?}");

        let mut returned = Vec::new();
        let mut idle_floor = Resources { threads: baseline.threads.min(released.threads), fds: baseline.fds.min(released.fds) };
        for round in 0..3 {
            for path in ["decoded success", "rejected admission/publication", "cancelled handshake", "partial initialization"] {
                let active = match path {
                    "decoded success" => successful_use().await,
                    "rejected admission/publication" => rejected_use().await,
                    "cancelled handshake" => cancelled_use().await,
                    _ => partial_use().await,
                };
                let after = settled(idle_floor, path).await;
                assert!(active.threads >= after.threads + 3, "native runtime threads did not disappear for {path}: active={active:?}, after={after:?}");
                // Process-global helper pools can retire idle threads after the
                // warm sample. Tighten, never raise, the allowed idle baseline.
                idle_floor.threads = idle_floor.threads.min(after.threads);
                idle_floor.fds = idle_floor.fds.min(after.fds);
                eprintln!("native resources round={round} path={path}: active={active:?}, after={after:?}");
                returned.push(after);
            }
        }
        // Detect a sustained per-iteration rise, including small FD leaks that
        // would otherwise fit within the per-observation tolerance initially.
        for window in returned.windows(3) {
            assert!(!window.windows(2).all(|pair| pair[1].threads > pair[0].threads), "monotonic native thread growth: {returned:?}");
            assert!(!window.windows(2).all(|pair| pair[1].fds > pair[0].fds), "monotonic descriptor growth: {returned:?}");
        }
        let first = returned.first().unwrap();
        let last = returned.last().unwrap();
        assert!(last.threads <= first.threads + 1 && last.fds <= first.fds + 1, "resource totals drifted across repeated lifetimes: {returned:?}");
    }).await.expect("native lifecycle resource deadline");
}
