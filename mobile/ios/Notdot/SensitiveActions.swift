import CryptoKit
import Foundation
import SwiftUI

/// Sensitive actions from this phone: setting up the step-up key and
/// performing an action the owner confirms with Face ID or Touch ID.
/// These are settings and approval controls; conversation stays voice-only.

/// What the screen needs from the link. The real backend talks to the
/// computer through the Rust core; debug builds can swap in a scripted one
/// to exercise every state of the screen (never compiled into release).
protocol SensitiveActionsBackend: Sendable {
    func status() async throws -> StepUpStatus
    func setUp() async throws -> StepUpProtection
    func resetMemory(requestId: String) async throws -> UInt64
}

/// Which Secure Enclave key is current (its Keychain tag; not a secret),
/// and a pending one whose set-up reply was lost.
enum ActiveStepUpKey {
    private static let key = "dev.notdot.mobile.step-up-tag"
    private static let pendingKey = "dev.notdot.mobile.step-up-pending-tag"
    static let pendingAttest = AppAttest(account: "dev.notdot.mobile.app-attest-key-pending")

    static var store: StepUpKeyStore? {
        UserDefaults.standard.data(forKey: key).map(StepUpKeyStore.init(tag:))
    }
    static var pending: StepUpKeyStore? {
        UserDefaults.standard.data(forKey: pendingKey).map(StepUpKeyStore.init(tag:))
    }

    /// Make `store` (with its App Attest key) current and delete the old.
    static func activate(_ store: StepUpKeyStore, keyId: String) throws {
        try AppAttest.standard.store(keyId: keyId)
        let previous = Self.store
        UserDefaults.standard.set(store.tag, forKey: key)
        if let previous, previous.tag != store.tag { try? previous.delete() }
    }

    /// The computer may have accepted `store`: keep it until we know.
    static func park(_ store: StepUpKeyStore, keyId: String) {
        dropPending()
        guard (try? pendingAttest.store(keyId: keyId)) != nil else { try? store.delete(); return }
        UserDefaults.standard.set(store.tag, forKey: pendingKey)
    }

    /// Settle a pending key from the key the computer says it holds.
    static func reconcile(computerHolds digest: Data?) {
        guard let pending, let keyId = try? pendingAttest.storedKeyId() else { return }
        if let digest, let publicKey = try? pending.publicKey(),
           Data(stepUpKeySha256(publicKey: publicKey)) == digest,
           (try? activate(pending, keyId: keyId)) != nil {
            UserDefaults.standard.removeObject(forKey: pendingKey)
            try? pendingAttest.clear()
        } else {
            dropPending()
        }
    }

    private static func dropPending() {
        try? pending?.delete()
        UserDefaults.standard.removeObject(forKey: pendingKey)
        try? pendingAttest.clear()
    }

    static func clear() {
        try? store?.delete()
        dropPending()
        UserDefaults.standard.removeObject(forKey: key)
        try? AppAttest.standard.clear()
    }
}

/// Creates the step-up key and its evidence for the core. iOS has no
/// attestation for arbitrary Secure Enclave keys, so the evidence is an App
/// Attest attestation binding the new key's public half: Apple vouches for
/// the app, the app for the key.
final class AppleAttester: StepUpAttester, @unchecked Sendable {
    private(set) var created: StepUpKeyStore?
    private(set) var keyId: String?

    func attest(request: AttestRequest) throws -> StepUpEvidence {
        guard StepUpKeyStore.hardwareAvailable else {
            throw MobileError.Unavailable(detail: "This iPhone has no Secure Enclave available to notdot, so sensitive actions stay off.")
        }
        guard AppAttest.supported else { throw AppAttest.unavailable }
        let store = StepUpKeyStore.fresh()
        let publicKey: Data
        do { publicKey = try store.publicKey() } catch {
            throw MobileError.Unavailable(detail: "Couldn't create a key protected by Face ID or Touch ID. Check a passcode and Face ID or Touch ID are set up.")
        }
        created = store
        let hash = try appAttestClientDataHash(request: request, stepUpKey: publicKey)
        let (keyId, attestation) = try AppAttest.standard.attestNewKey(clientDataHash: hash)
        self.keyId = keyId
        guard let id = Data(base64Encoded: keyId) else { throw AppAttest.unavailable }
        return .appleAppAttest(stepUpKey: publicKey, keyId: id, attestation: attestation)
    }

