import SwiftUI

/// Settings screen for sensitive actions. Every action names what it does
/// and where; the destructive one needs a confirmation and then Face ID or
/// Touch ID. Buttons are disabled while anything is in progress, so a
/// double tap can't start a second request.
struct SensitiveActionsView: View {
    @ObservedObject var model: SensitiveActionsModel
    @State private var confirmingErase = false
    @AccessibilityFocusState private var outcomeFocused: Bool

    var body: some View {
        Form {
            // Progress and results come first, so they stay on screen at
            // large text sizes.
            if let working = model.working {
                Section {
                    HStack(spacing: 12) {
                        ProgressView()
                        Text(working)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("working")
                }
            }

            if let outcome = model.outcome {
                Section {
                    Label {
                        Text(outcome.text)
                    } icon: {
                        Image(systemName: outcome.isProblem ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                            .foregroundStyle(outcome.isProblem ? .orange : .green)
                            .accessibilityHidden(true)
                    }
                    .accessibilityFocused($outcomeFocused)
                    .accessibilityIdentifier("outcome")
                }
            }
            Section {
                Text(model.statusText)
                    .accessibilityIdentifier("sensitiveStatus")
                if model.status?.enrolled != true {
                    Button("Set up sensitive actions") { Task { await model.setUp() } }
                        .disabled(model.busy || model.status == nil)
                        .accessibilityHint("Creates a key on this phone that only Face ID or Touch ID can use, and asks your computer to verify it.")
                        .accessibilityIdentifier("setUp")
                }
            } header: {
                Text("This phone")
            } footer: {
                Text("Sensitive actions change things on your computer. Each one needs your Face ID or Touch ID, and your computer must allow this phone first.")
            }

            Section {
                Button("Erase assistant memories", role: .destructive) { confirmingErase = true }
                .disabled(!model.canAct)
                .accessibilityHint("Asks you to confirm, then for Face ID or Touch ID.")
                .accessibilityIdentifier("erase")
            } header: {
                Text("Actions")
            } footer: {
                Text("Erases everything the assistant remembers for this profile on your computer. Other profiles aren't affected.")
            }

        }
        .navigationTitle("Sensitive actions")
        // An alert, not a popover: both choices are always shown, and
        // tapping elsewhere never counts as either.
        .alert("Erase this profile's memories?", isPresented: $confirmingErase) {
            Button("Erase memories", role: .destructive) { Task { await model.eraseMemories() } }
            Button("Keep memories", role: .cancel) {}
        } message: {
            Text("The assistant on your computer will forget everything it remembers for this profile. This can't be undone. You'll confirm with Face ID or Touch ID.")
        }
        .task { await model.refresh() }
        .onChange(of: model.outcome) { _, outcome in
            guard let outcome else { return }
            AccessibilityNotification.Announcement(outcome.text).post()
            outcomeFocused = true
        }
    }
}
