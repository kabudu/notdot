//! A paired phone's voice turn on this computer: transcribe the request,
//! run it as a turn for the phone's own profile, and stream the reply back
//! sentence by sentence, each sentence spoken with the configured voice.
//!
//! The orchestration ([`run`]) is independent of the providers so it is
//! tested with fakes; [`AppVoice`] connects it to the desktop's real
//! transcription, Codex turn and speech. Phone audio is never written to
//! disk; the request is held in memory for transcription only.
use notdot_link::{
    protocol::OpError,
    voice::{VoiceAudio, VoiceEvent, VoiceSink, audio_events},
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[path = "../../../crates/notdot-orb/src/speech_queue.rs"]
#[allow(dead_code)]
mod speech_queue;

/// Longest wait for a turn's reply to finish.
pub const TURN_LIMIT: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(50);

/// What a phone turn needs from the computer.
pub trait DesktopVoice: Send + Sync {
    /// Transcribe a 16 kHz mono WAV request.
    fn transcribe(&self, wav: Vec<u8>) -> impl Future<Output = Result<String, String>> + Send;
    /// Start a turn for the phone's profile; returns the turn id.
    fn submit(&self, text: String) -> impl Future<Output = Result<String, String>> + Send;
    /// The turn's spoken reply so far: (text, done, failed), or `None` if
    /// this computer has moved on to another turn.
    fn progress(&self, turn: &str) -> impl Future<Output = Option<(String, bool, bool)>> + Send;
    /// Speak one sentence: (mime type, encoded audio).
    fn synthesize(
        &self,
        text: String,
    ) -> impl Future<Output = Result<(String, Vec<u8>), String>> + Send;
    /// Stop the turn this phone started.
    fn stop(&self) -> impl Future<Output = ()> + Send;
}

fn unavailable(reason: impl ToString) -> OpError {
    OpError::NotAvailable {
        reason: reason.to_string(),
    }
}

/// 16 kHz mono PCM16 as a WAV file, for the transcription provider.
pub fn wav(audio: &VoiceAudio) -> Vec<u8> {
    let data = (audio.samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&audio.rate.to_le_bytes());
    out.extend_from_slice(&(audio.rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in &audio.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Resolves once `cancelled` is set (or its sender is gone); holds no
/// borrow across awaits, so the turn's future stays `Send`.
async fn until_cancelled(cancelled: &mut watch::Receiver<bool>) {
    let _ = cancelled.wait_for(|c| *c).await.map(|_| ());
}

/// Run one phone turn. `Ok(None)` if cancelled (the turn is stopped here).
pub async fn run(
    voice: &impl DesktopVoice,
    audio: VoiceAudio,
    sink: VoiceSink,
    mut cancelled: watch::Receiver<bool>,
) -> Result<Option<Value>, OpError> {
    let heard = tokio::select! {
        heard = voice.transcribe(wav(&audio)) => heard.map_err(unavailable)?,
        _ = until_cancelled(&mut cancelled) => return Ok(None),
    };
    let heard = heard.trim().to_owned();
    if heard.is_empty() {
        return Err(OpError::BadRequest {
            reason: "no speech heard".into(),
        });
    }
    sink.send(VoiceEvent::Heard {
        text: heard.clone(),
    })?;
    if *cancelled.borrow() {
        return Ok(None);
    }
    let turn = voice.submit(heard.clone()).await.map_err(unavailable)?;
    let mut queue = speech_queue::SpeechQueue::default();
    let mut reply = String::new();
    let mut clip = 0u32;
    let started = Instant::now();
    loop {
        if *cancelled.borrow() {
            voice.stop().await;
            return Ok(None);
        }
        let Some((text, done, failed)) = voice.progress(&turn).await else {
            return Err(OpError::Interrupted);
        };
        if failed {
            return Err(unavailable("the reply was interrupted before it finished"));
        }
        queue.update(&text, done).map_err(unavailable)?;
        while let Some(sentence) = queue.next() {
            if sentence.is_empty() {
                continue;
            }
            sink.send(VoiceEvent::Reply {
                text: sentence.clone(),
            })?;
            if !reply.is_empty() {
                reply.push(' ');
            }
            reply.push_str(&sentence);
            let spoken = tokio::select! {
                spoken = voice.synthesize(sentence) => spoken.map_err(unavailable)?,
                _ = until_cancelled(&mut cancelled) => {
                    voice.stop().await;
                    return Ok(None);
                }
            };
            for event in audio_events(clip, &spoken.0, &spoken.1) {
                sink.send(event)?;
            }
            clip += 1;
        }
        if queue.finished() {
            return Ok(Some(json!({"heard": heard, "reply": reply, "turn": turn})));
        }
        if started.elapsed() > TURN_LIMIT {
            voice.stop().await;
            return Err(unavailable("the reply took too long"));
        }
        tokio::select! {
            _ = tokio::time::sleep(POLL) => {}
            _ = until_cancelled(&mut cancelled) => {}
        }
    }
}

/// The desktop's real pipeline for a phone in `profile`.
pub struct AppVoice {
    pub state: std::sync::Arc<crate::AppState>,
    pub profile: notdot_core::profiles::ProfileId,
    pub request_id: String,
}
impl DesktopVoice for AppVoice {
    async fn transcribe(&self, wav: Vec<u8>) -> Result<String, String> {
        crate::transcribe_audio(&self.state, wav, "audio/wav".into()).await
    }
    async fn submit(&self, text: String) -> Result<String, String> {
        let identity = self
            .state
            .speakers
            .device_lock(&self.profile, &self.request_id)?;
        if self.state.context().id != self.profile {
            return Err("notdot on your computer is with someone else right now; try again when it's back with you".into());
        }
        crate::submit_turn(
            &self.state,
            identity,
            text,
            Some(self.request_id.clone()),
            None,
        )
        .await
    }
    async fn progress(&self, turn: &str) -> Option<(String, bool, bool)> {
        let s = self.state.snapshot.lock().await;
        (s.speech.turn == turn).then(|| (s.speech.text.clone(), s.speech.done, s.speech.failed))
    }
    async fn synthesize(&self, text: String) -> Result<(String, Vec<u8>), String> {
        crate::synthesize(&self.state, &text)
            .await
            .map(|clip| (clip.mime, clip.bytes))
    }
    async fn stop(&self) {
        let _ = crate::stop_turn(&self.state, Some(self.request_id.clone())).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notdot_link::protocol::{RequestId, ServerFrame};
    use std::sync::Mutex;

    /// Fake providers: a transcript, a reply that streams in deltas, and
    /// "speech" that is the sentence's bytes. No network, no audio device.
    struct Fake {
        transcript: String,
        deltas: Vec<&'static str>,
        shown: Mutex<usize>,
        fail_tts: bool,
        stopped: Mutex<bool>,
        submitted: Mutex<Vec<String>>,
    }
    impl Fake {
        fn new(transcript: &str, deltas: Vec<&'static str>) -> Self {
            Self {
                transcript: transcript.into(),
                deltas,
                shown: Mutex::new(0),
                fail_tts: false,
                stopped: Mutex::new(false),
                submitted: Mutex::new(Vec::new()),
            }
        }
    }
    impl DesktopVoice for Fake {
        async fn transcribe(&self, wav: Vec<u8>) -> Result<String, String> {
            assert_eq!(&wav[..4], b"RIFF");
            Ok(self.transcript.clone())
        }
        async fn submit(&self, text: String) -> Result<String, String> {
            self.submitted.lock().unwrap().push(text);
            Ok("turn-1".into())
        }
        async fn progress(&self, turn: &str) -> Option<(String, bool, bool)> {
            assert_eq!(turn, "turn-1");
            let mut shown = self.shown.lock().unwrap();
            *shown = (*shown + 1).min(self.deltas.len());
            let text: String = self.deltas[..*shown].concat();
            Some((text, *shown == self.deltas.len(), false))
        }
        async fn synthesize(&self, text: String) -> Result<(String, Vec<u8>), String> {
            if self.fail_tts {
                return Err("voice service unavailable".into());
            }
            Ok(("audio/mpeg".into(), text.into_bytes()))
        }
        async fn stop(&self) {
            *self.stopped.lock().unwrap() = true;
        }
    }

    fn audio() -> VoiceAudio {
        VoiceAudio {
            rate: 16_000,
            samples: vec![1000; 16_000],
        }
    }

    async fn collect(mut stream: notdot_link::voice::VoiceStream) -> Vec<VoiceEvent> {
        let mut events = Vec::new();
        while let Some(frame) = stream.next().await {
            if let ServerFrame::Event { event, .. } = frame {
                events.push(event);
            }
        }
        events
    }

    #[tokio::test]
    async fn a_streamed_reply_is_sent_sentence_by_sentence_with_its_speech() {
        let fake = Fake::new(
            " what's still running? ",
            vec![
                "Two tasks",
                " are running. The build",
                " is nearly done.",
                " Dr. Lee's review waits.",
            ],
        );
        let (sink, stream) = VoiceSink::detached();
        let (_cancel, cancelled) = watch::channel(false);
        let result = run(&fake, audio(), sink.clone(), cancelled)
            .await
            .unwrap()
            .unwrap();
        sink.close(ServerFrame::Done {
            id: RequestId(0),
            result: result.clone(),
        });
        assert_eq!(
            fake.submitted.lock().unwrap().as_slice(),
            ["what's still running?"]
        );
        assert_eq!(
            result["reply"],
            "Two tasks are running. The build is nearly done. Dr. Lee's review waits."
        );
        let events = collect(stream).await;
        let mut expected = vec![VoiceEvent::Heard {
            text: "what's still running?".into(),
        }];
        for (i, s) in [
            "Two tasks are running.",
            "The build is nearly done.",
            "Dr. Lee's review waits.",
        ]
        .into_iter()
        .enumerate()
        {
            expected.push(VoiceEvent::Reply { text: s.into() });
            expected.extend(audio_events(i as u32, "audio/mpeg", s.as_bytes()));
        }
        assert_eq!(events, expected);
    }

    #[tokio::test]
    async fn cancelling_stops_the_turn_on_this_computer() {
        let fake = Fake::new(
            "hello",
            vec!["One.", " Two.", " Three.", " Four.", " Five.", " Six."],
        );
        let (sink, _stream) = VoiceSink::detached();
        let (cancel, cancelled) = watch::channel(false);
        cancel.send(true).unwrap();
        // Cancelled before transcription finishes: nothing is submitted.
        assert_eq!(run(&fake, audio(), sink, cancelled).await.unwrap(), None);
        assert!(fake.submitted.lock().unwrap().is_empty());

        let (sink, _stream) = VoiceSink::detached();
        let (cancel, cancelled) = watch::channel(false);
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            cancel.send(true).unwrap();
        });
        assert_eq!(run(&fake, audio(), sink, cancelled).await.unwrap(), None);
        canceller.await.unwrap();
        assert!(
            *fake.stopped.lock().unwrap(),
            "the desktop turn was stopped"
        );
    }

    #[tokio::test]
    async fn silence_and_provider_failures_are_reported_plainly() {
        let (sink, _s) = VoiceSink::detached();
        let (_c, cancelled) = watch::channel(false);
        let quiet = Fake::new("   ", vec!["x."]);
        assert!(matches!(
            run(&quiet, audio(), sink, cancelled.clone()).await,
            Err(OpError::BadRequest { .. })
        ));
        assert!(quiet.submitted.lock().unwrap().is_empty());
        let (sink, _s) = VoiceSink::detached();
        let mut broken = Fake::new("hi", vec!["Hello."]);
        broken.fail_tts = true;
        assert!(matches!(
            run(&broken, audio(), sink, cancelled).await,
            Err(OpError::NotAvailable { reason }) if reason == "voice service unavailable"
        ));
    }

    #[test]
    fn requests_become_valid_wav() {
        let w = wav(&VoiceAudio {
            rate: 16_000,
            samples: vec![-2, 3],
        });
        assert_eq!(w.len(), 48);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
        assert_eq!(&w[44..], &[0xfe, 0xff, 3, 0]);
    }
}
