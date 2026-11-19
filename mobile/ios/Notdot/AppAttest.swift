import DeviceCheck
import Foundation
import Security

/// App Attest (DCAppAttestService): a Secure Enclave key that Apple
/// certifies as belonging to a genuine instance of this app on Apple
/// hardware. It can only produce attestations and assertions; it is not the
/// step-up key and Apple says nothing about any other key.
///
/// Needs the App Attest entitlement on an App ID registered with Apple, so
/// it works only in builds signed by an enrolled Apple developer team. It is
/// never supported in the Simulator. Every failure here means "unavailable";
/// there is no fallback.
struct AppAttest {
    /// Where the key identifier is kept (this device only).
    let account: String

    static let standard = AppAttest(account: "dev.notdot.mobile.app-attest-key")

    static var supported: Bool {
        #if targetEnvironment(simulator)
        false
        #else
        DCAppAttestService.shared.isSupported
        #endif
    }

    static let unavailable = MobileError.Unavailable(
        detail: "This iPhone can't prove to your computer that it's running the genuine notdot app (App Attest isn't available), so sensitive actions stay off.")

    /// A new attested key for `clientDataHash`. Blocks; call off the main
    /// thread. Returns the key id and the attestation object.
    func attestNewKey(clientDataHash: Data) throws -> (keyId: String, attestation: Data) {
        guard Self.supported else { throw Self.unavailable }
        let service = DCAppAttestService.shared
        let keyId = try wait { done in service.generateKey { done($0, $1) } }
        let attestation = try wait { done in
            service.attestKey(keyId, clientDataHash: clientDataHash) { done($0, $1) }
        }
        return (keyId, attestation)
    }

    /// An assertion with the stored key. Blocks; no prompt.
    func assertion(clientDataHash: Data) throws -> Data {
        guard Self.supported, let keyId = try storedKeyId() else { throw Self.unavailable }
        return try wait { done in
            DCAppAttestService.shared.generateAssertion(keyId, clientDataHash: clientDataHash) { done($0, $1) }
        }
    }

    private func wait<T>(_ call: (@escaping (T?, Error?) -> Void) -> Void) throws -> T {
        let semaphore = DispatchSemaphore(value: 0)
        var result: Result<T, Error> = .failure(Self.unavailable)
        call { value, error in
            if let value { result = .success(value) } else if let error { result = .failure(error) }
            semaphore.signal()
        }
        // Attestation contacts Apple; give it a bounded wait.
        guard semaphore.wait(timeout: .now() + 60) == .success else {
            throw MobileError.Offline(detail: "Apple's App Attest service didn't answer. Try again.")
        }
        do { return try result.get() } catch let error as DCError where error.code == .serverUnavailable {
            throw MobileError.Offline(detail: "Apple's App Attest service is unavailable. Try again later.")
        } catch let error as MobileError {
            throw error
        } catch {
            // Including a build without the App Attest entitlement.
            throw Self.unavailable
        }
    }

    // MARK: Key id storage (Keychain, this device only, no prompt)

    private var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: "dev.notdot.mobile",
         kSecAttrAccount as String: account]
    }

    func storedKeyId() throws -> String? {
        var query = query
        query[kSecReturnData as String] = true
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else {
            throw MobileError.Unavailable(detail: "Couldn't read this phone's App Attest key (\(status)).")
        }
        return String(data: data, encoding: .utf8)
    }

    func store(keyId: String) throws {
        try clear()
        var item = query
        item[kSecValueData as String] = Data(keyId.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw MobileError.Unavailable(detail: "Couldn't save this phone's App Attest key (\(status)).")
        }
    }

    func clear() throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw MobileError.Unavailable(detail: "Couldn't remove this phone's App Attest key (\(status)).")
        }
    }
}
