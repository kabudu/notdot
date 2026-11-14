import Foundation

/// Pairing and the encrypted link to the computer (via the Rust core).
/// The phone's identity and the paired computer are kept in the Keychain
/// (see IdentityStore), so a restarted app reconnects without pairing.
@MainActor
final class LinkModel: ObservableObject {
    @Published private(set) var code: String?
    @Published private(set) var message = "Not linked"
    @Published private(set) var route: LinkRoute = .disconnected
    @Published private(set) var notes: [String] = []
    /// Sensitive actions (set up, confirm, results); created once.
    private(set) var sensitive: SensitiveActionsModel!
    private let store: IdentityStore
    private var link: PhoneLink

    init(store: IdentityStore = .standard) {
        self.store = store
        // Restoring reads the Keychain only; nothing is created until the
        // user starts pairing. If it cannot be read, start unpaired.
        if let identity = try? store.identity(),
           let restored = try? PhoneLink.restore(identity: identity, paired: try? store.pairedRecord()) {
            link = restored
            message = (try? store.pairedRecord()) == nil ? "Not linked" : "Linked; not connected"
        } else {
            link = PhoneLink()
        }
        var backend: SensitiveActionsBackend = CoreSensitiveActions(link: { [unowned self] in self.link })
        #if DEBUG
        if let scripted = ScriptedSensitiveActions.fromLaunchArguments() { backend = scripted }
        #endif
        sensitive = SensitiveActionsModel(backend: backend) { [unowned self] detail in self.forget(detail) }
    }

    func pair(qr: String, name: String) async {
        do {
            // The identity is created (once) by this user action.
            if try store.identity() == nil {
                link = try PhoneLink.restore(identity: try store.identityCreatingIfNeeded(), paired: nil)
            }
            code = try await link.pair(qr: qr, name: name)
            message = "Check the computer shows the same code, then choose Allow there."
            switch try await link.awaitApproval() {
            case .approved:
                try store.savePairedRecord(await link.pairedRecord())
                message = "Linked"
                route = try await link.connect(relayOnly: false)
            case .denied:
                message = "The computer said no."
            case .rejected(let reason):
                message = reason
            }
        } catch {
            report(error)
        }
    }

    /// Show an error. If the computer no longer accepts this phone, forget
    /// everything stored for the link, so the next pairing starts afresh.
    private func report(_ error: Error) {
        guard case MobileError.Unpaired(let detail) = error else {
            message = "\(error)"
            return
        }
        forget(detail)
    }

    /// Delete the identity, paired record and step-up keys.
    private func forget(_ detail: String) {
        try? store.clear()
        ActiveStepUpKey.clear()
        link = PhoneLink()
        code = nil
        route = .disconnected
        notes = []
        message = detail
    }

    func reconnect(relayOnly: Bool) async {
        do { route = try await link.connect(relayOnly: relayOnly) } catch { report(error) }
    }

    func refreshNotes() async {
        do { notes = try await link.listNotes() } catch { report(error) }
    }

    #if DEBUG
    /// Debug-only: proves idempotent retries. Not a conversation surface.
    func addTestNote(id: String) async {
        do { _ = try await link.addNote(requestId: id, text: "Synthetic test note"); await refreshNotes() } catch { report(error) }
    }
    #endif
}
