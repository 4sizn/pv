//! Raw libwebrtc adapter. Owns native handles and callback conversion only.
mod media;
mod source;
use libwebrtc::{
    data_channel::{DataChannel, DataChannelInit, DataChannelState},
    ice_candidate::IceCandidate,
    peer_connection::{AnswerOptions, OfferOptions, PeerConnection, PeerConnectionState},
    peer_connection_factory::{
        IceServer as EngineIceServer, PeerConnectionFactory, RtcConfiguration,
    },
    session_description::{SdpType, SessionDescription},
};
use pv_media_runtime::{
    ports::*, Candidate, Description, DescriptionType, IceServer, MediaObservation, MediaSlot,
    NativeError, SourceKind, MAX_DATA_BYTES,
};
pub use source::{AudioInput, I420Buffer, LibWebRtcSource, VideoFrame, VideoInput, VideoRotation};
use std::sync::{Arc, Mutex};

const MAX_BUFFERED_BYTES: u64 = 1_048_576;
#[derive(Default)]
pub struct LibWebRtcFactory {
    factory: PeerConnectionFactory,
}
impl EngineFactory for LibWebRtcFactory {
    fn create(
        &self,
        peer_id: &str,
        initiator: bool,
        ice: &[IceServer],
        events: EventSink,
    ) -> Result<Box<dyn PeerPort>, NativeError> {
        let mut config = RtcConfiguration::default();
        config.ice_servers = ice
            .iter()
            .map(|ice| EngineIceServer {
                urls: ice.urls.clone(),
                username: ice.username.clone().unwrap_or_default(),
                password: ice.credential.clone().unwrap_or_default(),
            })
            .collect();
        let connection = self
            .factory
            .create_peer_connection(config)
            .map_err(|_| engine_error("Could not create a native peer"))?;
        let channel = Arc::new(Mutex::new(None));
        let id = peer_id.to_owned();
        let sink = events.clone();
        connection.on_ice_candidate(Some(Box::new(move |candidate| {
            sink.send(Input::Engine {
                peer_id: id.clone(),
                event: EngineEvent::Candidate(Candidate {
                    candidate: candidate.candidate(),
                    sdp_mid: Some(candidate.sdp_mid()),
                    sdp_m_line_index: u16::try_from(candidate.sdp_mline_index()).ok(),
                }),
            });
        })));
        let id = peer_id.to_owned();
        let sink = events.clone();
        connection.on_connection_state_change(Some(Box::new(move |state| {
            if matches!(
                state,
                PeerConnectionState::Failed | PeerConnectionState::Closed
            ) {
                sink.send(Input::Engine {
                    peer_id: id.clone(),
                    event: EngineEvent::Failed,
                });
            }
        })));
        let slot = channel.clone();
        let id = peer_id.to_owned();
        let sink = events.clone();
        connection.on_data_channel(Some(Box::new(move |incoming| {
            let mut slot = slot.lock().expect("data channel lock");
            if incoming.label() != "pv-data" || slot.is_some() {
                sink.send(Input::Engine {
                    peer_id: id.clone(),
                    event: EngineEvent::Error(engine_error("Unexpected data channel")),
                });
                return;
            }
            attach_channel(&incoming, &id, &sink);
            *slot = Some(incoming);
        })));
        let mut peer = LibWebRtcPeer {
            connection,
            channel,
            closed: false,
            media: media::Media::new(),
            factory: self.factory.clone(),
        };
        if initiator {
            peer.media.reserve(&peer.connection, &self.factory)?;
            match peer
                .connection
                .create_data_channel("pv-data", DataChannelInit::default())
            {
                Ok(data) => {
                    attach_channel(&data, peer_id, &events);
                    *peer.channel.lock().expect("data channel lock") = Some(data);
                }
                Err(_) => {
                    peer.close();
                    return Err(engine_error("Could not create a data channel"));
                }
            }
        }
        Ok(Box::new(peer))
    }
}

fn attach_channel(channel: &DataChannel, peer_id: &str, events: &EventSink) {
    let id = peer_id.to_owned();
    let sink = events.clone();
    channel.on_state_change(Some(Box::new(move |state| {
        sink.send(Input::Engine {
            peer_id: id.clone(),
            event: EngineEvent::Ready(state == DataChannelState::Open),
        });
    })));
    let id = peer_id.to_owned();
    let sink = events.clone();
    channel.on_message(Some(Box::new(move |buffer| {
        let event = if buffer.binary || buffer.data.len() > MAX_DATA_BYTES {
            EngineEvent::Error(NativeError::new(
                "invalid-data",
                "Expected a bounded UTF-8 data message",
            ))
        } else {
            match std::str::from_utf8(buffer.data) {
                Ok(data) => EngineEvent::Message(data.to_owned()),
                Err(_) => EngineEvent::Error(NativeError::new(
                    "invalid-data",
                    "Expected a UTF-8 data message",
                )),
            }
        };
        sink.send(Input::Engine {
            peer_id: id.clone(),
            event,
        });
    })));
    if channel.state() == DataChannelState::Open {
        events.send(Input::Engine {
            peer_id: peer_id.to_owned(),
            event: EngineEvent::Ready(true),
        });
    }
}

