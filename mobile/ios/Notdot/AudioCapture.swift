import AVFoundation

/// Microphone capture for wake detection. It never plays audio. In silent
/// test mode it never touches the audio session or the microphone at all.
final class AudioCapture {
    private let engine = AVAudioEngine()
    private(set) var isRunning = false
    let silent: Bool
    var onSamples: (([Float], UInt32) -> Void)?
    var onInterruption: ((Bool, Bool) -> Void)?

    init(silent: Bool) {
        self.silent = silent
        if !silent {
            NotificationCenter.default.addObserver(self, selector: #selector(interrupted(_:)),
                                                   name: AVAudioSession.interruptionNotification, object: nil)
        }
    }

    static var permitted: Bool { AVAudioApplication.shared.recordPermission == .granted }

    /// Ask for the microphone only from a user action, never unattended.
    static func requestPermission() async -> Bool {
        await AVAudioApplication.requestRecordPermission()
    }

    func start() throws {
        guard !silent, !isRunning, AudioCapture.permitted else { return }
        let session = AVAudioSession.sharedInstance()
        // Voice processing (echo cancellation) for later reply playback; the
        // `audio` background mode keeps recording through lock.
        try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.allowBluetoothHFP, .defaultToSpeaker])
        try session.setActive(true)
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        input.installTap(onBus: 0, bufferSize: AVAudioFrameCount(format.sampleRate / 50), format: format) { [weak self] buffer, _ in
            guard let channel = buffer.floatChannelData?[0] else { return }
            let samples = Array(UnsafeBufferPointer(start: channel, count: Int(buffer.frameLength)))
            self?.onSamples?(samples, UInt32(format.sampleRate))
        }
        try engine.start()
        isRunning = true
    }

    func stop() {
        guard isRunning else { return }
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        isRunning = false
    }

    @objc private func interrupted(_ note: Notification) {
        guard let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
              let type = AVAudioSession.InterruptionType(rawValue: raw) else { return }
        switch type {
        case .began:
            onInterruption?(true, false)
        case .ended:
            let options = (note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt).map(AVAudioSession.InterruptionOptions.init) ?? []
            onInterruption?(false, options.contains(.shouldResume))
        @unknown default:
            break
        }
    }
}
