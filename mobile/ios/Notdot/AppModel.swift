import ActivityKit
import Foundation
import SwiftUI

/// App state. The session rules live in the Rust core (VoiceSession); this
/// type feeds it OS events and applies the effects it returns.
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

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
        #if NOTDOT_WAKE
        if !silent, let models = Bundle.main.resourceURL?.appendingPathComponent("wake").path {
            wake = try? WakeEngine(models: models, name: "Ava", nameOnly: false)
        }
        capture.onSamples = { [weak self] samples, rate in
            guard let self, let wake = self.wake, wake.accept(samples: samples, rate: rate) else { return }
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