struct LibWebRtcPeer {
    connection: PeerConnection,
    channel: Arc<Mutex<Option<DataChannel>>>,
    closed: bool,
    media: media::Media,
    factory: PeerConnectionFactory,
}
impl PeerPort for LibWebRtcPeer {
    fn offer(&mut self) -> PortFuture<'_, Description> {
        Box::pin(async move {
            let description = self
                .connection
                .create_offer(OfferOptions {
                    offer_to_receive_audio: true,
                    offer_to_receive_video: true,
                    ..OfferOptions::default()
                })
                .await
                .map_err(|_| engine_error("Offer creation failed"))?;
            self.connection
                .set_local_description(description.clone())
                .await
                .map_err(|_| engine_error("Local description failed"))?;
            Ok(Description {
                r#type: DescriptionType::Offer,
                sdp: description.to_string(),
                slots: self.media.descriptors(),
            })
        })
    }
    fn answer(&mut self, offer: Description) -> PortFuture<'_, Description> {
        Box::pin(async move {
            let offered_slots = offer.slots;
            media::validate_slots(&offered_slots)?;
            let offer = SessionDescription::parse(&offer.sdp, SdpType::Offer)
                .map_err(|_| engine_error("Invalid remote offer"))?;
            self.connection
                .set_remote_description(offer)
                .await
                .map_err(|_| engine_error("Remote description failed"))?;
            self.media
                .adopt(&self.connection, &self.factory, &offered_slots)?;
            let answer = self
                .connection
                .create_answer(AnswerOptions::default())
                .await
                .map_err(|_| engine_error("Answer creation failed"))?;
            self.connection
                .set_local_description(answer.clone())
                .await
                .map_err(|_| engine_error("Local description failed"))?;
            self.media.observe()?;
            Ok(Description {
                r#type: DescriptionType::Answer,
                sdp: answer.to_string(),
                slots: self.media.descriptors(),
            })
        })
    }
    fn accept_answer(&mut self, answer: Description) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let answered_slots = answer.slots;
            media::validate_slots(&answered_slots)?;
            if self.media.descriptors() != answered_slots {
                return Err(engine_error("Remote media slots differ from the offer"));
            }
            let answer = SessionDescription::parse(&answer.sdp, SdpType::Answer)
                .map_err(|_| engine_error("Invalid remote answer"))?;
            self.connection
                .set_remote_description(answer)
                .await
                .map_err(|_| engine_error("Remote description failed"))?;
            self.media.accept(&answered_slots)
        })
    }
    fn add_candidate(&mut self, candidate: Candidate) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let candidate = IceCandidate::parse(
                candidate.sdp_mid.as_deref().unwrap_or_default(),
                candidate.sdp_m_line_index.map(i32::from).unwrap_or(-1),
                &candidate.candidate,
            )
            .map_err(|_| engine_error("Invalid remote ICE candidate"))?;
            self.connection
                .add_ice_candidate(candidate)
                .await
                .map_err(|_| engine_error("Remote ICE candidate failed"))
        })
    }
    fn send(&mut self, data: &str) -> Result<(), NativeError> {
        if self.closed || data.len() > MAX_DATA_BYTES {
            return Err(engine_error(
                "Data channel is closed or data exceeds limits",
            ));
        }
        let slot = self.channel.lock().expect("data channel lock");
        let channel = slot
            .as_ref()
            .ok_or_else(|| engine_error("Data channel is not ready"))?;
        if channel.state() != DataChannelState::Open {
            return Err(engine_error("Data channel is not open"));
        }
        if channel.buffered_amount().saturating_add(data.len() as u64) > MAX_BUFFERED_BYTES {
            return Err(NativeError::new(
                "backpressure",
                "Native send buffer is full",
            ));
        }
        channel
            .send(data.as_bytes(), false)
            .map_err(|_| engine_error("Native engine did not accept the data"))
    }
    fn slots(&self) -> Vec<MediaSlot> {
        self.media.descriptors()
    }
    fn set_source(
        &mut self,
        kind: SourceKind,
        source: Option<&dyn SourcePort>,
    ) -> Result<(), NativeError> {
        if self.closed {
            return Err(engine_error("Native peer is closed"));
        }
        self.media.set_source(kind, source)
    }
    fn media_stats(&mut self) -> PortFuture<'_, Vec<MediaObservation>> {
        Box::pin(self.media.stats(&self.connection))
    }
    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.connection.on_data_channel(None);
        self.connection.on_ice_candidate(None);
        self.connection.on_connection_state_change(None);
        if let Some(channel) = self.channel.lock().expect("data channel lock").take() {
            channel.on_message(None);
            channel.on_state_change(None);
            channel.on_buffered_amount_change(None);
            channel.close();
        }
        self.media.close();
        self.connection.close();
    }
}
impl Drop for LibWebRtcPeer {
    fn drop(&mut self) {
        self.close();
    }
}
fn engine_error(message: &str) -> NativeError {
    NativeError::new("engine", message)
}
