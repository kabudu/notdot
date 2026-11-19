import ActivityKit
import OSLog
import Foundation
import SwiftUI

/// App state. The session rules live in the Rust core (VoiceSession); this
/// type feeds it OS events and applies the effects it returns.
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()
    nonisolated static let log = Logger(subsystem: "dev.notdot.mobile.prototype", category: "voice")

    /// `-NotdotSilentTest YES` (or NOTDOT_SILENT=1): never request or open the
    /// microphone, never activate the audio session. For unattended runs.
    let silent: Bool
    @Published private(set) var effects: Effects
    @Published var orb: OrbState
    @Published private(set) var status = "Ready"
    /// Tap to talk: the default way to speak to the computer.
    @Published private(set) var talk: TalkState = .idle
    let link = LinkModel()

    private let session: VoiceSession
    private let capture: AudioCapture
    private var activity: Activity<NotdotSessionAttributes>?
    private var tick: Timer?
    private let clock = ContinuousClock()
    private let started = ContinuousClock.now
    #if NOTDOT_WAKE
    private nonisolated(unsafe) var wake: WakeEngine?
    #endif
    /// Voice turns to the paired computer (capture, send, play the reply).
    private nonisolated(unsafe) var voiceTurns: VoiceTurns?
    private nonisolated(unsafe) var tapToTalk: TapToTalk?
    /// Wake is ignored while a reply plays (and briefly after).
    private nonisolated let playback = PlaybackGate()

    init() {
        let arguments = ProcessInfo.processInfo
        silent = UserDefaults.standard.bool(forKey: "NotdotSilentTest") || arguments.environment["NOTDOT_SILENT"] == "1"
        var policy = defaultSessionPolicy(platform: .ios)
        policy.microphonePermitted = AudioCapture.permitted && !silent
        session = VoiceSession(policy: policy)
        capture = AudioCapture(silent: silent)
        effects = Effects(state: .off, microphone: false, surface: false, endSurface: false, remainingMs: nil)
        orb = OrbState(mode: .idle, task: .none, hue: orbDefaultHue(), level: nil, reducedMotion: UIAccessibility.isReduceMotionEnabled,
                       quota: .normal, taskAvailable: false, microphoneMuted: false, wakeReady: false, offline: true)
        #if DEBUG
        // Debug-only unattended test hook: start a session at launch
        // (microphone permission must already be granted, e.g. simctl).
        if !silent, UserDefaults.standard.bool(forKey: "NotdotAutoStart") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
                self?.handle(.appVisible)
                self?.start(source: .app)
            }
        }
        #endif
        let player = ReplyPlayer(silent: silent)
        let turns = VoiceTurns(
            client: PhoneLinkClient(link: { await MainActor.run { AppModel.shared.link.current } }),
            sink: player,
            ui: { ui in Task { @MainActor in AppModel.shared.voice(ui) } })
        voiceTurns = turns
        let capture = self.capture
        tapToTalk = TapToTalk(
            turns: turns,
            startMicrophone: { try MainActor.assumeIsolated { try capture.start() } },
            stopMicrophone: { Task { @MainActor in if !AppModel.shared.effects.microphone { capture.stop() } } },
            playing: { player.playing },
            onState: { state in Task { @MainActor in AppModel.shared.talk = state } })
        player.onPlaying = { [playback] active in
            playback.playback(active)
            Task { @MainActor in AppModel.shared.tapToTalk?.changed() }
        }
        #if NOTDOT_WAKE
        if !silent, let models = Bundle.main.resourceURL?.appendingPathComponent("wake").path {
            wake = try? WakeEngine(models: models, name: "Ava", nameOnly: false)
        }
        var meter = (sum: Float(0), count: 0, peak: Float(0))
        #endif
        capture.onSamples = { [weak self] samples, rate in
            guard let self else { return }
            // A tap's request: its own capture, no wake word.
            if let tap = self.tapToTalk, tap.isCapturing {
                tap.feed(samples: samples, rate: rate)
                return
            }
            #if NOTDOT_WAKE
            // Level only (never audio) for diagnostics, once a second.
            let rms = (samples.reduce(0) { $0 + $1 * $1 } / Float(max(samples.count, 1))).squareRoot()
            meter = (meter.sum + rms, meter.count + 1, max(meter.peak, rms))
            if meter.count * samples.count >= Int(rate) {
                // Backup decoder state and counters (no audio or text).
                let backup = self.wake.map { w in let s = w.backupStats(); return "\(w.backupState()) decodes=\(s.decodes) wakes=\(s.wakes) failures=\(s.failures) decode_ms_total=\(s.decodeMsTotal) decode_ms_max=\(s.decodeMsMax)" } ?? "none"
                AppModel.log.info("capture rate=\(rate) mean_rms=\(meter.sum / Float(meter.count)) peak_rms=\(meter.peak) backup=\(backup, privacy: .public)")
                meter = (0, 0, 0)
            }
            // After the wake phrase, the same audio is the request.
            if let turns = self.voiceTurns, turns.listening { turns.feed(samples: samples, rate: rate) }
            // The phone's own reply (and its echo) never wakes it; the
            // detector restarts afterwards rather than joining audio across it.
            if self.playback.suppressed { return }
            if self.playback.resumed() { self.wake?.reset() }
            guard let wake = self.wake, wake.accept(samples: samples, rate: rate) else { return }
            AppModel.log.info("wake detected")
            self.voiceTurns?.wake()
            Task { @MainActor in self.wakeHeard() }
            #endif
        }
        capture.onInterruption = { [weak self] began, resume in
            Task { @MainActor in
                self?.handle(began ? .interruptionBegan : .interruptionEnded(shouldResume: resume))
            }
        }
        NotificationCenter.default.addObserver(forName: .notdotStopHandsFree, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.handle(.stop) }
        }
    }

    private var nowMs: UInt64 { UInt64((clock.now - started).components.seconds * 1000) }

    func scenePhase(_ phase: ScenePhase) {
        switch phase {
        case .active: handle(.appVisible)
        case .background:
            // Tap to talk only listens while the app is in use; a reply keeps playing.
            tapToTalk?.hidden()
            handle(.appHidden)
        default: break
        }
    }

    /// User-initiated start. Asks for the microphone here, never unattended.
    func start(source: StartSource) {
        guard !silent else {
            status = "Silent test mode: microphone stays off"
            return
        }
        Task {
            if !AudioCapture.permitted, await AudioCapture.requestPermission() == false {
                status = "Microphone access is off. Turn it on in Settings to talk hands-free."
                return
            }
            handle(.start(source: source))
        }
    }

    /// Tap to talk (user action). While hands-free is running, a tap starts
    /// a request there instead (one microphone owner).
    func tapTalk() {
        guard !silent else {
            status = "Silent test mode: microphone stays off"
            return
        }
        Task {
            if !AudioCapture.permitted, await AudioCapture.requestPermission() == false {
                status = "Microphone access is off. Turn it on in Settings to talk."
                return
            }
            if effects.microphone {
                voiceTurns?.wake()
                wakeHeard()
                return
            }
            guard let tap = tapToTalk else { return }
            if tap.state == .idle, await link.current.pairedRecord() == nil {
                status = "Link this phone to your computer to talk."
                return
            }
            if !tap.tap() { status = "The microphone could not start." }
        }
    }

    /// What the status line says between turns.
    private var idleText: String {
        effects.microphone ? "Listening hands-free · Say Hey Ava" : "Tap to talk"
    }

    /// A wake word: open listening. No cue is played in this prototype.
    func wakeHeard() {
        handle(.activity)
        orb.mode = .listening
        status = "Listening…"
    }

    /// Show a voice turn's progress (text only; audio is never logged).
    func voice(_ ui: VoiceUi) {
        handle(.activity)
        let mode: OrbMode
        switch ui {
        case .listening: (mode, status) = (.listening, "Listening…")
        case .sending: (mode, status) = (.thinking, "Sending to your computer…")
        case .heard(let text): (mode, status) = (.thinking, "“\(text)”")
        case .replying(let text): (mode, status) = (.speaking, text)
        case .reconnecting: (mode, status) = (.thinking, "Reconnecting to your computer…")
        case .finished(let cancelled): (mode, status) = (.idle, cancelled ? "Stopped." : idleText)
        case .nothingHeard(let reason), .failed(let reason): (mode, status) = (.idle, reason)
        }
        // Tap to talk runs without a hands-free session, so the orb follows every turn.
        orb.mode = mode
        tapToTalk?.changed()
    }

    func handle(_ event: SessionEvent) {
        if case .stop = event { voiceTurns?.stop() }
        _ = session.handle(event: AudioCapture.permitted && !silent ? .permissionGranted : .permissionLost, nowMs: nowMs)
        let next = session.handle(event: event, nowMs: nowMs)
        apply(next)
    }

    private func apply(_ next: Effects) {
        effects = next
        orb.wakeReady = next.microphone
        status = Self.describe(next.state)
        if next.microphone {
            do {
                try capture.start()
            } catch {
                status = "The microphone could not start."
            }
        } else {
            capture.stop()
        }
        if next.surface { showActivity(next) }
        if next.endSurface { endActivity() }
        tick?.invalidate()
        if next.surface {
            tick = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.handle(.tick) }
            }
        }
    }

    static func describe(_ state: SessionState) -> String {
        switch state {
        case .off: "Not listening"
        case .foreground: "Tap to talk"
        case .handsFree: "Listening hands-free"
        case .paused(let reason):
            switch reason {
            case .interrupted: "Paused by a call"
            case .silenced: "Paused: another app is using the microphone"
            case .userPaused: "Paused"
            }
        case .ended(let reason):
            switch reason {
            case .idleTimeout: "Stopped after a quiet spell"
            case .surfaceEnded: "Stopped: the Live Activity ended"
            default: "Stopped"
            }
        }
    }

    private func showActivity(_ effects: Effects) {
        let endsAt = Date().addingTimeInterval(Double(effects.remainingMs ?? 0) / 1000)
        let content = ActivityContent(state: NotdotSessionAttributes.ContentState(status: status, endsAt: endsAt), staleDate: endsAt)
        if let activity {
            Task { await activity.update(content) }
        } else if ActivityAuthorizationInfo().areActivitiesEnabled {
            activity = try? Activity.request(attributes: NotdotSessionAttributes(), content: content)
        } else {
            // A session must be visible; without Live Activities it ends.
            handle(.surfaceEnded)
        }
    }

    private func endActivity() {
        guard let activity else { return }
        self.activity = nil
        Task { await activity.end(nil, dismissalPolicy: .immediate) }
    }
}
