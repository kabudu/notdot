//! Android Keystore key attestation, checked on the desktop.
//!
//! Follows <https://developer.android.com/privacy-and-security/security-key-attestation>:
//! the chain must end at one of Google's attestation root keys, no
//! certificate may be on Google's revocation status list, and the key
//! description (OID 1.3.6.1.4.1.11129.2.1.17) must appear only in the leaf.
//! On top of that this verifier insists on exactly the key the app creates:
//! a P-256 signing key generated in a TEE or StrongBox, usable only after a
//! strong biometric for each signature, on a locked, verified-boot device,
//! by an allowed package and signing certificate, attesting the desktop's
//! fresh challenge, and matching the step-up key being registered.
use super::Rejected;
use super::der::{self, Certificate, Reader, oids};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

/// Google's current attestation roots, from
/// <https://android.googleapis.com/attestation/root>. Trust is by public
/// key: older RSA root certificates share the RSA root's key.
const GOOGLE_ROOTS: [&str; 2] = [
    include_str!("roots/google_rsa_root.pem"),
    include_str!("roots/google_ec_root.pem"),
];

pub(crate) fn google_anchors() -> Vec<Vec<u8>> {
    GOOGLE_ROOTS
        .iter()
        .map(|pem| der::spki_of(&der::pem(pem).expect("bundled root")).expect("bundled root"))
        .collect()
}

/// An app allowed to hold step-up keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidApp {
    pub package: String,
    /// SHA-256 digests of the app's signing certificates.
    pub signers: Vec<[u8; 32]>,
}

/// Google's attestation certificate status list
/// (<https://android.googleapis.com/attestation/status>), as fetched by the
/// desktop. Both `REVOKED` and `SUSPENDED` entries are refused.
#[derive(Clone, Debug)]
pub struct StatusList {
    fetched_ms: u64,
    listed: HashSet<String>,
}
impl StatusList {
    /// Parse the JSON document. Keys are serial numbers in lower-case hex
    /// without leading zeros; every entry must have a known status.
    pub fn parse(json: &[u8], fetched_ms: u64) -> Result<Self, Rejected> {
        let bad = Rejected::Malformed("revocation status list");
        let doc: serde_json::Value = serde_json::from_slice(json).map_err(|_| bad.clone())?;
        let entries = doc
            .get("entries")
            .and_then(|e| e.as_object())
            .ok_or(bad.clone())?;
        let mut listed = HashSet::with_capacity(entries.len());
        for (serial, entry) in entries {
            let valid_key = !serial.is_empty()
                && !serial.starts_with('0')
                && serial
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            let status = entry.get("status").and_then(|s| s.as_str());
            if !valid_key || !matches!(status, Some("REVOKED" | "SUSPENDED")) {
                return Err(bad);
            }
            listed.insert(serial.clone());
        }
        Ok(Self { fetched_ms, listed })
    }
    pub fn fetched_ms(&self) -> u64 {
        self.fetched_ms
    }
    pub(crate) fn lists(&self, serial: &[u8]) -> bool {
        let hex: String = serial.iter().map(|b| format!("{b:02x}")).collect();
        let trimmed = hex.trim_start_matches('0');
        self.listed
            .contains(if trimmed.is_empty() { "0" } else { trimmed })
    }
}

/// Oldest status list accepted. Google serves it with a one-day cache
/// lifetime; two days tolerates one missed refresh.
pub const MAX_STATUS_AGE_MS: u64 = 48 * 3_600_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    TrustedEnvironment,
    StrongBox,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    pub level: Level,
    pub package: String,
    #[serde(with = "crate::b64_list")]
    pub signers: Vec<Vec<u8>>,
    pub attestation_version: u64,
    /// Serial numbers of every certificate in the chain, for later
    /// revocation checks.
    #[serde(with = "crate::b64_list")]
    pub serials: Vec<Vec<u8>>,
}

