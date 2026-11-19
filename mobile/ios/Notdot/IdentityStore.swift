import Foundation
import Security

/// The phone's link identity and paired-desktop record in the Keychain.
///
/// Items are this-device-only (never in backups or iCloud Keychain) and
/// readable after the first unlock, so a hands-free session can reconnect
/// while the phone is locked. No access control (biometrics or passcode)
/// is attached, so reading never shows a prompt. The identity is created
/// only when the user starts pairing.
struct IdentityStore {
    /// Keychain service; tests use a disposable one.
    let service: String

    static let standard = IdentityStore(service: "dev.notdot.mobile.link")

    private static let identityAccount = "phone-identity"
    private static let pairedAccount = "paired-desktop"

    enum Failure: Error, Equatable {
        case keychain(OSStatus)
    }

    func identity() throws -> Data? { try read(Self.identityAccount) }

    /// The stored identity, or a new one saved before it is returned.
    func identityCreatingIfNeeded() throws -> Data {
        if let existing = try identity() { return existing }
        let created = newIdentity()
        try write(Self.identityAccount, created)
        return created
    }

    func pairedRecord() throws -> String? {
        try read(Self.pairedAccount).flatMap { String(data: $0, encoding: .utf8) }
    }

    func savePairedRecord(_ record: String?) throws {
        if let record { try write(Self.pairedAccount, Data(record.utf8)) } else { try delete(Self.pairedAccount) }
    }

    /// Remove everything (unpair and forget this phone's identity).
    func clear() throws {
        try delete(Self.pairedAccount)
        try delete(Self.identityAccount)
    }

    private func query(_ account: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: account,
         kSecAttrSynchronizable as String: false]
    }

    private func read(_ account: String) throws -> Data? {
        var q = query(account)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw Failure.keychain(status) }
        return result as? Data
    }

    private func write(_ account: String, _ data: Data) throws {
        let update = [kSecValueData as String: data,
                      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly] as [String: Any]
        var status = SecItemUpdate(query(account) as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            status = SecItemAdd(query(account).merging(update) { $1 } as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw Failure.keychain(status) }
    }

    private func delete(_ account: String) throws {
        let status = SecItemDelete(query(account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw Failure.keychain(status) }
    }
}
