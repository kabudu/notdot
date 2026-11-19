//! Synthetic attestation for tests: a private test CA that mints Android
//! key attestation chains and App Attest objects in the platforms' formats.
//!
//! Nothing here is genuine evidence. A policy only accepts it after
//! [`AttestationPolicy::with_test_roots`](super::AttestationPolicy::with_test_roots),
//! and every enrolment it accepts is marked `test_only`.
use super::{AttestationPolicy, Evidence, android, apple};
use crate::DeviceId;
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use sha2::{Digest, Sha256};

/// A fresh random P-256 key.
pub fn random_key() -> SigningKey {
    loop {
        if let Ok(key) = SigningKey::from_slice(&crate::random_bytes::<32>()) {
            return key;
        }
    }
}

/// The uncompressed SEC1 point of a key.
pub fn point(key: &SigningKey) -> Vec<u8> {
    key.verifying_key()
        .to_encoded_point(false)
        .as_bytes()
        .to_vec()
}

fn len(out: &mut Vec<u8>, n: usize) {
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes = (n as u32).to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (4 - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
}

/// A DER element with a single-octet identifier.
pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    len(&mut out, content.len());
    out.extend_from_slice(content);
    out
}

/// `[n] EXPLICIT` with the high-tag-number form when needed.
pub fn context(number: u32, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if number < 31 {
        out.push(0xa0 | number as u8);
    } else {
        out.push(0xbf);
        let mut groups = Vec::new();
        let mut n = number;
        loop {
            groups.push((n & 0x7f) as u8);
            n >>= 7;
            if n == 0 {
                break;
            }
        }
        for (i, g) in groups.iter().rev().enumerate() {
            out.push(if i + 1 < groups.len() { g | 0x80 } else { *g });
        }
    }
    len(&mut out, content.len());
    out.extend_from_slice(content);
    out
}

pub fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

pub fn int(n: u64) -> Vec<u8> {
    let bytes = n.to_be_bytes();
    let skip = bytes.iter().take_while(|&&b| b == 0).count().min(7);
    let mut digits = bytes[skip..].to_vec();
    if digits[0] & 0x80 != 0 {
        digits.insert(0, 0);
    }
    tlv(0x02, &digits)
}

fn oid(encoded: &[u8]) -> Vec<u8> {
    tlv(0x06, encoded)
}

fn name(cn: &str) -> Vec<u8> {
    // Name ::= SEQUENCE OF SET OF { commonName UTF8String }
    seq(&[tlv(
        0x31,
        &seq(&[oid(&[0x55, 0x04, 0x03]), tlv(0x0c, cn.as_bytes())]),
    )])
}