/// Whether an earlier verified chain is still acceptable: its app still
/// allowed, and no certificate listed in a fresh status list.
pub fn still_valid(
    verified: &Verified,
    apps: &[AndroidApp],
    status: Option<&StatusList>,
    now_ms: u64,
) -> Result<(), Rejected> {
    let app = apps
        .iter()
        .find(|app| app.package == verified.package)
        .ok_or(Rejected::App)?;
    if !verified
        .signers
        .iter()
        .all(|d| app.signers.iter().any(|s| s.as_slice() == d.as_slice()))
    {
        return Err(Rejected::App);
    }
    let status = status.ok_or(Rejected::RevocationUnavailable)?;
    if status.fetched_ms > now_ms + der::SKEW_MS as u64
        || now_ms.saturating_sub(status.fetched_ms) > MAX_STATUS_AGE_MS
    {
        return Err(Rejected::RevocationUnavailable);
    }
    if verified.serials.iter().any(|s| status.lists(s)) {
        return Err(Rejected::Revoked);
    }
    Ok(())
}

/// What a key description says, for checks and tests.
#[derive(Debug)]
pub(crate) struct KeyDescription<'a> {
    pub attestation_version: u64,
    pub attestation_level: u64,
    pub keymint_level: u64,
    pub challenge: &'a [u8],
    pub software: BTreeMap<u32, &'a [u8]>,
    pub hardware: BTreeMap<u32, &'a [u8]>,
}

/// Authorization tags (`Tag` numbers without their type bits).
mod tag {
    pub const PURPOSE: u32 = 1;
    pub const ALGORITHM: u32 = 2;
    pub const KEY_SIZE: u32 = 3;
    pub const EC_CURVE: u32 = 10;
    pub const NO_AUTH_REQUIRED: u32 = 503;
    pub const USER_AUTH_TYPE: u32 = 504;
    pub const AUTH_TIMEOUT: u32 = 505;
    pub const ALLOW_WHILE_ON_BODY: u32 = 506;
    pub const ORIGIN: u32 = 702;
    pub const ROOT_OF_TRUST: u32 = 704;
    pub const APPLICATION_ID: u32 = 709;
}
const PURPOSE_SIGN: u64 = 2;
const PURPOSE_VERIFY: u64 = 3;
const ALGORITHM_EC: u64 = 3;
const CURVE_P256: u64 = 1;
const ORIGIN_GENERATED: u64 = 0;
const AUTH_BIOMETRIC: u64 = 2;
const BOOT_VERIFIED: u64 = 0;

/// Each element of an AuthorizationList is `[tag] EXPLICIT value`; tags
/// must be strictly ascending, as DER requires for this SEQUENCE.
fn authorization_list(bytes: &[u8]) -> Result<BTreeMap<u32, &[u8]>, Rejected> {
    let mut reader = Reader::new(bytes);
    let mut list = BTreeMap::new();
    let mut last = None;
    while !reader.is_empty() {
        let tlv = reader.next()?;
        if tlv.class != 0xa0 {
            return Err(Rejected::Malformed("authorization list entry"));
        }
        if last.is_some_and(|last| tlv.number <= last) {
            return Err(Rejected::Malformed("authorization tags out of order"));
        }
        last = Some(tlv.number);
        list.insert(tlv.number, tlv.value);
    }
    Ok(list)
}

pub(crate) fn key_description(extension: &[u8]) -> Result<KeyDescription<'_>, Rejected> {
    let mut fields = Reader::new(der::single(extension, der::SEQUENCE)?);
    let attestation_version = der::uint(fields.take(der::INTEGER)?)?;
    let attestation_level = der::uint(fields.take(der::ENUMERATED)?)?;
    let _keymint_version = der::uint(fields.take(der::INTEGER)?)?;
    let keymint_level = der::uint(fields.take(der::ENUMERATED)?)?;
    let challenge = fields.take(der::OCTET_STRING)?;
    let _unique_id = fields.take(der::OCTET_STRING)?;
    let software = authorization_list(fields.take(der::SEQUENCE)?)?;
    let hardware = authorization_list(fields.take(der::SEQUENCE)?)?;
    fields.finish()?;
    Ok(KeyDescription {
        attestation_version,
        attestation_level,
        keymint_level,
        challenge,
        software,
        hardware,
    })
}

