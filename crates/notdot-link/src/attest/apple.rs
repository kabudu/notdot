//! Apple App Attest, checked on the desktop.
//!
//! Follows "Validating apps that connect to your server" in Apple's
//! DeviceCheck documentation. What App Attest proves: the attested key was
//! created by `DCAppAttestService` in the Secure Enclave of genuine Apple
//! hardware, for an app with this App ID, and only that app instance can
//! produce assertions with it. What it does not prove: anything about other
//! keys. The step-up key is a separate Secure Enclave key; the app binds its
//! public key into the attested client data, so the desktop learns that a
//! genuine instance of the app vouched for it, not that the hardware did.
use super::Rejected;
use super::der::{self, oids};
use sha2::{Digest, Sha256};

const APPLE_ROOT: &str = include_str!("roots/apple_app_attestation_root.pem");

pub(crate) fn apple_root() -> Vec<u8> {
    der::pem(APPLE_ROOT).expect("bundled root")
}

/// An app allowed to enrol, by App ID (team identifier and bundle ID).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppleApp {
    pub team_id: String,
    pub bundle_id: String,
}
impl AppleApp {
    pub fn rp_id_hash(&self) -> [u8; 32] {
        Sha256::digest(format!("{}.{}", self.team_id, self.bundle_id)).into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Development,
    Production,
}

const AAGUID_DEVELOPMENT: &[u8; 16] = b"appattestdevelop";
const AAGUID_PRODUCTION: &[u8; 16] = b"appattest\0\0\0\0\0\0\0";
const FLAG_ATTESTED: u8 = 0x40;
const FLAG_EXTENSIONS: u8 = 0x80;
const CATEGORY_KEY: &str = "apple_validation_category_01";
const BUNDLE_VERSION_KEY: &str = "apple_bundle_version_01";
/// Most entries in one CBOR array or map.
pub const MAX_CBOR_ITEMS: u64 = 64;
/// Largest attestation or assertion object accepted.
pub const MAX_OBJECT_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cbor<'a> {
    Uint(u64),
    /// The value is `-1 - n`.
    Negative(u64),
    Bytes(&'a [u8]),
    Text(&'a str),
    Array(Vec<Cbor<'a>>),
    Map(Vec<(Cbor<'a>, Cbor<'a>)>),
}
impl<'a> Cbor<'a> {
    fn get(&self, key: &Cbor<'_>) -> Option<&Cbor<'a>> {
        match self {
            Cbor::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn text_key(&self, key: &str) -> Option<&Cbor<'a>> {
        self.get(&Cbor::Text(key))
    }
    fn bytes(&self) -> Option<&'a [u8]> {
        match self {
            Cbor::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

/// Strict CBOR: definite lengths, shortest integer encodings, no tags,
/// floats or simple values, unique map keys, bounded nesting.
pub fn cbor(bytes: &[u8]) -> Result<(Cbor<'_>, &[u8]), Rejected> {
    item(bytes, 0)
}

fn take(rest: &[u8], n: u64) -> Result<(&[u8], &[u8]), Rejected> {
    let n = usize::try_from(n).map_err(|_| Rejected::Malformed("bad CBOR"))?;
    if rest.len() < n {
        return Err(Rejected::Malformed("bad CBOR"));
    }
    Ok(rest.split_at(n))
}

fn item(bytes: &[u8], depth: usize) -> Result<(Cbor<'_>, &[u8]), Rejected> {
    let bad = Rejected::Malformed("bad CBOR");
    if depth > 8 {
        return Err(bad);
    }
    let (&first, rest) = bytes.split_first().ok_or(bad.clone())?;
    let major = first >> 5;
    let info = first & 0x1f;
    let (arg, mut rest) = match info {
        0..=23 => (u64::from(info), rest),
        24..=27 => {
            let n = 1usize << (info - 24);
            if rest.len() < n {
                return Err(bad);
            }
            let (octets, tail) = rest.split_at(n);
            let value = octets.iter().fold(0u64, |v, &b| (v << 8) | u64::from(b));
            // Shortest form only.
            let min = match n {
                1 => 24,
                2 => 0x100,
                4 => 0x1_0000,
                _ => 0x1_0000_0000,
            };
            if value < min {
                return Err(bad);
            }
            (value, tail)
        }
        _ => return Err(bad),
    };
    let value = match major {
        0 => Cbor::Uint(arg),
        1 => Cbor::Negative(arg),
        2 => {
            let (b, tail) = take(rest, arg)?;
            rest = tail;
            Cbor::Bytes(b)
        }
        3 => {
            let (b, tail) = take(rest, arg)?;
            rest = tail;
            Cbor::Text(std::str::from_utf8(b).map_err(|_| bad.clone())?)
        }
        4 => {
            // Real objects have a handful of entries; a cap keeps hostile
            // input (and the duplicate-key check below) cheap.
            if arg > MAX_CBOR_ITEMS || arg > rest.len() as u64 {
                return Err(bad);
            }
            let mut items = Vec::with_capacity(arg as usize);
            for _ in 0..arg {
                let (v, tail) = item(rest, depth + 1)?;
                items.push(v);
                rest = tail;
            }
            Cbor::Array(items)
        }
        5 => {
            if arg > MAX_CBOR_ITEMS || arg.saturating_mul(2) > rest.len() as u64 {
                return Err(bad);
            }
            let mut entries: Vec<(Cbor<'_>, Cbor<'_>)> = Vec::with_capacity(arg as usize);
            for _ in 0..arg {
                let (k, tail) = item(rest, depth + 1)?;
                let (v, tail) = item(tail, depth + 1)?;
                if entries.iter().any(|(existing, _)| existing == &k) {
                    return Err(bad);
                }
                entries.push((k, v));
                rest = tail;
            }
            Cbor::Map(entries)
        }
        _ => return Err(bad),
    };
    Ok((value, rest))
}

/// Parsed authenticator data.
struct AuthData<'a> {
    rp_id_hash: &'a [u8],
    counter: u32,
    aaguid: Option<&'a [u8]>,
    credential_id: Option<&'a [u8]>,
    credential_key: Option<Cbor<'a>>,
    extensions: Option<Cbor<'a>>,
}

fn auth_data(bytes: &[u8]) -> Result<AuthData<'_>, Rejected> {
    let bad = Rejected::Malformed("authenticator data");
    if bytes.len() < 37 {
        return Err(bad);
    }
    let rp_id_hash = &bytes[..32];
    let flags = bytes[32];
    let counter = u32::from_be_bytes(bytes[33..37].try_into().expect("4 bytes"));
    let mut rest = &bytes[37..];
    let (mut aaguid, mut credential_id, mut credential_key) = (None, None, None);
    if flags & FLAG_ATTESTED != 0 {
        if rest.len() < 18 {
            return Err(bad);
        }
        aaguid = Some(&rest[..16]);
        let len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
        rest = &rest[18..];
        if rest.len() < len {
            return Err(bad);
        }
        credential_id = Some(&rest[..len]);
        let (key, tail) = cbor(&rest[len..])?;
        credential_key = Some(key);
        rest = tail;
    }
    let mut extensions = None;
    // Apple's own sample carries the extensions map without setting the
    // extension-data flag, so trailing bytes must be exactly one map.
    if flags & FLAG_EXTENSIONS != 0 || !rest.is_empty() {
        let (map, tail) = cbor(rest)?;
        if !matches!(map, Cbor::Map(_)) {
            return Err(bad);
        }
        extensions = Some(map);
        rest = tail;
    }
    if !rest.is_empty() {
        return Err(bad);
    }
    Ok(AuthData {
        rp_id_hash,
        counter,
        aaguid,
        credential_id,
        credential_key,
        extensions,
    })
}

/// The launch validation category and bundle version Apple reports.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppFacts {
    pub validation_category: u32,
    pub bundle_version: String,
}

fn app_facts(extensions: Option<&Cbor<'_>>) -> Result<Option<AppFacts>, Rejected> {
    let Some(extensions) = extensions else {
        return Ok(None);
    };
    let bad = Rejected::Malformed("App Attest extensions");
    // A UInt32, carried as four little-endian bytes in Apple's sample.
    let category = match extensions.text_key(CATEGORY_KEY) {
        Some(Cbor::Bytes(b)) if b.len() == 4 => {
            u32::from_le_bytes((*b).try_into().expect("4 bytes"))
        }
        Some(Cbor::Uint(n)) => u32::try_from(*n).map_err(|_| bad.clone())?,
        _ => return Err(bad),
    };
    let version = match extensions.text_key(BUNDLE_VERSION_KEY) {
        Some(Cbor::Text(t)) if !t.is_empty() && t.len() <= 64 => t.to_string(),
        _ => return Err(bad),
    };
    Ok(Some(AppFacts {
        validation_category: category,
        bundle_version: version,
    }))
}

/// Policy for App Attest.
#[derive(Clone, Debug)]
pub struct ApplePolicy {
    pub apps: Vec<AppleApp>,
    /// Accept keys from the App Attest development (sandbox) environment.
    pub allow_development: bool,
    /// Launch validation categories accepted (Apple's raw values).
    pub validation_categories: Vec<u32>,
}

/// A verified App Attest key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// The attested key, an uncompressed P-256 point.
    pub key: Vec<u8>,
    pub key_id: [u8; 32],
    pub rp_id_hash: [u8; 32],
    pub environment: Environment,
    pub facts: AppFacts,
    /// Unverified here: only Apple's fraud-metric service checks it, which
    /// needs a developer account and server-to-server access to Apple.
    pub receipt: Vec<u8>,
}

/// Verify an attestation object for `key_id` and `client_data_hash`.
pub fn verify_attestation(
    object: &[u8],
    key_id: &[u8; 32],
    client_data_hash: &[u8],
    anchors: &[Vec<u8>],
    root: &[u8],
    policy: &ApplePolicy,
    now_ms: u64,
) -> Result<Verified, Rejected> {
    use subtle::ConstantTimeEq;
    if policy.apps.is_empty() || policy.validation_categories.is_empty() {
        return Err(Rejected::NotConfigured);
    }
    if object.len() > MAX_OBJECT_BYTES {
        return Err(Rejected::Malformed("attestation too large"));
    }
    let (top, rest) = cbor(object)?;
    if !rest.is_empty() {
        return Err(Rejected::Malformed("trailing attestation bytes"));
    }
    let bad = || Rejected::Malformed("attestation object");
    if top.text_key("fmt") != Some(&Cbor::Text("apple-appattest")) {
        return Err(Rejected::Unsupported("attestation format"));
    }
    let statement = top.text_key("attStmt").ok_or_else(bad)?;
    let auth_bytes = top
        .text_key("authData")
        .and_then(Cbor::bytes)
        .ok_or_else(bad)?;
    let receipt = statement
        .text_key("receipt")
        .and_then(Cbor::bytes)
        .ok_or_else(bad)?;
    let Some(Cbor::Array(x5c)) = statement.text_key("x5c") else {
        return Err(bad());
    };
    let mut chain = x5c
        .iter()
        .map(|c| c.bytes().map(<[u8]>::to_vec).ok_or_else(bad))
        .collect::<Result<Vec<_>, _>>()?;
    // x5c holds the credential certificate and intermediate; the root is
    // pinned locally.
    if chain.len() != 2 {
        return Err(Rejected::Chain("App Attest certificate count"));
    }
    chain.push(root.to_vec());
    let certs = der::verify_chain(&chain, anchors, now_ms as i64)?;
    let leaf = &certs[0];
    // nonce = SHA256(authData || clientDataHash), in the leaf's extension.
    let nonce: [u8; 32] = Sha256::new()
        .chain_update(auth_bytes)
        .chain_update(client_data_hash)
        .finalize()
        .into();
    let extension = leaf
        .extension(oids::APPLE_NONCE)
        .ok_or(Rejected::Challenge)?;
    let mut sequence = der::Reader::new(der::single(extension.value, der::SEQUENCE)?);
    let tagged = sequence.next()?;
    sequence.finish()?;
    if !tagged.is_context(1) {
        return Err(Rejected::Malformed("App Attest nonce"));
    }
    let embedded = der::single(tagged.value, der::OCTET_STRING)?;
    if embedded.len() != 32 || !bool::from(embedded.ct_eq(&nonce)) {
        return Err(Rejected::Challenge);
    }
    if leaf.key_kind != Some(der::KeyKind::P256) || leaf.key.len() != 65 {
        return Err(Rejected::WrongKey);
    }
    let computed_id: [u8; 32] = Sha256::digest(leaf.key).into();
    if &computed_id != key_id {
        return Err(Rejected::WrongKey);
    }
    let auth = auth_data(auth_bytes)?;
    let app = policy
        .apps
        .iter()
        .find(|app| app.rp_id_hash().as_slice() == auth.rp_id_hash)
        .ok_or(Rejected::App)?;
    if auth.counter != 0 {
        return Err(Rejected::Counter);
    }
    let environment = match auth.aaguid {
        Some(a) if a == AAGUID_PRODUCTION => Environment::Production,
        Some(a) if a == AAGUID_DEVELOPMENT && policy.allow_development => Environment::Development,
        Some(a) if a == AAGUID_DEVELOPMENT => return Err(Rejected::Environment),
        _ => return Err(Rejected::Malformed("App Attest aaguid")),
    };
    if auth.credential_id != Some(key_id.as_slice()) {
        return Err(Rejected::WrongKey);
    }
    // The COSE key in authData must be the certificate's key.
    let cose = auth.credential_key.as_ref().ok_or_else(bad)?;
    let x = cose.get(&Cbor::Negative(1)).and_then(Cbor::bytes);
    let y = cose.get(&Cbor::Negative(2)).and_then(Cbor::bytes);
    if cose.get(&Cbor::Uint(1)) != Some(&Cbor::Uint(2))
        || cose.get(&Cbor::Negative(0)) != Some(&Cbor::Uint(1))
        || x != Some(&leaf.key[1..33])
        || y != Some(&leaf.key[33..65])
    {
        return Err(Rejected::WrongKey);
    }
    let facts = app_facts(auth.extensions.as_ref())?.ok_or(Rejected::App)?;
    if !policy
        .validation_categories
        .contains(&facts.validation_category)
    {
        return Err(Rejected::App);
    }
    Ok(Verified {
        key: leaf.key.to_vec(),
        key_id: *key_id,
        rp_id_hash: app.rp_id_hash(),
        environment,
        facts,
        receipt: receipt.to_vec(),
    })
}

/// Verify an assertion over `client_data` with a stored attested key, and
/// return its counter, which must exceed `last_counter`.
pub fn verify_assertion(
    assertion: &[u8],
    client_data: &[u8],
    key: &[u8],
    rp_id_hash: &[u8; 32],
    last_counter: u32,
    validation_categories: &[u32],
) -> Result<u32, Rejected> {
    if assertion.len() > MAX_OBJECT_BYTES {
        return Err(Rejected::Malformed("assertion too large"));
    }
    let (top, rest) = cbor(assertion)?;
    if !rest.is_empty() {
        return Err(Rejected::Malformed("trailing assertion bytes"));
    }
    let bad = || Rejected::Malformed("assertion object");
    let signature = top
        .text_key("signature")
        .and_then(Cbor::bytes)
        .ok_or_else(bad)?;
    let auth_bytes = top
        .text_key("authenticatorData")
        .and_then(Cbor::bytes)
        .ok_or_else(bad)?;
    let client_data_hash = Sha256::digest(client_data);
    let nonce = Sha256::new()
        .chain_update(auth_bytes)
        .chain_update(client_data_hash)
        .finalize();
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, key)
        .verify(&nonce, signature)
        .map_err(|_| Rejected::Signature)?;
    let auth = auth_data(auth_bytes)?;
    if auth.aaguid.is_some() {
        return Err(Rejected::Malformed("assertion carries attested data"));
    }
    if auth.rp_id_hash != rp_id_hash {
        return Err(Rejected::App);
    }
    if auth.counter <= last_counter {
        return Err(Rejected::Counter);
    }
    // Apple documents these extensions for assertions too; when present
    // they must still name an accepted category.
    if let Some(facts) = app_facts(auth.extensions.as_ref())?
        && !validation_categories.contains(&facts.validation_category)
    {
        return Err(Rejected::App);
    }
    Ok(auth.counter)
}
