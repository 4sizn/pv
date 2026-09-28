//! Actual bidirectional native RTP encode/decode with caller-owned synthetic
//! producers. No browser, physical capture, speaker, renderer, STUN or TURN.
mod support;
use pv_media_libwebrtc::{I420Buffer, LibWebRtcFactory, VideoFrame, VideoRotation};
use pv_media_native::{create_client_with_engine, SourceKind, SourcePort, State};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use support::media::*;
use tokio::time::{sleep, timeout};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_fixed_slots_decode_bidirectional_media_detach_republish_and_rejoin() {
    timeout(Duration::from_secs(75), async {
        let server = Server::start().await;
        let options = server.options();
        let factory = Arc::new(LibWebRtcFactory::default());
        let exchanges = Arc::new(AtomicUsize::new(0));
        let engine = Arc::new(CountedFactory {
            engine: factory.clone(),
            exchanges: exchanges.clone(),
        });
        let a = create_client_with_engine(engine.clone()).unwrap();
        let b = create_client_with_engine(engine).unwrap();
        a.join(options[0].clone()).await.unwrap();
        b.join(options[1].clone()).await.unwrap();
        ready(&a, 1).await;
        ready(&b, 1).await;
        assert_eq!(exchanges.load(Ordering::SeqCst), 2);
        let empty_a = observations(&a).await;
        let empty_b = observations(&b).await;
        assert_eq!(empty_a.len(), 3);
        assert_eq!(
            empty_a
                .iter()
                .map(|(kind, item)| (*kind, &item.mid))
                .collect::<Vec<_>>(),
            empty_b
                .iter()
                .map(|(kind, item)| (*kind, &item.mid))
                .collect::<Vec<_>>()
        );
        assert!(empty_a.values().all(|item| item.frames_decoded == 0
            && item.bytes_received == 0
            && item.audio_energy == 0));

        let closed = Arc::new(AtomicUsize::new(0));
        publish_all(&a, &factory, "a", &closed, 35).await;
        publish_all(&b, &factory, "b", &closed, 55).await;
        remote_sources(&a, "b-", 3).await;
        remote_sources(&b, "a-", 3).await;
        let received_a = decoded(&a, None).await;
        let received_b = decoded(&b, None).await;
        assert_ne!(
            received_b[&SourceKind::Camera].content_signature,
            received_b[&SourceKind::Screen].content_signature
        );
        // Both video slots must carry changing decoded content in both directions.
        changed_video(&a, &received_a).await;
        changed_video(&b, &received_b).await;
        for index in 0..3 {
            a.unpublish(format!("a-{index}")).await.unwrap();
        }
        assert_eq!(
            closed.load(Ordering::SeqCst),
            3,
            "unpublish joins every producer"
        );
        remote_sources(&b, "", 0).await;
        sleep(Duration::from_millis(700)).await;
        let stopped = observations(&b).await;
        sleep(Duration::from_millis(350)).await;
        let settled = observations(&b).await;
        for kind in [
            SourceKind::Camera,
            SourceKind::Screen,
            SourceKind::Microphone,
        ] {
            assert_eq!(
                stopped[&kind].bytes_received, settled[&kind].bytes_received,
                "RTP bytes after detach: {kind:?}"
            );
            assert_eq!(stopped[&kind].frames_decoded, settled[&kind].frames_decoded);
        }
        decoded(&a, Some(&received_a)).await;

        publish_all(&a, &factory, "replacement", &closed, 90).await;
        remote_sources(&b, "replacement-", 3).await;
        let resumed = decoded(&b, Some(&settled)).await;
        changed_video(&b, &settled).await;
        for (client, data) in [(&a, "data-under-av-a"), (&b, "data-under-av-b")] {
            let result = client.send(data.into()).await.unwrap();
            assert_eq!(result.accepted_peer_ids.len(), 1);
            assert!(result.failures.is_empty());
        }
        received_data(&a, "data-under-av-b").await;
        received_data(&b, "data-under-av-a").await;
        for kind in [
            SourceKind::Camera,
            SourceKind::Screen,
            SourceKind::Microphone,
        ] {
            assert_eq!(resumed[&kind].mid, settled[&kind].mid);
        }
        assert_eq!(
            exchanges.load(Ordering::SeqCst),
            2,
            "attach/detach/republish requires no SDP"
        );

        a.leave().await.unwrap();
        assert_eq!(
            closed.load(Ordering::SeqCst),
            6,
            "leave joins republished producers"
        );
        ready(&b, 0).await;
        assert!(b.snapshot().remote_sources.is_empty());
        a.join(options[0].clone()).await.unwrap();
        ready(&a, 1).await;
        ready(&b, 1).await;
        a.publish(
            "rejoined-camera".into(),
            synthetic(&factory, SourceKind::Camera, 110, closed.clone()),
        )
        .await
        .unwrap();
        remote_sources(&b, "rejoined-", 1).await;
        timeout(Duration::from_secs(10), async {
            loop {
                if observations(&b).await[&SourceKind::Camera].frames_decoded >= 2 {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("rejoined native video");
        assert_eq!(exchanges.load(Ordering::SeqCst), 4);
        assert_eq!(a.destroy().await.unwrap().state, State::Destroyed);
        assert_eq!(b.destroy().await.unwrap().state, State::Destroyed);
        assert_eq!(
            closed.load(Ordering::SeqCst),
            10,
            "all producer tasks joined"
        );
    })
    .await
    .expect("native media test deadline");
}

#[tokio::test]
async fn raw_empty_sender_and_unnegotiated_direction_are_safe_and_ingress_closes() {
    use libwebrtc::{
        peer_connection_factory::{PeerConnectionFactory, RtcConfiguration},
        rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit},
        MediaType,
    };
    let factory = PeerConnectionFactory::default();
    let pc = factory
        .create_peer_connection(RtcConfiguration::default())
        .unwrap();
    let slot = pc
        .add_transceiver_for_media(
            MediaType::Video,
            RtpTransceiverInit {
                direction: RtpTransceiverDirection::SendRecv,
                stream_ids: vec![],
                send_encodings: vec![],
            },
        )
        .unwrap();
    assert!(slot.mid().is_none());
    assert!(slot.current_direction().is_none());
    assert!(slot.sender().track().is_none());
    slot.sender().set_track(None).unwrap();
    assert!(slot.sender().track().is_none());
    pc.close();
    drop(slot);
    drop(pc);
    drop(factory);

    let factory = LibWebRtcFactory::default();
    let (mut video, input) = factory.video_source(SourceKind::Camera, 320, 180).unwrap();
    assert_eq!(
        input
            .capture(VideoFrame::new(
                VideoRotation::VideoRotation0,
                I420Buffer::new_black(2, 2)
            ))
            .unwrap_err()
            .code,
        "invalid-frame"
    );
    video.close().await.unwrap();
    assert_eq!(
        input
            .capture(VideoFrame::new(
                VideoRotation::VideoRotation0,
                I420Buffer::new_black(320, 180)
            ))
            .unwrap_err()
            .code,
        "source-closed"
    );
    let (audio, input) = factory.audio_source().unwrap();
    assert_eq!(
        input.capture_10ms(&[0; 479]).unwrap_err().code,
        "invalid-frame"
    );
    input.capture_10ms(&[1; 480]).unwrap();
    drop(audio);
    assert_eq!(
        input.capture_10ms(&[1; 480]).unwrap_err().code,
        "source-closed"
    );
}
