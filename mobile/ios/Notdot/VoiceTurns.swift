import AVFoundation
import Foundation

/// The paired computer, as a voice turn needs it (faked in unit tests).
protocol VoiceTurnClient: Sendable {
    func ensureConnected() async throws
    func turn(requestId: String, pcm16: Data, listener: VoiceReplyListener) async throws -> VoiceTurnResult
    func cancel(requestId: String) async
}

/// Plays reply clips in order; `stopAll` drops anything queued or playing.
protocol ReplySink: AnyObject {
    func play(clip: UInt32, mime: String, audio: Data)
    func stopAll()
}

/// What the screen shows during a voice turn.
enum VoiceUi: Equatable {
    case listening, sending
    case heard(String), replying(String), reconnecting(UInt32)
    case finished(cancelled: Bool)
    case nothingHeard(String), failed(String)
}

struct PhoneLinkClient: VoiceTurnClient, @unchecked Sendable {
    let link: @Sendable () async -> PhoneLink?
    func ensureConnected() async throws {
        guard let link = await link(), await link.pairedRecord() != nil else {
            throw MobileError.Offline(detail: "Link this phone to your computer first.")
        }
        if await link.route() == .disconnected { _ = try await link.connect(relayOnly: false) }
    }
    func turn(requestId: String, pcm16: Data, listener: VoiceReplyListener) async throws -> VoiceTurnResult {
        guard let link = await link() else { throw MobileError.Offline(detail: "Link this phone to your computer first.") }
        return try await link.voiceTurn(requestId: requestId, pcm16: pcm16, listener: listener)
    }
    func cancel(requestId: String) async {
        _ = try? await link()?.cancelVoice(requestId: requestId)
    }
}

/// The plain-language detail every core error carries.
func plainDetail(_ error: Error) -> String {
    switch error as? MobileError {
    case .Invalid(let d), .Link(let d), .Unpaired(let d), .Wake(let d), .Cancelled(let d), .Denied(let d),
         .Expired(let d), .Offline(let d), .Unavailable(let d): return d
    default: return "Couldn't reach your computer."
    }
}

/// One voice turn at a time: after the wake phrase, capture the request
/// (endpointed in the Rust core), send it, and play the reply clip by clip.
/// A new wake phrase or Stop cancels the turn in progress. Request audio is
/// kept in memory only and dropped once sent. Thread-safe: audio arrives on
/// the capture thread, UI updates go to `ui`.
final class VoiceTurns: @unchecked Sendable {
    private let client: VoiceTurnClient
    private let sink: ReplySink
    private let ui: @Sendable (VoiceUi) -> Void
    private let lock = NSLock()
    private var capture: RequestCapture?
    private var requestId: String?

    init(client: VoiceTurnClient, sink: ReplySink, ui: @escaping @Sendable (VoiceUi) -> Void) {
        self.client = client
        self.sink = sink
        self.ui = ui
    }

    var listening: Bool { lock.withLock { capture != nil } }

    /// A request is being sent or its reply is still arriving.
    var busy: Bool { lock.withLock { requestId != nil } }

    /// The wake phrase was heard: start capturing the request.
    func wake() {
        lock.withLock {
            cancelCurrent()
            capture = RequestCapture()
        }
        ui(.listening)
    }

    /// Microphone audio after the wake phrase, at the hardware rate.
    func feed(samples: [Float], rate: UInt32) {
        guard let current = lock.withLock({ capture }) else { return }
        guard let state = try? current.pushSamples(samples: samples, rate: rate) else { return }
        lock.lock()
        guard capture === current else { lock.unlock(); return }
        switch state {
        case .done, .tooLong:
            capture = nil
            let pcm = current.takePcm16()
            lock.unlock()
            send(pcm)
        case .noSpeech:
            capture = nil
            lock.unlock()
            ui(.nothingHeard("I didn't hear a request."))
        default:
            lock.unlock()
        }
    }

    private func send(_ pcm16: Data) {
        let id = newRequestId()
        lock.withLock { requestId = id }
        ui(.sending)
        let listener = Listener(owner: self, id: id)
        Task.detached { [client, weak self] in
            var outcome: VoiceUi
            do {
                try await client.ensureConnected()
                let result = try await client.turn(requestId: id, pcm16: pcm16, listener: listener)
                outcome = .finished(cancelled: result.cancelled)
            } catch {
                outcome = .failed(plainDetail(error))
            }
            guard let self else { return }
            let current = self.lock.withLock { () -> Bool in
                guard self.requestId == id else { return false }
                self.requestId = nil
                return true
            }
            if current { self.ui(outcome) }
        }
    }

    fileprivate func deliver(_ id: String, _ action: () -> Void) {
        if lock.withLock({ requestId == id }) { action() }
    }

    /// Stop: drop the capture, stop playback, and cancel the turn there too.
    func stop() {
        lock.withLock { cancelCurrent() }
    }

    /// Call with the lock held.
    private func cancelCurrent() {
        capture = nil
        sink.stopAll()
        guard let id = requestId else { return }
        requestId = nil
        Task.detached { [client] in await client.cancel(requestId: id) }
    }

    private final class Listener: VoiceReplyListener, @unchecked Sendable {
        weak var owner: VoiceTurns?
        let id: String
        init(owner: VoiceTurns, id: String) { self.owner = owner; self.id = id }
        func heard(text: String) { owner?.deliver(id) { owner?.ui(.heard(text)) } }
        func replyText(text: String) { owner?.deliver(id) { owner?.ui(.replying(text)) } }
        func replyClip(clip: UInt32, mime: String, audio: Data) { owner?.deliver(id) { owner?.sink.play(clip: clip, mime: mime, audio: audio) } }
        func reconnecting(attempt: UInt32) { owner?.deliver(id) { owner?.ui(.reconnecting(attempt)) } }
    }
}

/// Plays reply clips one after another from memory (nothing is written to
/// storage). In silent test mode it never plays; it only counts clips.
final class ReplyPlayer: NSObject, ReplySink, AVAudioPlayerDelegate, @unchecked Sendable {
    private let silent: Bool
    private var queue: [Data] = []
    private var player: AVAudioPlayer?
    private(set) var clipsReceived = 0
    /// True when a reply starts sounding, false once it has finished or stopped.
    var onPlaying: ((Bool) -> Void)?
    private let state = NSLock()
    private var audible = false
    /// A reply is sounding (any thread).
    var playing: Bool { state.withLock { audible } }

    init(silent: Bool) { self.silent = silent }

    private func sounding(_ active: Bool) {
        let changed = state.withLock { () -> Bool in defer { audible = active }; return audible != active }
        if changed { onPlaying?(active) }
    }

    func play(clip: UInt32, mime: String, audio: Data) {
        DispatchQueue.main.async { [self] in
            clipsReceived += 1
            guard !silent else { return }
            queue.append(audio)
            if player == nil { next() }
        }
    }

    func stopAll() {
        DispatchQueue.main.async { [self] in
            queue.removeAll()
            player?.stop()
            player = nil
            sounding(false)
        }
    }

    private func next() {
        player = nil
        while !queue.isEmpty {
            let audio = queue.removeFirst()
            // A clip that can't play is skipped, not fatal to the reply.
            if let p = try? AVAudioPlayer(data: audio) {
                p.delegate = self
                player = p
                if p.play() {
                    sounding(true)
                    return
                }
            }
        }
        sounding(false)
    }

    func audioPlayerDidFinishPlaying(_ p: AVAudioPlayer, successfully: Bool) {
        DispatchQueue.main.async { [self] in if player === p { next() } }
    }
}
