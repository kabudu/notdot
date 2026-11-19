import AppIntents
import Foundation

/// One press (Action button, Control) or one Siri phrase starts a hands-free
/// session. iOS 18 AudioRecordingIntent: the system shows a recording
/// indicator, and the app must keep a Live Activity active while recording.
struct StartHandsFreeIntent: AudioRecordingIntent {
    static let title: LocalizedStringResource = "Start hands-free"
    static let description = IntentDescription("Starts listening for your companion's name.")

    @MainActor
    func perform() async throws -> some IntentResult {
        AppModel.shared.start(source: .systemAction)
        return .result()
    }
}

struct NotdotShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: StartHandsFreeIntent(),
            phrases: ["Talk to \(.applicationName)", "Start \(.applicationName) hands-free"],
            shortTitle: "Start hands-free",
            systemImageName: "waveform"
        )
    }
}
