//! Voice turns end to end over real Iroh QUIC endpoints on this machine,
//! with iroh's local test relay. The desktop runs the synthetic voice
//! pipeline (fake transcription, turn and speech): no microphone, no
//! speaker, no paid service. Synthetic profiles and audio only.
use iroh::{EndpointAddr, RelayMap, TransportAddr, test_utils::run_relay_server};
use notdot_link::{
    pairing::{DeviceProfile, OFFER_TTL_MS, ProfileRef},
    protocol::{OpError, PairFrame, RequestId, ServerFrame},
    service::system_clock,
    transport::{Desktop, Link, NetConfig, Phone, VoiceReplies, desktop_addr},
    voice::{AUDIO_CHUNK_BYTES, MAX_VOICE_SECONDS, SyntheticVoice, VoiceEvent},
};
use std::{collections::BTreeMap, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(60);

fn net(relays: &RelayMap) -> NetConfig {
    NetConfig {
        relays: Some(relays.clone()),
        direct: true,
        insecure_test_relay: true,
        relay_roots: Vec::new(),
    }
}

fn relay_only(addr: &EndpointAddr) -> EndpointAddr {
    EndpointAddr::from_parts(
        addr.id,
        addr.addrs
            .iter()
            .filter(|a| matches!(a, TransportAddr::Relay(_)))
            .cloned(),
    )
}

async fn paired(relays: &RelayMap, voice: SyntheticVoice) -> (Desktop, Phone, EndpointAddr) {
    let desktop = Desktop::bind(None, &net(relays), system_clock())
        .await
        .unwrap();
    desktop.set_profile_epoch("owner", 1);
    desktop.online().await;
    desktop.service().synthetic().unwrap().set_voice(voice);
    let phone = Phone::bind(None, &net(relays)).await.unwrap();
    let owner = ProfileRef {
        id: "owner".into(),
        epoch: 1,
    };
    let qr = desktop
        .create_offer(owner, DeviceProfile::Owner, OFFER_TTL_MS)
        .unwrap();
    let addr = desktop_addr(&qr).unwrap();
    let pairing = phone
        .pair(&qr, addr.clone(), "Synthetic phone")
        .await
        .unwrap();
    desktop.approve(&desktop.pending()[0]).unwrap();
    assert!(matches!(
        pairing.outcome().await.unwrap(),
        PairFrame::Approved { .. }
    ));
    (desktop, phone, addr)
}

/// One second of a synthetic tone as 16 kHz PCM16 LE (never played).
fn request(seconds: f32, amplitude: f32) -> Vec<u8> {
    let n = (16_000.0 * seconds) as usize;
    (0..n)
        .flat_map(|k| {
            let v = (k as f32 / 16_000.0 * 300.0 * std::f32::consts::TAU).sin() * amplitude;
            (v as i16).to_le_bytes()
        })
        .collect()
}

/// What a phone has received of one reply: events by sequence number.
#[derive(Default)]
struct Received {
    events: BTreeMap<u32, VoiceEvent>,
    duplicates: u32,
    last: Option<ServerFrame>,
}
impl Received {
    fn next(&self) -> u32 {
        self.events.len() as u32
    }
    /// Read frames until the final one, or until `stop_after` new events.
    async fn read(&mut self, replies: &mut VoiceReplies, stop_after: Option<usize>) {
        let mut new = 0;
        while let Some(frame) = replies.next().await.unwrap() {
            match frame {
                ServerFrame::Event { seq, event, .. } => {
                    assert!(seq <= self.next(), "events arrive in order");
                    if self.events.insert(seq, event).is_some() {
                        self.duplicates += 1;
                    }
                    new += 1;
                    if stop_after.is_some_and(|n| new >= n) {
                        return;
                    }
                }
                final_frame => {
                    self.last = Some(final_frame);
                    return;
                }
            }
        }
    }
    /// Reassembled audio clips by clip number, each only if complete.
    fn clips(&self) -> BTreeMap<u32, Vec<u8>> {
        let mut clips: BTreeMap<u32, (Vec<u8>, bool)> = BTreeMap::new();
        for event in self.events.values() {
            if let VoiceEvent::Audio {
                clip, data, last, ..
            } = event
            {
                let entry = clips.entry(*clip).or_default();
                assert!(!entry.1, "no audio after a clip's last piece");
                entry.0.extend_from_slice(data);
                entry.1 = *last;
            }
        }
        clips
            .into_iter()
            .filter(|(_, (_, done))| *done)
            .map(|(k, (v, _))| (k, v))
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_turn_streams_its_reply_resumes_after_a_drop_and_never_runs_twice() {
    tokio::time::timeout(TIMEOUT, async {
        let (_relay_map, relay_url, _guard) = run_relay_server().await.unwrap();
        let relays = RelayMap::from(relay_url);
        let voice = SyntheticVoice {
            sentences: 3,
            step: Duration::from_millis(25),
            clip_samples: 30_000,
        };
        let (desktop, phone, addr) = paired(&relays, voice.clone()).await;
        // Over the relay, as from outside the home network.
        let link = phone.connect(relay_only(&addr)).await.unwrap();
        let pcm = request(1.2, 8000.0);
        let id = RequestId::random();
        let mut got = Received::default();
        let mut replies = link.voice_turn(id, &pcm).await.unwrap();
        got.read(&mut replies, Some(3)).await;
        assert!(matches!(&got.events[&0], VoiceEvent::Heard { text } if text == "synthetic request of 1200 ms"));
        // The network drops mid-reply: the turn keeps running.
        link.close();
        drop(replies);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let link = phone.connect(relay_only(&addr)).await.unwrap();
        let mut replies = link.voice_resume(id, got.next()).await.unwrap();
        got.read(&mut replies, None).await;
        assert_eq!(got.duplicates, 0);
        let Some(ServerFrame::Done { result, .. }) = &got.last else {
            panic!("expected Done, got {:?}", got.last)
        };
        assert_eq!(result["reply"], "Synthetic sentence 1. Synthetic sentence 2. Synthetic sentence 3.");
        let clips = got.clips();
        assert_eq!(clips.len(), 3);
        for (i, clip) in &clips {
            assert_eq!(clip, &voice.clip(*i), "clip {i} arrives intact and in order");
            assert!(clip.len() > AUDIO_CHUNK_BYTES, "clips span several chunks");
        }
        let texts: Vec<_> = got
            .events
            .values()
            .filter_map(|e| match e {
                VoiceEvent::Reply { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Synthetic sentence 1.", "Synthetic sentence 2.", "Synthetic sentence 3."]);
        // Sending the same request again (unsure whether it arrived) replays
        // the same reply from its start; the turn is not run twice.
        let mut again = Received::default();
        let mut replies = link.voice_turn(id, &pcm).await.unwrap();
        again.read(&mut replies, None).await;
        assert_eq!(again.events, got.events);
        assert_eq!(again.last, got.last);
        assert_eq!(desktop.service().synthetic().unwrap().voice_turns(), 1);
        // The same id with different audio is refused, not run.
        let mut replies = link.voice_turn(id, &request(1.0, 8000.0)).await.unwrap();
        let mut other = Received::default();
        other.read(&mut replies, None).await;
        assert!(matches!(other.last, Some(ServerFrame::Failed { error: OpError::BadRequest { .. }, .. })));
        assert_eq!(desktop.service().synthetic().unwrap().voice_turns(), 1);
        link.close();
        phone.close().await;
        desktop.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_a_voice_turn_mid_reply_and_resuming_reports_it() {
    tokio::time::timeout(TIMEOUT, async {
        let (_relay_map, relay_url, _guard) = run_relay_server().await.unwrap();
        let relays = RelayMap::from(relay_url);
        let voice = SyntheticVoice {
            sentences: 5,
            step: Duration::from_millis(60),
            clip_samples: 8_000,
        };
        let (desktop, phone, addr) = paired(&relays, voice).await;
        let link = phone.connect(addr.clone()).await.unwrap();
        let id = RequestId::random();
        let mut got = Received::default();
        let mut replies = link.voice_turn(id, &request(1.0, 8000.0)).await.unwrap();
        got.read(&mut replies, Some(2)).await;
        assert!(link.cancel(id).await.unwrap(), "the running turn was found");
        got.read(&mut replies, None).await;
        assert!(
            matches!(got.last, Some(ServerFrame::Cancelled { .. })),
            "{:?}",
            got.last
        );
        assert!(got.events.len() < 11, "stopped before the reply finished");
        assert!(!link.cancel(id).await.unwrap(), "nothing left to cancel");
        let mut late = Received::default();
        let mut replies = link.voice_resume(id, got.next()).await.unwrap();
        late.read(&mut replies, None).await;
        assert!(matches!(late.last, Some(ServerFrame::Cancelled { .. })));
        // A new turn can start once the cancelled one has ended.
        let mut next = Received::default();
        let mut replies = link
            .voice_turn(RequestId::random(), &request(0.5, 8000.0))
            .await
            .unwrap();
        next.read(&mut replies, None).await;
        assert!(matches!(next.last, Some(ServerFrame::Done { .. })));
        link.close();
        phone.close().await;
        desktop.close().await;
    })
    .await
    .unwrap();
}

async fn final_of(link: &Link, id: RequestId, pcm: &[u8]) -> ServerFrame {
    let mut got = Received::default();
    let mut replies = link.voice_turn(id, pcm).await.unwrap();
    got.read(&mut replies, None).await;
    got.last.expect("a final frame")
}

#[tokio::test(flavor = "multi_thread")]
async fn voice_turns_refuse_silence_overlong_audio_overlap_and_unknown_resumes() {
    tokio::time::timeout(TIMEOUT, async {
        let (_relay_map, relay_url, _guard) = run_relay_server().await.unwrap();
        let relays = RelayMap::from(relay_url);
        let voice = SyntheticVoice {
            sentences: 4,
            step: Duration::from_millis(80),
            clip_samples: 4_000,
        };
        let (desktop, phone, addr) = paired(&relays, voice).await;
        let link = phone.connect(addr.clone()).await.unwrap();
        // Silence: the fake recogniser hears nothing, nothing runs.
        assert!(matches!(
            final_of(&link, RequestId::random(), &request(1.0, 0.0)).await,
            ServerFrame::Failed { error: OpError::BadRequest { reason, .. }, .. } if reason == "no speech heard"
        ));
        // Longer than the limit: refused before any audio is read.
        let long = request(MAX_VOICE_SECONDS as f32 + 1.0, 8000.0);
        assert!(matches!(
            final_of(&link, RequestId::random(), &long).await,
            ServerFrame::Failed { error: OpError::BadRequest { .. }, .. }
        ));
        // One running turn per phone.
        let first = RequestId::random();
        let mut replies = link.voice_turn(first, &request(1.0, 8000.0)).await.unwrap();
        let mut got = Received::default();
        got.read(&mut replies, Some(1)).await;
        assert!(matches!(
            final_of(&link, RequestId::random(), &request(1.0, 8000.0)).await,
            ServerFrame::Failed { error: OpError::Busy, .. }
        ));
        got.read(&mut replies, None).await;
        assert!(matches!(got.last, Some(ServerFrame::Done { .. })));
        // Resuming a turn the computer never saw asks for it again.
        let mut replies = link.voice_resume(RequestId::random(), 0).await.unwrap();
        let mut unknown = Received::default();
        unknown.read(&mut replies, None).await;
        assert!(matches!(unknown.last, Some(ServerFrame::Failed { error: OpError::UnknownRequest, .. })));
        // Revoking the phone ends the link; a removed phone can't send.
        desktop.revoke(phone.id()).unwrap();
        assert!(link.voice_turn(RequestId::random(), &request(0.5, 8000.0)).await.is_err()
            || link.closed().await.is_some());
        phone.close().await;
        desktop.close().await;
    })
    .await
    .unwrap();
}