    /// Keep the new keys if the computer accepted them; park them if the
    /// reply was lost (settled at the next status check); else delete.
    func finish(_ outcome: Result<Void, Error>) throws {
        guard let created else { return }
        switch outcome {
        case .success:
            guard let keyId else { throw AppAttest.unavailable }
            do { try ActiveStepUpKey.activate(created, keyId: keyId) } catch {
                try? created.delete()
                throw MobileError.Unavailable(detail: "Couldn't save the new key on this phone. Set up sensitive actions again.")
            }
        case .failure(MobileError.Offline) where keyId != nil:
            ActiveStepUpKey.park(created, keyId: keyId!)
        case .failure:
            try? created.delete()
        }
    }
}

/// Signs one action: Face ID or Touch ID with the step-up key, then an App
/// Attest assertion over the same message (no prompt).
final class AppleSigner: StepUpSigner, @unchecked Sendable {
    let reason: String
    init(reason: String) { self.reason = reason }

    func sign(request: SignRequest) throws -> StepUpSignature {
        guard let store = ActiveStepUpKey.store else {
            throw MobileError.Unavailable(detail: "Set up sensitive actions on this phone first.")
        }
        let signature = try store.sign(request.message, reason: reason)
        let assertion = try AppAttest.standard.assertion(clientDataHash: request.appAttestClientDataHash)
        return StepUpSignature(signature: signature, assertion: assertion)
    }
}

struct CoreSensitiveActions: SensitiveActionsBackend, @unchecked Sendable {
    let link: @MainActor () -> PhoneLink

    func status() async throws -> StepUpStatus {
        let status = try await link().stepUpStatus()
        ActiveStepUpKey.reconcile(computerHolds: status.keySha256)
        return status
    }

    func setUp() async throws -> StepUpProtection {
        let attester = AppleAttester()
        do {
            let protection = try await link().setUpStepUp(attester: attester)
            try attester.finish(.success(()))
            return protection
        } catch {
            try? attester.finish(.failure(error))
            throw error
        }
    }

    func resetMemory(requestId: String) async throws -> UInt64 {
        try await link().resetMemory(
            requestId: requestId,
            signer: AppleSigner(reason: "Confirm erasing this profile's memories on your computer"))
    }
}

/// The outcome of the last attempt, in plain words.
enum SensitiveOutcome: Equatable {
    case done(String)
    case cancelled
    case denied(String)
    case expired
    case offline(String)
    case revoked(String)
    case unavailable(String)
    case failed(String)

    var text: String {
        switch self {
        case .done(let text): text
        case .cancelled: "Cancelled. Nothing changed."
        case .denied(let text): text
        case .expired: "That took too long, so nothing changed. Try again."
        case .offline(let text): "\(text) Try again when your computer is reachable; it won't be done twice."
        case .revoked(let text): text
        case .unavailable(let text): text
        case .failed(let text): "Something went wrong: \(text)"
        }
    }

    var isProblem: Bool {
        if case .done = self { return false }
        return true
    }

    static func from(_ error: Error) -> SensitiveOutcome {
        switch error {
        case MobileError.Cancelled: .cancelled
        case MobileError.Denied(let detail): .denied(detail)
        case MobileError.Expired: .expired
        case MobileError.Offline(let detail): .offline(detail)
        case MobileError.Unpaired(let detail): .revoked(detail)
        case MobileError.Unavailable(let detail): .unavailable(detail)
        case MobileError.Link(let detail), MobileError.Invalid(let detail): .failed(detail)
        default: .failed("\(error)")
        }
    }
}

@MainActor
final class SensitiveActionsModel: ObservableObject {
    @Published private(set) var status: StepUpStatus?
    @Published private(set) var working: String?
    @Published private(set) var outcome: SensitiveOutcome?
    private let backend: SensitiveActionsBackend
    private let onUnpaired: (String) -> Void
    /// Kept after a dropped connection so a retry is the same request and
    /// can never erase twice.
    private(set) var pendingResetId: String?

    init(backend: SensitiveActionsBackend, onUnpaired: @escaping (String) -> Void) {
        self.backend = backend
        self.onUnpaired = onUnpaired
    }

