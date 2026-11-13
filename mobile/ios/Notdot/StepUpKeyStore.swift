import CryptoKit
import Foundation
import LocalAuthentication
import Security

/// The owner step-up key: a P-256 key created inside the Secure Enclave.
///
/// The private key never leaves the Secure Enclave, and every signature
/// needs Face ID or Touch ID with the currently enrolled biometrics (a new
/// enrolment invalidates the key). Creating or finding the key never shows a
/// prompt; only `sign`, called for an action the owner just started, does.
///
/// There is no fallback: without a Secure Enclave (the Simulator, or a
/// phone without enrolled biometrics) this reports `noSecureHardware` and
/// sensitive actions stay unavailable.
///
/// Apple offers no attestation for such a key. The computer learns about
/// it through App Attest instead (see `SensitiveActions.swift`): Apple
/// vouches for the app, and the app vouches for this key.
struct StepUpKeyStore {
    /// Keychain application tag; tests use a disposable one.
    let tag: Data

    /// A store for a fresh tag, so a new key never replaces the current
    /// one until the computer has accepted it.
    static func fresh() -> StepUpKeyStore {
        StepUpKeyStore(tag: Data("dev.notdot.mobile.step-up.\(UUID().uuidString)".utf8))
    }

    enum Failure: Error, Equatable, CustomStringConvertible {
        /// No Secure Enclave, or no biometrics enrolled.
        case noSecureHardware(String)
        case keychain(OSStatus)

        var description: String {
            switch self {
            case .noSecureHardware(let why): why
            case .keychain(let status): "Keychain error \(status)"
            }
        }
    }

    /// Whether this device has a Secure Enclave. Always false in the
    /// Simulator: it reports one, but it is not a phone's hardware, so a key
    /// made there must never be labelled as Secure Enclave-backed.
    static var hardwareAvailable: Bool {
        #if targetEnvironment(simulator)
        false
        #else
        SecureEnclave.isAvailable
        #endif
    }

    /// The existing key's public half, or a new key's. Never prompts.
    func publicKey() throws -> Data {
        try Self.publicKey(of: try existing() ?? create())
    }

    /// Sign with this key: the system asks for Face ID or Touch ID first.
    /// Blocks until the owner answers; call off the main thread.
    func sign(_ message: Data, reason: String) throws -> Data {
        let context = LAContext()
        context.localizedReason = reason
        guard let key = try existing(context: context) else {
            throw MobileError.Unavailable(detail: "Sensitive actions aren't set up on this phone.")
        }
        return try Self.sign(message, with: key)
    }

    func delete() throws {
        let status = SecItemDelete(lookup() as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw Failure.keychain(status) }
    }

    /// The SEC1 (X9.63, 65-byte uncompressed) public key the core expects.
    static func publicKey(of privateKey: SecKey) throws -> Data {
        var error: Unmanaged<CFError>?
        guard let publicKey = SecKeyCopyPublicKey(privateKey),
              let bytes = SecKeyCopyExternalRepresentation(publicKey, &error) as Data? else {
            throw Failure.keychain(errSecInvalidKeyRef)
        }
        return bytes
    }

    /// ECDSA P-256 / SHA-256, DER encoded, as the desktop verifies it.
    static func sign(_ message: Data, with privateKey: SecKey) throws -> Data {
        var error: Unmanaged<CFError>?
        guard let signature = SecKeyCreateSignature(privateKey, .ecdsaSignatureMessageX962SHA256,
                                                    message as CFData, &error) as Data? else {
            let failure = error?.takeRetainedValue()
            // The owner dismissed Face ID / Touch ID, or chose Cancel.
            if let failure, CFErrorGetCode(failure) == Int(errSecUserCanceled)
                || CFErrorGetCode(failure) == LAError.userCancel.rawValue
                || CFErrorGetCode(failure) == LAError.appCancel.rawValue
                || CFErrorGetCode(failure) == LAError.systemCancel.rawValue {
                throw MobileError.Cancelled(detail: "Not confirmed; nothing changed.")
            }
            let detail = failure.map { CFErrorCopyDescription($0) as String } ?? "signing failed"
            throw MobileError.Denied(detail: "Not confirmed: \(detail)")
        }
        return signature
    }

    private func lookup(context: LAContext? = nil) -> [String: Any] {
        var query: [String: Any] = [kSecClass as String: kSecClassKey,
                                    kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
                                    kSecAttrApplicationTag as String: tag,
                                    kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave]
        if let context { query[kSecUseAuthenticationContext as String] = context }
        return query
    }

    private func existing(context: LAContext? = nil) throws -> SecKey? {
        var query = lookup(context: context)
        query[kSecReturnRef as String] = true
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let result else { throw Failure.keychain(status) }
        return (result as! SecKey)
    }

    private func create() throws -> SecKey {
        guard Self.hardwareAvailable else {
            throw Failure.noSecureHardware("This device has no Secure Enclave (or is the Simulator).")
        }
        var error: Unmanaged<CFError>?
        guard let access = SecAccessControlCreateWithFlags(
            nil, kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
            [.privateKeyUsage, .biometryCurrentSet], &error) else {
            throw Failure.noSecureHardware("Could not set the key's access policy.")
        }
        let attributes: [String: Any] = [
            kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeySizeInBits as String: 256,
            kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
            kSecPrivateKeyAttrs as String: [kSecAttrIsPermanent as String: true,
                                           kSecAttrApplicationTag as String: tag,
                                           kSecAttrAccessControl as String: access],
        ]
        guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &error) else {
            let detail = error?.takeRetainedValue().localizedDescription ?? "unknown error"
            // Typically: no passcode or no enrolled biometrics.
            throw Failure.noSecureHardware("The Secure Enclave key could not be created: \(detail)")
        }
        return key
    }
}
