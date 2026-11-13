import SwiftUI

/// Voice-only home: the orb, one status line and one control. There is no
/// conversation text field; text appears only in setup (pairing).
struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showingLink = false

    var body: some View {
        ZStack {
            Color(red: 0.02, green: 0.04, blue: 0.08).ignoresSafeArea()
            VStack(spacing: 24) {
                OrbView(state: model.orb)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                Text(model.status)
                    .font(.title3)
                    .foregroundStyle(.white)
                    .accessibilityAddTraits(.updatesFrequently)
                if model.effects.surface {
                    Button("Stop listening") { model.handle(.stop) }
                        .buttonStyle(.borderedProminent)
                        .frame(minHeight: 44)
                } else {
                    Button("Start hands-free") { model.start(source: .app) }
                        .buttonStyle(.borderedProminent)
                        .frame(minHeight: 44)
                        .accessibilityHint("Listens for your companion's name until you stop or it goes quiet.")
                }
                Button("Linked computer") { showingLink = true }
                    .frame(minHeight: 44)
            }
            .padding()
        }
        .sheet(isPresented: $showingLink) { PairingView(link: model.link) }
    }
}

struct PairingView: View {
    @ObservedObject var link: LinkModel
    @State private var qr = ""
    @State private var name = "My phone"
    #if DEBUG
    @State private var noteID = newRequestId()
    #endif

    var body: some View {
        NavigationStack {
            Form {
                Section("Link to your computer") {
                    TextField("Pairing code from the computer", text: $qr)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("This phone's name", text: $name)
                    Button("Link") { Task { await link.pair(qr: qr, name: name) } }
                        .disabled(qr.isEmpty)
                }
                if let code = link.code {
                    Section("Compare this code") {
                        Text(code).font(.largeTitle.monospacedDigit()).accessibilityLabel("Code \(code)")
                    }
                }
                Section("Status") {
                    Text(link.message)
                    Text(link.stepUp).font(.footnote)
                    Text(routeText).accessibilityIdentifier("route")
                }
                #if DEBUG
                Section("Debug") {
                    Button("Reconnect over the internet only") { Task { await link.reconnect(relayOnly: true) } }
                    Button("Add test note (same id each tap)") { Task { await link.addTestNote(id: noteID) } }
                    Text("\(link.notes.count) note(s)").accessibilityIdentifier("noteCount")
                }
                #endif
            }
            .navigationTitle("Linked computer")
        }
    }

    private var routeText: String {
        switch link.route {
        case .direct: "On this network"
        case .relay: "Over the internet (encrypted relay)"
        case .unknown: "Connecting"
        case .disconnected: "Not connected"
        }
    }
}