    var busy: Bool { working != nil }

    var statusText: String {
        guard let status else { return "Checking with your computer…" }
        guard status.enrolled, let protection = status.protection else {
            return "Not set up. Set up to confirm sensitive actions with Face ID or Touch ID."
        }
        let verified = describeProtection(protection: protection)
        let test = status.testOnly ? " (test key, not real hardware)"
            : status.development ? " (development build)" : ""
        return status.granted
            ? "Ready. \(verified)\(test)"
            : "Set up. \(verified)\(test) Allow sensitive actions for this phone on your computer to use them."
    }

    var canAct: Bool { !busy && status?.enrolled == true && status?.granted == true }

    func refresh() async {
        do { status = try await backend.status() } catch { finish(error) }
    }

    func setUp() async {
        guard !busy else { return }
        working = "Setting up… this can take a few seconds."
        outcome = nil
        do {
            let protection = try await backend.setUp()
            working = nil
            outcome = .done("Set up. \(describeProtection(protection: protection))")
            await refresh()
        } catch { finish(error) }
    }

    /// Erase this profile's memories. Called only after the owner confirms
    /// the destructive dialog; Face ID or Touch ID follows.
    func eraseMemories() async {
        guard !busy else { return }
        let id = pendingResetId ?? newRequestId()
        pendingResetId = id
        working = "Waiting for Face ID or Touch ID…"
        outcome = nil
        do {
            let removed = try await backend.resetMemory(requestId: id)
            pendingResetId = nil
            working = nil
            outcome = .done(removed == 1 ? "Erased 1 memory." : "Erased \(removed) memories.")
        } catch { finish(error) }
    }

    private func finish(_ error: Error) {
        working = nil
        let outcome = SensitiveOutcome.from(error)
        // Keep the id only when the computer may have done it already.
        if case .offline = outcome {} else { pendingResetId = nil }
        self.outcome = outcome
        if case .revoked(let detail) = outcome {
            status = nil
            onUnpaired(detail)
        }
    }
}

#if DEBUG
/// Debug-only scripted backend for UI tests and visual QA: a named scenario
/// decides each outcome. Selected with `-NotdotSensitiveScenario <name>`.
/// It never talks to a computer and never runs in release builds.
actor ScriptedSensitiveActions: SensitiveActionsBackend {
    let scenario: String
    private var enrolled: Bool
    private var removed: [String: UInt64] = [:]
    private(set) var effects = 0

    init(scenario: String) {
        self.scenario = scenario
        enrolled = !["setup", "unavailable"].contains(scenario)
    }

    static func fromLaunchArguments() -> ScriptedSensitiveActions? {
        let args = ProcessInfo.processInfo.arguments
        guard let i = args.firstIndex(of: "-NotdotSensitiveScenario"), i + 1 < args.count else { return nil }
        return ScriptedSensitiveActions(scenario: args[i + 1])
    }

    func status() async throws -> StepUpStatus {
        StepUpStatus(enrolled: enrolled, protection: enrolled ? .appleAppAttested : nil,
                     granted: enrolled && scenario != "notgranted", testOnly: true,
                     development: false, keySha256: nil)
    }

    func setUp() async throws -> StepUpProtection {
        try await Task.sleep(for: .milliseconds(300))
        if scenario == "unavailable" { throw AppAttest.unavailable }
        enrolled = true
        return .appleAppAttested
    }

    func resetMemory(requestId: String) async throws -> UInt64 {
        try await Task.sleep(for: .milliseconds(300))
        if let done = removed[requestId] { return done }
        switch scenario {
        case "cancelled": throw MobileError.Cancelled(detail: "cancelled")
        case "denied", "notgranted":
            throw MobileError.Denied(detail: "This phone isn't allowed to do that right now. Allow sensitive actions for it on your computer, then try again.")
        case "expired": throw MobileError.Expired(detail: "expired")
        case "offline": throw MobileError.Offline(detail: "Not connected to the computer.")
        case "revoked": throw MobileError.Unpaired(detail: "The computer no longer accepts this phone. Link it again to continue.")
        default:
            effects += 1
            removed[requestId] = 3
            return 3
        }
    }
}
#endif