fn generalized(ms: i64) -> Vec<u8> {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil from days (Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let text = format!(
        "{year:04}{month:02}{day:02}{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
    tlv(0x18, text.as_bytes())
}

/// An extension for [`TestCa::issue`].
pub struct Ext {
    pub oid: Vec<u8>,
    pub critical: bool,
    pub value: Vec<u8>,
}

fn basic_constraints(ca: bool) -> Ext {
    Ext {
        oid: super::der::oids::BASIC_CONSTRAINTS.to_vec(),
        critical: true,
        value: if ca {
            seq(&[tlv(0x01, &[0xff])])
        } else {
            seq(&[])
        },
    }
}

/// Validity window used for test certificates.
#[derive(Clone, Copy)]
pub struct Validity {
    pub not_before_ms: i64,
    pub not_after_ms: i64,
}
impl Validity {
    pub fn around(now_ms: u64) -> Self {
        Self {
            not_before_ms: now_ms as i64 - 86_400_000,
            not_after_ms: now_ms as i64 + 365 * 86_400_000,
        }
    }
}

/// A test certificate authority with a P-256 key.
#[derive(Clone)]
pub struct TestCa {
    pub key: SigningKey,
    pub cn: String,
    pub der: Vec<u8>,
}
impl TestCa {
    pub fn root(cn: &str, validity: Validity) -> Self {
        let key = random_key();
        let der = issue(
            &key,
            cn,
            cn,
            &point(&key),
            vec![basic_constraints(true)],
            validity,
            1,
        );
        Self {
            key,
            cn: cn.into(),
            der,
        }
    }
    pub fn intermediate(&self, cn: &str, validity: Validity) -> Self {
        let key = random_key();
        let der = self.issue(cn, &point(&key), vec![basic_constraints(true)], validity, 2);
        Self {
            key,
            cn: cn.into(),
            der,
        }
    }
    pub fn issue(
        &self,
        cn: &str,
        subject_point: &[u8],
        extensions: Vec<Ext>,
        validity: Validity,
        serial: u64,
    ) -> Vec<u8> {
        issue(
            &self.key,
            &self.cn,
            cn,
            subject_point,
            extensions,
            validity,
            serial,
        )
    }
}

fn issue(
    issuer_key: &SigningKey,
    issuer_cn: &str,
    cn: &str,
    subject_point: &[u8],
    extensions: Vec<Ext>,
    validity: Validity,
    serial: u64,
) -> Vec<u8> {
    let algorithm = seq(&[oid(super::der::oids::ECDSA_SHA256)]);
    let spki = seq(&[
        seq(&[
            oid(super::der::oids::EC_PUBLIC_KEY),
            oid(super::der::oids::P256),
        ]),
        tlv(0x03, &[&[0u8][..], subject_point].concat()),
    ]);
    let exts: Vec<Vec<u8>> = extensions
        .iter()
        .map(|e| {
            let mut parts = vec![oid(&e.oid)];
            if e.critical {
                parts.push(tlv(0x01, &[0xff]));
            }
            parts.push(tlv(0x04, &e.value));
            seq(&parts)
        })
        .collect();
    let tbs = seq(&[
        context(0, &int(2)),
        int(serial),
        algorithm.clone(),
        name(issuer_cn),
        seq(&[
            generalized(validity.not_before_ms),
            generalized(validity.not_after_ms),
        ]),
        name(cn),
        spki,
        context(3, &seq(&exts)),
    ]);
    let signature: Signature = issuer_key.sign(&tbs);
    seq(&[
        tbs,
        algorithm,
        tlv(0x03, &[&[0u8][..], signature.to_der().as_bytes()].concat()),
    ])
}

/// The fields of an Android key description, defaulting to what the app's
/// step-up key looks like on a good device.
#[derive(Clone)]
pub struct KeySpec {
    pub attestation_version: u64,
    pub attestation_level: u64,
    pub keymint_level: u64,
    pub challenge: Vec<u8>,
    pub purposes: Vec<u64>,
    pub algorithm: u64,
    pub curve: u64,
    pub origin: u64,
    pub no_auth_required: bool,
    pub user_auth_type: Option<u64>,
    pub auth_timeout: Option<u64>,
    pub locked: bool,
    pub boot_state: u64,
    pub packages: Vec<String>,
    pub signers: Vec<[u8; 32]>,
    /// Put the application id in the hardware list too.
    pub duplicate_app_id: bool,
    /// Emit hardware tags in descending order.
    pub misorder: bool,
}
impl KeySpec {
    pub fn good(challenge: [u8; 32], app: &android::AndroidApp) -> Self {
        Self {
            attestation_version: 300,
            attestation_level: 1,
            keymint_level: 1,
            challenge: challenge.to_vec(),
            purposes: vec![2],
            algorithm: 3,
            curve: 1,
            origin: 0,
            no_auth_required: false,
            user_auth_type: Some(2),
            auth_timeout: None,
            locked: true,
            boot_state: 0,
            packages: vec![app.package.clone()],
            signers: app.signers.clone(),
            duplicate_app_id: false,
            misorder: false,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let app_id = tlv(
            0x04,
            &seq(&[
                tlv(
                    0x31,
                    &self
                        .packages
                        .iter()
                        .map(|p| seq(&[tlv(0x04, p.as_bytes()), int(1)]))
                        .collect::<Vec<_>>()
                        .concat(),
                ),
                tlv(
                    0x31,
                    &self
                        .signers
                        .iter()
                        .map(|s| tlv(0x04, s))
                        .collect::<Vec<_>>()
                        .concat(),
                ),
            ]),
        );
        let software = seq(&[context(709, &app_id)]);
        let mut hw: Vec<Vec<u8>> = vec![
            context(
                1,
                &tlv(
                    0x31,
                    &self
                        .purposes
                        .iter()
                        .map(|&p| int(p))
                        .collect::<Vec<_>>()
                        .concat(),
                ),
            ),
            context(2, &int(self.algorithm)),
            context(3, &int(256)),
            context(10, &int(self.curve)),
        ];
        if self.no_auth_required {
            hw.push(context(503, &tlv(0x05, &[])));
        }
        if let Some(t) = self.user_auth_type {
            hw.push(context(504, &int(t)));
        }
        if let Some(t) = self.auth_timeout {
            hw.push(context(505, &int(t)));
        }
        hw.push(context(702, &int(self.origin)));
        hw.push(context(
            704,
            &seq(&[
                tlv(0x04, &[0; 32]),
                tlv(0x01, &[if self.locked { 0xff } else { 0 }]),
                tlv(0x0a, &[self.boot_state as u8]),
                tlv(0x04, &[1; 32]),
            ]),
        ));
        if self.duplicate_app_id {
            hw.push(context(709, &app_id));
        }
        if self.misorder {
            hw.reverse();
        }
        seq(&[
            int(self.attestation_version),
            tlv(0x0a, &[self.attestation_level as u8]),
            int(self.attestation_version),
            tlv(0x0a, &[self.keymint_level as u8]),
            tlv(0x04, &self.challenge),
            tlv(0x04, &[]),
            software,
            seq(&hw),
        ])
    }
}

/// Test roots plus what a policy needs to accept their evidence.
#[derive(Clone)]
pub struct TestRoots {
    pub android_root: TestCa,
    pub android_intermediate: TestCa,
    pub apple_root: TestCa,
    pub apple_intermediate: TestCa,
    pub android_app: android::AndroidApp,
    pub apple_app: apple::AppleApp,
    pub validity: Validity,
}
impl TestRoots {
    pub fn new(now_ms: u64) -> Self {
        let validity = Validity::around(now_ms);
        let android_root = TestCa::root("Test Android Root", validity);
        let android_intermediate = android_root.intermediate("Test Android Batch", validity);
        let apple_root = TestCa::root("Test App Attest Root", validity);
        let apple_intermediate = apple_root.intermediate("Test App Attest CA", validity);
        Self {
            android_root,
            android_intermediate,
            apple_root,
            apple_intermediate,
            android_app: android::AndroidApp {
                package: "dev.example.test".into(),
                signers: vec![[0x5a; 32]],
            },
            apple_app: apple::AppleApp {
                team_id: "TESTTEAM01".into(),
                bundle_id: "dev.example.test".into(),
            },
            validity,
        }
    }

    /// A policy trusting these roots, with an empty fresh status list.
    pub fn policy(&self, now_ms: u64) -> AttestationPolicy {
        let mut policy = AttestationPolicy::default()
            .with_test_roots(&self.android_root.der, &self.apple_root.der);
        policy.android_apps = vec![self.android_app.clone()];
        policy.apple = apple::ApplePolicy {
            apps: vec![self.apple_app.clone()],
            allow_development: false,
            validation_categories: vec![1],
        };
        policy.set_status_list(
            android::StatusList::parse(br#"{"entries":{}}"#, now_ms).expect("empty list"),
        );
        policy
    }

    /// An Android chain for `key` described by `spec`, leaf first.
    pub fn android_chain(&self, key: &SigningKey, spec: &KeySpec) -> Vec<Vec<u8>> {
        let leaf = self.android_intermediate.issue(
            "Android Keystore Key",
            &point(key),
            vec![Ext {
                oid: super::der::oids::ANDROID_KEY_ATTESTATION.to_vec(),
                critical: false,
                value: spec.encode(),
            }],
            self.validity,
            1,
        );
        vec![
            leaf,
            self.android_intermediate.der.clone(),
            self.android_root.der.clone(),
        ]
    }

    /// A phone's good Android evidence for this challenge, and its key.
    pub fn android_evidence(&self, challenge: [u8; 32]) -> (SigningKey, Evidence) {
        let key = random_key();
        let chain = self.android_chain(&key, &KeySpec::good(challenge, &self.android_app));
        (key, Evidence::AndroidKeystore { chain })
    }

    pub fn app_attest_device(&self) -> AppAttestDevice {
        AppAttestDevice::new(self, &self.apple_app, b"appattest\0\0\0\0\0\0\0")
    }
}

/// A simulated App Attest key on a test "device".
pub struct AppAttestDevice {
    pub key: SigningKey,
    pub key_id: [u8; 32],
    pub counter: u32,
    pub rp_id_hash: [u8; 32],
    pub aaguid: [u8; 16],
    pub category: u32,
    intermediate: TestCa,
    validity: Validity,
}

fn cbor_head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | n as u8),
        24..=0xff => out.extend_from_slice(&[m | 24, n as u8]),
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        _ => {
            out.push(m | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
    }
}
pub fn cbor_bytes(out: &mut Vec<u8>, b: &[u8]) {
    cbor_head(out, 2, b.len() as u64);
    out.extend_from_slice(b);
}
pub fn cbor_text(out: &mut Vec<u8>, t: &str) {
    cbor_head(out, 3, t.len() as u64);
    out.extend_from_slice(t.as_bytes());
}
pub fn cbor_map(out: &mut Vec<u8>, n: u64) {
    cbor_head(out, 5, n);
}

impl AppAttestDevice {
    pub fn new(roots: &TestRoots, app: &apple::AppleApp, aaguid: &[u8; 16]) -> Self {
        let key = random_key();
        let key_id = Sha256::digest(point(&key)).into();
        Self {
            key,
            key_id,
            counter: 0,
            rp_id_hash: app.rp_id_hash(),
            aaguid: *aaguid,
            category: 1,
            intermediate: roots.apple_intermediate.clone(),
            validity: roots.validity,
        }
    }

    fn extensions(&self, out: &mut Vec<u8>) {
        cbor_map(out, 2);
        cbor_text(out, "apple_bundle_version_01");
        cbor_text(out, "1");
        cbor_text(out, "apple_validation_category_01");
        cbor_bytes(out, &self.category.to_le_bytes());
    }

    /// `attestKey(keyId, clientDataHash)`.
    pub fn attest(&self, client_data_hash: &[u8; 32]) -> Vec<u8> {
        let p = point(&self.key);
        let mut auth = self.rp_id_hash.to_vec();
        // As Apple's sample: extensions follow without the ED flag.
        auth.push(0x40);
        auth.extend_from_slice(&0u32.to_be_bytes());
        auth.extend_from_slice(&self.aaguid);
        auth.extend_from_slice(&32u16.to_be_bytes());
        auth.extend_from_slice(&self.key_id);
        // COSE EC2 key: {1: 2, 3: -7, -1: 1, -2: x, -3: y}
        cbor_map(&mut auth, 5);
        auth.extend_from_slice(&[0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21]);
        cbor_bytes(&mut auth, &p[1..33]);
        auth.push(0x22);
        cbor_bytes(&mut auth, &p[33..65]);
        self.extensions(&mut auth);
        let nonce: [u8; 32] = Sha256::new()
            .chain_update(&auth)
            .chain_update(client_data_hash)
            .finalize()
            .into();
        let nonce_ext = seq(&[context(1, &tlv(0x04, &nonce))]);
        let leaf = self.intermediate.issue(
            "test credential",
            &p,
            vec![Ext {
                oid: super::der::oids::APPLE_NONCE.to_vec(),
                critical: false,
                value: nonce_ext,
            }],
            self.validity,
            7,
        );
        let mut out = Vec::new();
        cbor_map(&mut out, 3);
        cbor_text(&mut out, "fmt");
        cbor_text(&mut out, "apple-appattest");
        cbor_text(&mut out, "attStmt");
        cbor_map(&mut out, 2);
        cbor_text(&mut out, "x5c");
        cbor_head(&mut out, 4, 2);
        cbor_bytes(&mut out, &leaf);
        cbor_bytes(&mut out, &self.intermediate.der);
        cbor_text(&mut out, "receipt");
        cbor_bytes(&mut out, b"test receipt");
        cbor_text(&mut out, "authData");
        cbor_bytes(&mut out, &auth);
        out
    }

    /// `generateAssertion(keyId, SHA256(clientData))`.
    pub fn assert(&mut self, client_data: &[u8]) -> Vec<u8> {
        self.counter += 1;
        let mut auth = self.rp_id_hash.to_vec();
        auth.push(0x00);
        auth.extend_from_slice(&self.counter.to_be_bytes());
        self.extensions(&mut auth);
        let nonce = Sha256::new()
            .chain_update(&auth)
            .chain_update(Sha256::digest(client_data))
            .finalize();
        let signature: Signature = self.key.sign(&nonce);
        let mut out = Vec::new();
        cbor_map(&mut out, 2);
        cbor_text(&mut out, "signature");
        cbor_bytes(&mut out, signature.to_der().as_bytes());
        cbor_text(&mut out, "authenticatorData");
        cbor_bytes(&mut out, &auth);
        out
    }

    /// Enrolment evidence vouching for `step_up_key`.
    pub fn evidence(
        &self,
        desktop: DeviceId,
        phone: DeviceId,
        nonce: &[u8; 32],
        step_up_key: &[u8],
    ) -> Evidence {
        let hash = super::app_attest_client_data_hash(desktop, phone, nonce, step_up_key);
        Evidence::AppleAppAttest {
            step_up_key: step_up_key.to_vec(),
            key_id: self.key_id,
            attestation: self.attest(&hash),
        }
    }
}
