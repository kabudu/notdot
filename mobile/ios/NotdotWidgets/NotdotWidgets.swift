import ActivityKit
import SwiftUI
import WidgetKit

@main
struct NotdotWidgets: WidgetBundle {
    var body: some Widget { NotdotSessionActivity() }
}

/// Lock Screen and Dynamic Island surface for a hands-free session: status
/// in words (not colour alone), time left, and a Stop button that works
/// without unlocking.
struct NotdotSessionActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: NotdotSessionAttributes.self) { context in
            HStack(spacing: 12) {
                Circle().fill(.cyan.opacity(0.8)).frame(width: 28, height: 28).accessibilityHidden(true)
                VStack(alignment: .leading) {
                    Text("notdot").font(.headline)
                    Text(context.state.status).font(.subheadline)
                    Text(timerInterval: Date()...context.state.endsAt, countsDown: true)
                        .font(.caption).monospacedDigit()
                        .accessibilityLabel("Time left")
                }
                Spacer()
                Button(intent: StopHandsFreeIntent()) { Text("Stop") }
                    .frame(minWidth: 44, minHeight: 44)
            }
            .padding()
            .activityBackgroundTint(.black.opacity(0.6))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) { Text("notdot") }
                DynamicIslandExpandedRegion(.trailing) {
                    Button(intent: StopHandsFreeIntent()) { Text("Stop") }
                }
                DynamicIslandExpandedRegion(.bottom) { Text(context.state.status) }
            } compactLeading: {
                Image(systemName: "waveform").accessibilityLabel("notdot listening")
            } compactTrailing: {
                Text(timerInterval: Date()...context.state.endsAt, countsDown: true).monospacedDigit().frame(maxWidth: 44)
            } minimal: {
                Image(systemName: "waveform").accessibilityLabel("notdot listening")
            }
        }
    }
}
