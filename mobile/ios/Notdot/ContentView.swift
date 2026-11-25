import SwiftUI
import VisionKit
import AVFoundation
import UniformTypeIdentifiers

/// Voice-only home: the orb, one status line and Tap to talk (the default
/// way to speak to the computer); hands-free wake is an opt-in experiment.
/// There is no conversation text field; text appears only in setup (pairing).
struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showingLink = Self.openSensitive
    #if DEBUG
    /// Debug-only: open the sensitive-actions screen at launch (UI tests).
    static let openSensitive = ProcessInfo.processInfo.arguments.contains("-NotdotSensitiveScenario")
    #else
    static let openSensitive = false
    #endif

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
                Button(talkTitle) { model.tapTalk() }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .frame(minHeight: 44)
                    .accessibilityHint("Listens for one request, sends it to your computer and plays the reply.")
                if model.effects.surface {
                    Button("Stop hands-free") { model.handle(.stop) }
                        .frame(minHeight: 44)
                } else {
                    Button("Hands-free (experimental)") { model.start(source: .app) }
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

    private var talkTitle: String {
        switch model.talk {
        case .idle: "Tap to talk"
        case .listening: "Listening · tap to stop"
        case .replying: "Stop"
        }
    }
}

struct PairingView: View {
    @ObservedObject var link: LinkModel
    @State private var path: [String] = ContentView.openSensitive ? ["sensitive"] : []
    @State private var scanning = false
    @State private var pairing = false
    @State private var scannerMessage = ""
    @State private var name = "My phone"
    #if DEBUG
    @State private var noteID = newRequestId()
    #endif

    var body: some View {
        NavigationStack(path: $path) {
            Form {
                Section("Link to your computer") {
                    TextField("This phone's name", text: $name)
                    Button("Scan QR code") { Task { await openScanner() } }
                        .disabled(pairing)
                    PasteButton(payloadType: String.self) { values in
                        if let invitation = values.first { acceptInvitation(invitation) }
                    }
                    .accessibilityLabel("Paste pairing code")
                    .disabled(pairing)
                    if !scannerMessage.isEmpty { Text(scannerMessage) }
                }
                if let code = link.code {
                    Section("Compare this code") {
                        Text(code).font(.largeTitle.monospacedDigit()).accessibilityLabel("Code \(code)")
                    }
                }
                Section("Status") {
                    Text(link.message)
                    Text(routeText).accessibilityIdentifier("route")
                }
                Section {
                    NavigationLink("Sensitive actions", value: "sensitive")
                        .accessibilityHint("Set up Face ID or Touch ID confirmation, and erase memories.")
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
            .navigationDestination(for: String.self) { _ in
                SensitiveActionsView(model: link.sensitive)
            }
        }
        .sheet(isPresented: $scanning) {
            NavigationStack {
                PairingScanner { invitation in
                    scanning = false
                    if let invitation { acceptInvitation(invitation) }
                    else { scannerMessage = "Could not start the camera. Try again or paste the pairing code." }
                }
                .navigationTitle("Scan pairing QR code")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { scanning = false }
                    }
                }
            }
        }
    }

    @MainActor
    private func openScanner() async {
        guard DataScannerViewController.isSupported else {
            scannerMessage = "Camera scanning is unavailable. Copy the pairing code from your computer, then paste it here."
            return
        }
        guard await AVCaptureDevice.requestAccess(for: .video), DataScannerViewController.isAvailable else {
            scannerMessage = "Allow camera access in Settings, or paste the pairing code."
            return
        }
        scannerMessage = ""
        scanning = true
    }

    private func acceptInvitation(_ invitation: String) {
        guard !pairing else { return }
        let code = invitation.trimmingCharacters(in: .whitespacesAndNewlines)
        guard code.hasPrefix("notdot-pair:1:") else {
            scannerMessage = "That is not a notdot pairing QR code."
            return
        }
        scannerMessage = ""
        pairing = true
        Task {
            await link.pair(qr: code, name: name)
            pairing = false
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


@MainActor
private struct PairingScanner: UIViewControllerRepresentable {
    var result: (String?) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(result: result) }

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])])
        scanner.delegate = context.coordinator
        do { try scanner.startScanning() }
        catch {
            let coordinator = context.coordinator
            Task { coordinator.finish(nil) }
        }
        return scanner
    }

    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ scanner: DataScannerViewController, coordinator: Coordinator) {
        scanner.stopScanning()
    }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        private var result: ((String?) -> Void)?

        init(result: @escaping (String?) -> Void) { self.result = result }

        func finish(_ invitation: String?) {
            let callback = result
            result = nil
            callback?(invitation)
        }

        func dataScanner(_ scanner: DataScannerViewController,
                         didAdd addedItems: [RecognizedItem], allItems: [RecognizedItem]) {
            for case .barcode(let barcode) in addedItems {
                if let invitation = barcode.payloadStringValue {
                    scanner.stopScanning()
                    finish(invitation)
                    return
                }
            }
        }

        func dataScanner(_ scanner: DataScannerViewController,
                         becameUnavailableWithError error: DataScannerViewController.ScanningUnavailable) {
            finish(nil)
        }
    }
}