fn int(list: &BTreeMap<u32, &[u8]>, tag: u32) -> Result<Option<u64>, Rejected> {
    list.get(&tag)
        .map(|v| der::uint(der::single(v, der::INTEGER)?))
        .transpose()
}

/// Package names and signing digests from `attestationApplicationId`.
pub(crate) fn application_id(bytes: &[u8]) -> Result<(Vec<String>, Vec<Vec<u8>>), Rejected> {
    let mut fields = Reader::new(der::single(bytes, der::SEQUENCE)?);
    let mut packages = Vec::new();
    let mut infos = Reader::new(fields.take(der::SET)?);
    while !infos.is_empty() {
        let mut info = Reader::new(infos.take(der::SEQUENCE)?);
        let name = info.take(der::OCTET_STRING)?;
        der::uint(info.take(der::INTEGER)?)?;
        info.finish()?;
        packages.push(
            String::from_utf8(name.to_vec()).map_err(|_| Rejected::Malformed("package name"))?,
        );
    }
    let mut digests = Vec::new();
    let mut set = Reader::new(fields.take(der::SET)?);
    while !set.is_empty() {
        digests.push(set.take(der::OCTET_STRING)?.to_vec());
    }
    fields.finish()?;
    Ok((packages, digests))
}

/// Verify an attested step-up key. `chain` is leaf first, root last.
pub fn verify(
    chain: &[Vec<u8>],
    anchors: &[Vec<u8>],
    apps: &[AndroidApp],
    status: Option<&StatusList>,
    challenge: &[u8; 32],
    step_up_key: &[u8],
    now_ms: u64,
) -> Result<Verified, Rejected> {
    if apps.is_empty() {
        return Err(Rejected::NotConfigured);
    }
    let certs = der::verify_chain(chain, anchors, now_ms as i64)?;
    let status = status.ok_or(Rejected::RevocationUnavailable)?;
    if status.fetched_ms > now_ms + der::SKEW_MS as u64
        || now_ms.saturating_sub(status.fetched_ms) > MAX_STATUS_AGE_MS
    {
        return Err(Rejected::RevocationUnavailable);
    }
    if certs.iter().any(|c| status.lists(c.serial)) {
        return Err(Rejected::Revoked);
    }
    let leaf: &Certificate<'_> = &certs[0];
    // Only the certificate nearest the root carrying the extension is
    // trustworthy; for a key the app generated that must be the leaf.
    if certs[1..]
        .iter()
        .any(|c| c.extension(oids::ANDROID_KEY_ATTESTATION).is_some())
    {
        return Err(Rejected::Chain("attestation extension above the leaf"));
    }
    let extension = leaf
        .extension(oids::ANDROID_KEY_ATTESTATION)
        .ok_or(Rejected::NotHardware)?;
    let description = key_description(extension.value)?;
    check(&description, apps, challenge)?;
    if leaf.key_kind != Some(der::KeyKind::P256) || leaf.key != step_up_key {
        return Err(Rejected::WrongKey);
    }
    let level = match (description.attestation_level, description.keymint_level) {
        (1, 1) => Level::TrustedEnvironment,
        (2, 2) => Level::StrongBox,
        _ => return Err(Rejected::NotHardware),
    };
    let (packages, signers) = application_id(app_id(&description)?)?;
    Ok(Verified {
        level,
        package: packages.into_iter().next().unwrap_or_default(),
        signers,
        attestation_version: description.attestation_version,
        serials: certs.iter().map(|c| c.serial.to_vec()).collect(),
    })
}

fn app_id<'a>(description: &KeyDescription<'a>) -> Result<&'a [u8], Rejected> {
    let found = [&description.software, &description.hardware]
        .into_iter()
        .filter_map(|list| list.get(&tag::APPLICATION_ID))
        .collect::<Vec<_>>();
    match found.as_slice() {
        [one] => der::single(one, der::OCTET_STRING),
        _ => Err(Rejected::App),
    }
}

