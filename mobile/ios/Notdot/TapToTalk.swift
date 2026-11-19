import Foundation

enum TalkState: Equatable { case idle, listening, replying }

/// Tap to talk, the default way to speak to the paired computer: no wake
/// word. A tap opens the microphone (only while the app is in use), captures
/// one request until the core's endpointer says it ended, sends it and plays
/// the reply. A tap while listening or replying stops everything, here and
/// on the computer. The microphone closes as soon as the request ends.
/// Thread-safe: audio arrives on the capture thread.
final class TapToTalk: @unchecked Sendable {
    private let turns: VoiceTurns
    private let startMicrophone: () throws -> Void
    private let stopMicrophone: () -> Void
    private let playing: () -> Bool
    private let onState: (TalkState) -> Void
    private let lock = NSLock()
    private var capturing = false

    init(turns: VoiceTurns, startMicrophone: @escaping () throws -> Void, stopMicrophone: @escaping () -> Void,
         playing: @escaping () -> Bool, onState: @escaping (TalkState) -> Void) {
        self.turns = turns
        self.startMicrophone = startMicrophone
        self.stopMicrophone = stopMicrophone
        self.playing = playing
        self.onState = onState
    }

    var isCapturing: Bool { lock.withLock { capturing } }

    var state: TalkState {
        if isCapturing { return .listening }
        return turns.busy || playing() ? .replying : .idle
    }

    /// The talk button. False only if the microphone could not start.
    @discardableResult
    func tap() -> Bool {
        if state != .idle {
            let wasCapturing = lock.withLock { () -> Bool in defer { capturing = false }; return capturing }
            if wasCapturing { stopMicrophone() }
            turns.stop()
        } else {
            do { try startMicrophone() } catch { return false }
            lock.withLock { capturing = true }
            turns.wake()
        }
        changed()
        return true
    }

    /// Microphone audio (any rate) while a tap is capturing.
    func feed(samples: [Float], rate: UInt32) {
        guard isCapturing else { return }
        turns.feed(samples: samples, rate: rate)
        guard !turns.listening else { return }
        let mine = lock.withLock { () -> Bool in defer { capturing = false }; return capturing }
        if mine { stopMicrophone() }
        changed()
    }

    /// The app was hidden: an unfinished request is dropped (a reply keeps playing).
    func hidden() {
        let wasCapturing = lock.withLock { () -> Bool in defer { capturing = false }; return capturing }
        if wasCapturing {
            stopMicrophone()
            turns.stop()
        }
        changed()
    }

    /// Re-publish the state (turn and playback events call this).
    func changed() { onState(state) }
}

/// Wake is ignored while the phone plays a reply and for `hold` after it
/// (room echo), so the assistant cannot wake itself by saying its own name.
/// `resumed()` is true once after suppression ends, to restart a detector.
final class PlaybackGate: @unchecked Sendable {
    private let lock = NSLock()
    private let now: () -> TimeInterval
    private let hold: TimeInterval
    private var playing = false
    private var quietUntil: TimeInterval = -.infinity
    private var wasSuppressed = false

    init(hold: TimeInterval = 0.4, now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
        self.hold = hold
        self.now = now
    }

    func playback(_ active: Bool) {
        lock.withLock {
            if playing && !active { quietUntil = now() + hold }
            playing = active
        }
    }

    var suppressed: Bool {
        lock.withLock {
            let s = playing || now() < quietUntil
            if s { wasSuppressed = true }
            return s
        }
    }

    func resumed() -> Bool {
        if suppressed { return false }
        return lock.withLock { defer { wasSuppressed = false }; return wasSuppressed }
    }
}
