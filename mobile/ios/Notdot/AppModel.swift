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
        #if NOTDOT_WAKE
        if !silent, let models = Bundle.main.resourceURL?.appendingPathComponent("wake").path {
            wake = try? WakeEngine(models: models, name: "Ava", nameOnly: false)
        }
        var meter = (sum: Float(0), count: 0, peak: Float(0))
        capture.onSamples = { [weak self] samples, rate in
            guard let self else { return }
            // Level only (never audio) for diagnostics, once a second.
            let rms = (samples.reduce(0) { $0 + $1 * $1 } / Float(max(samples.count, 1))).squareRoot()
            meter = (meter.sum + rms, meter.count + 1, max(meter.peak, rms))
            if meter.count * samples.count >= Int(rate) {
                // Backup decoder state and counters (no audio or text).
                let backup = self.wake.map { w in let s = w.backupStats(); return "\(w.backupState()) decodes=\(s.decodes) wakes=\(s.wakes) failures=\(s.failures) decode_ms_total=\(s.decodeMsTotal) decode_ms_max=\(s.decodeMsMax)" } ?? "none"
                AppModel.log.info("capture rate=\(rate) mean_rms=\(meter.sum / Float(meter.count)) peak_rms=\(meter.peak) backup=\(backup, privacy: .public)")
                meter = (0, 0, 0)
            }
            guard let wake = self.wake, wake.accept(samples: samples, rate: rate) else { return }
            AppModel.log.info("wake detected")
            Task { @MainActor in self.wakeHeard() }
        }
        #endif
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
        case .background: handle(.appHidden)
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

    /// A wake word: open listening. No cue is played in this prototype.
    func wakeHeard() {
        handle(.activity)
        orb.mode = .listening
    }

    func handle(_ event: SessionEvent) {
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
        case .foreground: "Ready"
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