pub(crate) fn check(
    description: &KeyDescription<'_>,
    apps: &[AndroidApp],
    challenge: &[u8; 32],
) -> Result<(), Rejected> {
    use subtle::ConstantTimeEq;
    if description.attestation_version < 3 {
        return Err(Rejected::Unsupported("attestation version"));
    }
    if !matches!(
        (description.attestation_level, description.keymint_level),
        (1, 1) | (2, 2)
    ) {
        return Err(Rejected::NotHardware);
    }
    if description.challenge.len() != 32 || !bool::from(description.challenge.ct_eq(challenge)) {
        return Err(Rejected::Challenge);
    }
    let hw = &description.hardware;
    let sw = &description.software;
    // Key shape: a P-256 signing key generated inside the secure hardware.
    let purposes = hw
        .get(&tag::PURPOSE)
        .ok_or(Rejected::KeyProperties("purpose"))?;
    let mut set = Reader::new(der::single(purposes, der::SET)?);
    let mut signs = false;
    while !set.is_empty() {
        match der::uint(set.take(der::INTEGER)?)? {
            PURPOSE_SIGN => signs = true,
            PURPOSE_VERIFY => {}
            _ => return Err(Rejected::KeyProperties("purpose")),
        }
    }
    if !signs {
        return Err(Rejected::KeyProperties("purpose"));
    }
    if int(hw, tag::ALGORITHM)? != Some(ALGORITHM_EC)
        || int(hw, tag::EC_CURVE)? != Some(CURVE_P256)
        || int(hw, tag::KEY_SIZE)?.is_some_and(|size| size != 256)
    {
        return Err(Rejected::KeyProperties("algorithm"));
    }
    if int(hw, tag::ORIGIN)? != Some(ORIGIN_GENERATED) {
        return Err(Rejected::KeyProperties("origin"));
    }
    // Every signature needs a fresh strong biometric, enforced in hardware.
    if hw.contains_key(&tag::NO_AUTH_REQUIRED) || sw.contains_key(&tag::NO_AUTH_REQUIRED) {
        return Err(Rejected::KeyProperties("user authentication"));
    }
    if int(hw, tag::USER_AUTH_TYPE)? != Some(AUTH_BIOMETRIC) {
        return Err(Rejected::KeyProperties("user authentication"));
    }
    let timeout = match hw.get(&tag::AUTH_TIMEOUT).or(sw.get(&tag::AUTH_TIMEOUT)) {
        Some(v) => Some(der::uint(der::single(v, der::INTEGER)?)?),
        None => None,
    };
    if timeout.is_some_and(|t| t != 0)
        || hw.contains_key(&tag::ALLOW_WHILE_ON_BODY)
        || sw.contains_key(&tag::ALLOW_WHILE_ON_BODY)
    {
        return Err(Rejected::KeyProperties("user authentication"));
    }
    device_integrity(hw)?;
    // The app: exactly one package, every signer allowed for it.
    let (packages, digests) = application_id(app_id(description)?)?;
    let [package] = packages.as_slice() else {
        return Err(Rejected::App);
    };
    let app = apps
        .iter()
        .find(|app| &app.package == package)
        .ok_or(Rejected::App)?;
    if digests.is_empty()
        || !digests
            .iter()
            .all(|d| app.signers.iter().any(|s| s.as_slice() == d.as_slice()))
    {
        return Err(Rejected::App);
    }
    Ok(())
}

/// Device integrity: a locked bootloader with verified boot.
pub(crate) fn device_integrity(hw: &BTreeMap<u32, &[u8]>) -> Result<(), Rejected> {
    let root = hw
        .get(&tag::ROOT_OF_TRUST)
        .ok_or(Rejected::DeviceIntegrity)?;
    let mut fields = Reader::new(der::single(root, der::SEQUENCE)?);
    let _boot_key = fields.take(der::OCTET_STRING)?;
    let locked = der::boolean(fields.take(der::BOOLEAN)?)?;
    let state = der::uint(fields.take(der::ENUMERATED)?)?;
    if !fields.is_empty() {
        fields.take(der::OCTET_STRING)?;
    }
    fields.finish()?;
    if !locked || state != BOOT_VERIFIED {
        return Err(Rejected::DeviceIntegrity);
    }
    Ok(())
}

/// SHA-256 of a signing certificate, the form `AndroidApp::signers` takes.
pub fn signer_digest(certificate: &[u8]) -> [u8; 32] {
    Sha256::digest(certificate).into()
}
