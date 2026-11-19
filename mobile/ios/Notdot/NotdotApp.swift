import SwiftUI

@main
struct NotdotApp: App {
    @StateObject private var model = AppModel.shared
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
                .preferredColorScheme(.dark)
        }
        .onChange(of: phase) { _, phase in model.scenePhase(phase) }
    }
}
