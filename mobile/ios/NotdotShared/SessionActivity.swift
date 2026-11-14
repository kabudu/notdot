import ActivityKit
import AppIntents
import Foundation

/// The Live Activity that makes a hands-free session visible and stoppable.
/// A session lasts only as long as this activity (the system ends a Live
/// Activity after at most eight hours); the audio session itself has no
/// documented background limit.
struct NotdotSessionAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        /// "Listening", "Paused by a call", ... (plain language, no colour).
        var status: String
        /// When the session will end if nothing happens.
        var endsAt: Date
    }
}

/// Stop control for the Lock Screen / Dynamic Island. Works without unlock.
struct StopHandsFreeIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "Stop listening"
    static let description = IntentDescription("Ends the hands-free session.")

    @MainActor
    func perform() async throws -> some IntentResult {
        NotificationCenter.default.post(name: .notdotStopHandsFree, object: nil)
        return .result()
    }
}

extension Notification.Name {
    static let notdotStopHandsFree = Notification.Name("notdot.stopHandsFree")
}
