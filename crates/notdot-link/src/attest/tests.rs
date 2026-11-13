#![allow(clippy::field_reassign_with_default)]
use super::testing::{AppAttestDevice, KeySpec, TestCa, TestRoots, Validity, point, random_key};
use super::*;

const DESKTOP: DeviceId = DeviceId([1; 32]);
const PHONE: DeviceId = DeviceId([2; 32]);
const NOW: u64 = 1_790_000_000_000;

fn ms(y: i64, m: i64, d: i64) -> u64 {
    // Midday UTC on a civil date.
    let days = {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
        era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
    };
    (days * 86_400 + 43_200) as u64 * 1000
}

fn pem_chain(text: &str) -> Vec<Vec<u8>> {
    text.split("-----END CERTIFICATE-----")
        .filter(|block| block.contains("BEGIN CERTIFICATE"))
        .map(|block| der::pem(block).unwrap())
        .collect()
}

fn fresh_status(now: u64) -> android::StatusList {
    android::StatusList::parse(br#"{"entries":{}}"#, now).unwrap()
}

// ---- Genuine platform vectors ----

const APPLE_SAMPLE: &str = include_str!("fixtures/apple_app_attest_sample.b64");

fn apple_sample() -> (Vec<u8>, [u8; 32], Vec<u8>, apple::ApplePolicy) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let object = STANDARD.decode(APPLE_SAMPLE.trim()).unwrap();
    let key_id: [u8; 32] = STANDARD
        .decode("zgSY9YSD+7TaDXssY6WlOPVS1K3Lmk+pFhlcSWE+ZV0=")
        .unwrap()
        .try_into()
        .unwrap();
    // Apple's sample app passed the challenge itself as `clientDataHash`.
    let hash = b"example_server_challenge".to_vec();
    let policy = apple::ApplePolicy {
        apps: vec![apple::AppleApp {
            team_id: "1234567890".into(),
            bundle_id: "com.example.myapp".into(),
        }],
        allow_development: false,
        validation_categories: vec![1],
    };
    (object, key_id, hash, policy)
}

#[test]
fn apples_published_sample_attestation_verifies_to_the_real_root() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let (object, key_id, hash, policy) = apple_sample();
    let root = apple::apple_root();
    let anchors = vec![der::spki_of(&root).unwrap()];
    let during = ms(2026, 4, 21);
    let verified =
        apple::verify_attestation(&object, &key_id, &hash, &anchors, &root, &policy, during)
            .unwrap();
    assert_eq!(verified.environment, apple::Environment::Production);
    assert_eq!(verified.facts.validation_category, 1);
    assert_eq!(verified.facts.bundle_version, "1");
    // Apple's expected values from the guide.
    assert_eq!(
        STANDARD.encode(verified.rp_id_hash),
        "9EZtaPketsEGIMt+Y8coMkRoXuHWRntUFg51MXIFfwM="
    );
    assert_eq!(verified.key_id, key_id);
    assert!(!verified.receipt.is_empty());

    let check =
        |object: &[u8], key_id: &[u8; 32], hash: &[u8], policy: &apple::ApplePolicy, now| {
            apple::verify_attestation(object, key_id, hash, &anchors, &root, policy, now)
        };
    // Today the leaf has expired: refused, never trusted past its dates.
    assert_eq!(
        check(&object, &key_id, &hash, &policy, NOW),
        Err(Rejected::Expired)
    );
    // Another challenge.
    let other = b"another_server_challenge".to_vec();
    assert_eq!(
        check(&object, &key_id, &other, &policy, during),
        Err(Rejected::Challenge)
    );
    // Another key id.
    assert_eq!(
        check(&object, &[0; 32], &hash, &policy, during),
        Err(Rejected::WrongKey)
    );
    // Another App ID or category.
    let mut wrong_app = policy.clone();
    wrong_app.apps[0].bundle_id = "com.example.other".into();
    assert_eq!(
        check(&object, &key_id, &hash, &wrong_app, during),
        Err(Rejected::App)
    );
    let mut wrong_category = policy.clone();
    wrong_category.validation_categories = vec![7];
    assert_eq!(
        check(&object, &key_id, &hash, &wrong_category, during),
        Err(Rejected::App)
    );
    // Unconfigured policies accept nothing.
    let mut empty = policy.clone();
    empty.apps.clear();
    assert_eq!(
        check(&object, &key_id, &hash, &empty, during),
        Err(Rejected::NotConfigured)
    );
    // Any changed byte is refused (and never panics), except inside the
    // receipt: App Attest validation does not cover it; only Apple's
    // fraud-metric service checks it, server to server.
    let receipt_at = object
        .windows(verified.receipt.len())
        .position(|w| w == verified.receipt.as_slice())
        .unwrap();
    let receipt = receipt_at..receipt_at + verified.receipt.len();
    for i in (0..object.len())
        .step_by(7)
        .filter(|i| !receipt.contains(i))
    {
        let mut tampered = object.clone();
        tampered[i] ^= 0x01;
        assert!(
            check(&tampered, &key_id, &hash, &policy, during).is_err(),
            "byte {i}"
        );
    }
    // Truncations are refused.
    for cut in (0..object.len()).step_by(97) {
        assert!(check(&object[..cut], &key_id, &hash, &policy, during).is_err());
    }
    // A test root never stands in for Apple's.
    let test = TestCa::root("Test App Attest Root", Validity::around(during));
    let test_anchor = vec![der::spki_of(&test.der).unwrap()];
    assert!(matches!(
        apple::verify_attestation(
            &object,
            &key_id,
            &hash,
            &test_anchor,
            &test.der,
            &policy,
            during
        ),
        Err(Rejected::Chain(_) | Rejected::UntrustedRoot)
    ));
    // Nor does Apple's root certificate without its pinned key.
    assert_eq!(
        apple::verify_attestation(
            &object,
            &key_id,
            &hash,
            &test_anchor,
            &root,
            &policy,
            during
        ),
        Err(Rejected::UntrustedRoot)
    );
}

fn google_policy_apps() -> Vec<android::AndroidApp> {
    vec![android::AndroidApp {
        package: "com.google.android.attestation".into(),
        signers: vec![[0; 32]],
    }]
}

#[test]
fn real_pixel_chains_validate_to_googles_roots_and_their_key_descriptions_parse() {
    let anchors = android::google_anchors();
    let cases = [
        (
            include_str!("fixtures/android_caiman_sdk36_TEE_EC_RKP.pem"),
            ms(2025, 9, 30),
            1,
        ),
        (
            include_str!("fixtures/android_caiman_sdk36_SB_EC_RKP.pem"),
            ms(2025, 9, 30),
            2,
        ),
        (
            include_str!("fixtures/android_tegu_sdk36_TEE_EC_2026_ROOT.pem"),
            ms(2026, 3, 1),
            1,
        ),
    ];
    for (pem, during, level) in cases {
        let chain = pem_chain(pem);
        let certs = der::verify_chain(&chain, &anchors, during as i64).unwrap();
        let ext = certs[0]
            .extension(der::oids::ANDROID_KEY_ATTESTATION)
            .unwrap();
        let description = android::key_description(ext.value).unwrap();
        assert_eq!(description.attestation_level, level);
        assert_eq!(description.keymint_level, level);
        assert!(description.attestation_version >= 300);
        let (packages, digests) = android::application_id(
            der::single(description.software[&709], der::OCTET_STRING).unwrap(),
        )
        .unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(digests.len(), 1);
        // The full verifier gets as far as the challenge, which was someone
        // else's: refused, as it must be.
        let leaf_key = certs[0].key.to_vec();
        let status = fresh_status(during);
        assert_eq!(
            android::verify(
                &chain,
                &anchors,
                &google_policy_apps(),
                Some(&status),
                &[0; 32],
                &leaf_key,
                during
            ),
            Err(Rejected::Challenge)
        );
        // The short-lived RKP certificates have expired by now.
        assert_eq!(
            android::verify(
                &chain,
                &anchors,
                &google_policy_apps(),
                Some(&fresh_status(NOW)),
                &[0; 32],
                &leaf_key,
                NOW
            ),
            Err(Rejected::Expired)
        );
        // Revoking any certificate in the chain refuses it.
        for cert in &certs {
            let hex: String = cert.serial.iter().map(|b| format!("{b:02x}")).collect();
            let hex = hex.trim_start_matches('0');
            let json = format!(
                r#"{{"entries":{{"{hex}":{{"status":"REVOKED","reason":"KEY_COMPROMISE"}}}}}}"#
            );
            let revoked = android::StatusList::parse(json.as_bytes(), during).unwrap();
            assert_eq!(
                android::verify(
                    &chain,
                    &anchors,
                    &google_policy_apps(),
                    Some(&revoked),
                    &[0; 32],
                    &leaf_key,
                    during
                ),
                Err(Rejected::Revoked)
            );
        }
        // Missing or stale revocation data refuses it.
        for status in [
            None,
            Some(fresh_status(during - android::MAX_STATUS_AGE_MS - 1)),
        ] {
            assert_eq!(
                android::verify(
                    &chain,
                    &anchors,
                    &google_policy_apps(),
                    status.as_ref(),
                    &[0; 32],
                    &leaf_key,
                    during
                ),
                Err(Rejected::RevocationUnavailable)
            );
        }
        // Against a test root, Google's chain is untrusted.
        let test = TestCa::root("Test Android Root", Validity::around(during));
        assert_eq!(
            der::verify_chain(&chain, &[der::spki_of(&test.der).unwrap()], during as i64).err(),
            Some(Rejected::UntrustedRoot)
        );
        // Dropping the root, or swapping in a re-signed leaf, fails.
        assert!(der::verify_chain(&chain[..chain.len() - 1], &anchors, during as i64).is_err());
    }
}

#[test]
fn googles_invalid_samples_are_refused() {
    let anchors = android::google_anchors();
    for pem in [
        include_str!("fixtures/android_invalid_tags_not_in_ascending_order.pem"),
        include_str!("fixtures/android_invalid_malformed_rot_device_locked.pem"),
    ] {
        let chain = pem_chain(pem);
        // The full verifier refuses them.
        let leaf = der::Certificate::parse(&chain[0]).unwrap();
        let status = fresh_status(NOW);
        assert!(
            android::verify(
                &chain,
                &anchors,
                &google_policy_apps(),
                Some(&status),
                &[0; 32],
                leaf.key,
                NOW
            )
            .is_err()
        );
        // So does the key-description parser on its own, whatever the
        // signatures say.
        let ext = leaf.extension(der::oids::ANDROID_KEY_ATTESTATION).unwrap();
        let refused = android::key_description(ext.value)
            .and_then(|d| android::device_integrity(&d.hardware));
        assert!(
            matches!(refused, Err(Rejected::Malformed(_))),
            "{refused:?}"
        );
    }
}

#[test]
fn the_status_list_format_is_strict() {
    for bad in [
        &br#"{}"#[..],
        br#"{"entries":{"0abc":{"status":"REVOKED"}}}"#,
        br#"{"entries":{"ABC":{"status":"REVOKED"}}}"#,
        br#"{"entries":{"abc":{"status":"FINE"}}}"#,
        br#"not json"#,
    ] {
        assert!(android::StatusList::parse(bad, 0).is_err());
    }
    let list = android::StatusList::parse(
        br#"{"entries":{"1a2b":{"status":"SUSPENDED","expires":"2030-01-01"}}}"#,
        0,
    )
    .unwrap();
    assert!(list.lists(&[0x00, 0x1a, 0x2b]));
    assert!(!list.lists(&[0x1a, 0x2c]));
}

// ---- Synthetic adversarial cases ----

fn android_enrol(
    roots: &TestRoots,
    policy: &AttestationPolicy,
    spec: KeySpec,
) -> Result<Enrollment, Rejected> {
    let key = random_key();
    let chain = roots.android_chain(&key, &spec);
    policy.verify(
        &Evidence::AndroidKeystore { chain },
        DESKTOP,
        PHONE,
        &[3; 32],
        NOW,
    )
}

#[test]
fn android_enrolment_needs_every_property_of_the_apps_step_up_key() {
    let roots = TestRoots::new(NOW);
    let policy = roots.policy(NOW);
    let challenge = attestation_challenge(DESKTOP, PHONE, &[3; 32]);
    let good = KeySpec::good(challenge, &roots.android_app);
    let enrolled = android_enrol(&roots, &policy, good.clone()).unwrap();
    assert_eq!(enrolled.protection, Protection::AndroidTrustedEnvironment);
    assert!(
        enrolled.test_only,
        "test roots never yield genuine evidence"
    );
    let strongbox = KeySpec {
        attestation_level: 2,
        keymint_level: 2,
        ..good.clone()
    };
    assert_eq!(
        android_enrol(&roots, &policy, strongbox)
            .unwrap()
            .protection,
        Protection::AndroidStrongBox
    );

    let refused = |change: &dyn Fn(&mut KeySpec)| {
        let mut spec = good.clone();
        change(&mut spec);
        android_enrol(&roots, &policy, spec).unwrap_err()
    };
    use Rejected::*;
    assert_eq!(refused(&|s| s.attestation_level = 0), NotHardware);
    assert_eq!(refused(&|s| s.keymint_level = 0), NotHardware);
    assert_eq!(refused(&|s| s.keymint_level = 2), NotHardware);
    assert_eq!(refused(&|s| s.challenge = vec![0; 32]), Challenge);
    assert_eq!(
        refused(&|s| s.challenge = challenge[..16].to_vec()),
        Challenge
    );
    // Bound to this desktop, phone and nonce.
    for (desktop, phone, nonce) in [
        (DeviceId([9; 32]), PHONE, [3; 32]),
        (DESKTOP, DeviceId([9; 32]), [3; 32]),
        (DESKTOP, PHONE, [4; 32]),
    ] {
        let mut spec = good.clone();
        spec.challenge = attestation_challenge(desktop, phone, &nonce).to_vec();
        assert_eq!(android_enrol(&roots, &policy, spec).unwrap_err(), Challenge);
    }
    assert_eq!(refused(&|s| s.purposes = vec![3]), KeyProperties("purpose"));
    assert_eq!(
        refused(&|s| s.purposes = vec![2, 7]),
        KeyProperties("purpose")
    );
    assert_eq!(refused(&|s| s.algorithm = 1), KeyProperties("algorithm"));
    assert_eq!(refused(&|s| s.curve = 2), KeyProperties("algorithm"));
    assert_eq!(refused(&|s| s.origin = 2), KeyProperties("origin"));
    assert_eq!(
        refused(&|s| s.no_auth_required = true),
        KeyProperties("user authentication")
    );
    assert_eq!(
        refused(&|s| s.user_auth_type = None),
        KeyProperties("user authentication")
    );
    assert_eq!(
        refused(&|s| s.user_auth_type = Some(1)),
        KeyProperties("user authentication")
    );
    assert_eq!(
        refused(&|s| s.user_auth_type = Some(3)),
        KeyProperties("user authentication")
    );
    assert_eq!(
        refused(&|s| s.auth_timeout = Some(30)),
        KeyProperties("user authentication")
    );
    assert!(
        android_enrol(
            &roots,
            &policy,
            KeySpec {
                auth_timeout: Some(0),
                ..good.clone()
            }
        )
        .is_ok()
    );
    assert_eq!(refused(&|s| s.locked = false), DeviceIntegrity);
    assert_eq!(refused(&|s| s.boot_state = 2), DeviceIntegrity);
    assert_eq!(
        refused(&|s| s.packages = vec!["dev.example.other".into()]),
        App
    );
    assert_eq!(
        refused(&|s| s.packages.push("dev.example.shared".into())),
        App
    );
    assert_eq!(refused(&|s| s.signers = vec![[0x11; 32]]), App);
    assert_eq!(refused(&|s| s.signers.push([0x11; 32])), App);
    assert_eq!(refused(&|s| s.signers.clear()), App);
    assert_eq!(refused(&|s| s.duplicate_app_id = true), App);
    assert!(matches!(refused(&|s| s.misorder = true), Malformed(_)));
    assert_eq!(
        refused(&|s| s.attestation_version = 2),
        Unsupported("attestation version")
    );

    // A policy with no apps, or without revocation data, refuses all.
    let mut unconfigured = policy.clone();
    unconfigured.android_apps.clear();
    assert_eq!(
        android_enrol(&roots, &unconfigured, good.clone()).unwrap_err(),
        NotConfigured
    );
    let mut no_status = AttestationPolicy::default()
        .with_test_roots(&roots.android_root.der, &roots.apple_root.der);
    no_status.android_apps = vec![roots.android_app.clone()];
    assert_eq!(
        android_enrol(&roots, &no_status, good.clone()).unwrap_err(),
        RevocationUnavailable
    );
    // The default policy trusts only Google's roots and no apps.
    assert_eq!(
        android_enrol(&roots, &AttestationPolicy::default(), good.clone()).unwrap_err(),
        NotConfigured
    );
    let mut google_only = AttestationPolicy::default();
    google_only.android_apps = vec![roots.android_app.clone()];
    google_only.set_status_list(fresh_status(NOW));
    assert_eq!(
        android_enrol(&roots, &google_only, good).unwrap_err(),
        UntrustedRoot
    );
}

#[test]
fn android_chains_must_be_whole_signed_current_and_carry_one_extension() {
    let roots = TestRoots::new(NOW);
    let policy = roots.policy(NOW);
    let challenge = attestation_challenge(DESKTOP, PHONE, &[3; 32]);
    let spec = KeySpec::good(challenge, &roots.android_app);
    let key = random_key();
    let chain = roots.android_chain(&key, &spec);
    let verify = |chain: Vec<Vec<u8>>, now| {
        policy.verify(
            &Evidence::AndroidKeystore { chain },
            DESKTOP,
            PHONE,
            &[3; 32],
            now,
        )
    };
    assert!(verify(chain.clone(), NOW).is_ok());
    // Outside the validity window (beyond the allowed skew).
    let year = 365 * 86_400_000;
    assert_eq!(
        verify(chain.clone(), NOW + 2 * year),
        Err(Rejected::Expired)
    );
    assert_eq!(
        verify(chain.clone(), NOW - 2 * 86_400_000),
        Err(Rejected::Expired)
    );
    // Missing intermediate, missing root, reordered, or empty.
    assert!(verify(vec![chain[0].clone(), chain[2].clone()], NOW).is_err());
    assert!(verify(chain[..2].to_vec(), NOW).is_err());
    assert!(
        verify(
            vec![chain[1].clone(), chain[0].clone(), chain[2].clone()],
            NOW
        )
        .is_err()
    );
    assert!(verify(Vec::new(), NOW).is_err());
    assert!(verify(vec![chain[0].clone(); 9], NOW).is_err());
    // A leaf signed by an impostor CA with the same name.
    let impostor = TestCa {
        key: random_key(),
        ..roots.android_intermediate.clone()
    };
    let forged = impostor.issue(
        "Android Keystore Key",
        &point(&key),
        vec![testing::Ext {
            oid: der::oids::ANDROID_KEY_ATTESTATION.to_vec(),
            critical: false,
            value: spec.encode(),
        }],
        roots.validity,
        1,
    );
    assert!(matches!(
        verify(vec![forged, chain[1].clone(), chain[2].clone()], NOW),
        Err(Rejected::Chain(_))
    ));
    // An intermediate that is not a CA cannot issue.
    let not_ca = roots.android_root.issue(
        "Test Android Batch",
        &point(&roots.android_intermediate.key),
        Vec::new(),
        roots.validity,
        3,
    );
    assert!(matches!(
        verify(vec![chain[0].clone(), not_ca, chain[2].clone()], NOW),
        Err(Rejected::Chain(_))
    ));
    // An unknown critical extension.
    let critical = roots.android_intermediate.issue(
        "Android Keystore Key",
        &point(&key),
        vec![
            testing::Ext {
                oid: der::oids::ANDROID_KEY_ATTESTATION.to_vec(),
                critical: false,
                value: spec.encode(),
            },
            testing::Ext {
                oid: vec![0x2a, 3, 4],
                critical: true,
                value: vec![0x05, 0x00],
            },
        ],
        roots.validity,
        1,
    );
    assert!(matches!(
        verify(vec![critical, chain[1].clone(), chain[2].clone()], NOW),
        Err(Rejected::Chain(_))
    ));
    // No attestation extension: a plain certificate proves nothing.
    let plain = roots.android_intermediate.issue(
        "Android Keystore Key",
        &point(&key),
        Vec::new(),
        roots.validity,
        1,
    );
    assert_eq!(
        verify(vec![plain, chain[1].clone(), chain[2].clone()], NOW),
        Err(Rejected::NotHardware)
    );
    // The extension in an intermediate is refused.
    let tainted = roots.android_root.issue(
        "Test Android Batch",
        &point(&roots.android_intermediate.key),
        vec![
            testing::Ext {
                oid: der::oids::BASIC_CONSTRAINTS.to_vec(),
                critical: true,
                value: testing::seq(&[testing::tlv(0x01, &[0xff])]),
            },
            testing::Ext {
                oid: der::oids::ANDROID_KEY_ATTESTATION.to_vec(),
                critical: false,
                value: spec.encode(),
            },
        ],
        roots.validity,
        2,
    );
    assert!(matches!(
        verify(vec![chain[0].clone(), tainted, chain[2].clone()], NOW),
        Err(Rejected::Chain(_))
    ));
    // Random bytes in place of each certificate never panic.
    for i in 0..chain.len() {
        for len in [0usize, 1, 2, 64, 300] {
            let mut broken = chain.clone();
            broken[i] = crate::random_bytes::<512>()[..len].to_vec();
            assert!(verify(broken, NOW).is_err());
        }
    }
    // Every single-byte change in the leaf is refused.
    for i in 0..chain[0].len() {
        let mut broken = chain.clone();
        broken[0][i] ^= 0x01;
        assert!(verify(broken, NOW).is_err(), "byte {i}");
    }
}

fn apple_setup() -> (TestRoots, AttestationPolicy, AppAttestDevice, Vec<u8>) {
    let roots = TestRoots::new(NOW);
    let policy = roots.policy(NOW);
    let device = roots.app_attest_device();
    let step_up = point(&random_key());
    (roots, policy, device, step_up)
}

#[test]
fn app_attest_enrolment_binds_the_step_up_key_and_the_challenge() {
    let (roots, policy, device, step_up) = apple_setup();
    let nonce = [3; 32];
    let evidence = device.evidence(DESKTOP, PHONE, &nonce, &step_up);
    let enrolled = policy
        .verify(&evidence, DESKTOP, PHONE, &nonce, NOW)
        .unwrap();
    assert_eq!(enrolled.protection, Protection::AppleAppAttested);
    assert_eq!(enrolled.step_up_key, step_up);
    assert!(enrolled.test_only);
    let attested = enrolled.app_attest.unwrap();
    assert_eq!(attested.key, point(&device.key));
    assert_eq!(attested.counter, 0);

    // Swapping in another step-up key breaks the binding.
    let Evidence::AppleAppAttest {
        key_id,
        attestation,
        ..
    } = evidence.clone()
    else {
        unreachable!()
    };
    let swapped = Evidence::AppleAppAttest {
        step_up_key: point(&random_key()),
        key_id,
        attestation: attestation.clone(),
    };
    assert_eq!(
        policy.verify(&swapped, DESKTOP, PHONE, &nonce, NOW),
        Err(Rejected::Challenge)
    );
    // Another desktop, phone or nonce.
    assert_eq!(
        policy.verify(&evidence, DeviceId([9; 32]), PHONE, &nonce, NOW),
        Err(Rejected::Challenge)
    );
    assert_eq!(
        policy.verify(&evidence, DESKTOP, DeviceId([9; 32]), &nonce, NOW),
        Err(Rejected::Challenge)
    );
    assert_eq!(
        policy.verify(&evidence, DESKTOP, PHONE, &[4; 32], NOW),
        Err(Rejected::Challenge)
    );
    // The App Attest key cannot double as the step-up key.
    let reuse = device.evidence(DESKTOP, PHONE, &nonce, &point(&device.key));
    assert_eq!(
        policy.verify(&reuse, DESKTOP, PHONE, &nonce, NOW),
        Err(Rejected::WrongKey)
    );
    // A malformed step-up key.
    let bad_key = device.evidence(DESKTOP, PHONE, &nonce, &[4; 65]);
    assert_eq!(
        policy.verify(&bad_key, DESKTOP, PHONE, &nonce, NOW),
        Err(Rejected::WrongKey)
    );
    // A key id that is not the attested key's.
    let wrong_id = Evidence::AppleAppAttest {
        step_up_key: step_up.clone(),
        key_id: [7; 32],
        attestation,
    };
    assert_eq!(
        policy.verify(&wrong_id, DESKTOP, PHONE, &nonce, NOW),
        Err(Rejected::WrongKey)
    );

    // Development keys only when allowed; other apps never.
    let dev = AppAttestDevice::new(&roots, &roots.apple_app, b"appattestdevelop");
    let dev_evidence = dev.evidence(DESKTOP, PHONE, &nonce, &step_up);
    assert_eq!(
        policy.verify(&dev_evidence, DESKTOP, PHONE, &nonce, NOW),
        Err(Rejected::Environment)
    );
    let mut allow_dev = policy.clone();
    allow_dev.apple.allow_development = true;
    assert!(
        allow_dev
            .verify(&dev_evidence, DESKTOP, PHONE, &nonce, NOW)
            .is_ok()
    );
    let unknown_aaguid = AppAttestDevice::new(&roots, &roots.apple_app, b"something-else!!");
    assert!(matches!(
        policy.verify(
            &unknown_aaguid.evidence(DESKTOP, PHONE, &nonce, &step_up),
            DESKTOP,
            PHONE,
            &nonce,
            NOW
        ),
        Err(Rejected::Malformed(_))
    ));
    let other_app = apple::AppleApp {
        team_id: "OTHERTEAM1".into(),
        bundle_id: "dev.example.test".into(),
    };
    let other = AppAttestDevice::new(&roots, &other_app, b"appattest\0\0\0\0\0\0\0");
    assert_eq!(
        policy.verify(
            &other.evidence(DESKTOP, PHONE, &nonce, &step_up),
            DESKTOP,
            PHONE,
            &nonce,
            NOW
        ),
        Err(Rejected::App)
    );
    let mut uncategorised = roots.app_attest_device();
    uncategorised.category = 7;
    assert_eq!(
        policy.verify(
            &uncategorised.evidence(DESKTOP, PHONE, &nonce, &step_up),
            DESKTOP,
            PHONE,
            &nonce,
            NOW
        ),
        Err(Rejected::App)
    );
    // Apple's genuine root does not accept the test CA's objects.
    let mut real_roots = AttestationPolicy::default();
    real_roots.apple = policy.apple.clone();
    assert!(matches!(
        real_roots.verify(&evidence, DESKTOP, PHONE, &nonce, NOW),
        Err(Rejected::Chain(_) | Rejected::UntrustedRoot)
    ));
    assert_eq!(
        AttestationPolicy::default().verify(&evidence, DESKTOP, PHONE, &nonce, NOW),
        Err(Rejected::NotConfigured)
    );
}

#[test]
fn app_attest_assertions_need_the_key_the_app_id_and_a_rising_counter() {
    let (_roots, _policy, mut device, _) = apple_setup();
    let key = point(&device.key);
    let rp = device.rp_id_hash;
    let data = assertion_client_data(b"message one");
    let first = device.assert(&data);
    assert_eq!(
        apple::verify_assertion(&first, &data, &key, &rp, 0, &[1]),
        Ok(1)
    );
    // Replay of the same assertion: counter not higher.
    assert_eq!(
        apple::verify_assertion(&first, &data, &key, &rp, 1, &[1]),
        Err(Rejected::Counter)
    );
    // Other client data, key or App ID.
    let second = device.assert(&data);
    assert_eq!(
        apple::verify_assertion(
            &second,
            &assertion_client_data(b"message two"),
            &key,
            &rp,
            1,
            &[1]
        ),
        Err(Rejected::Signature)
    );
    assert_eq!(
        apple::verify_assertion(&second, &data, &point(&random_key()), &rp, 1, &[1]),
        Err(Rejected::Signature)
    );
    assert_eq!(
        apple::verify_assertion(&second, &data, &key, &[0; 32], 1, &[1]),
        Err(Rejected::App)
    );
    assert_eq!(
        apple::verify_assertion(&second, &data, &key, &rp, 1, &[9]),
        Err(Rejected::App)
    );
    assert_eq!(
        apple::verify_assertion(&second, &data, &key, &rp, 1, &[1]),
        Ok(2)
    );
    // Tampering never verifies.
    for i in 0..second.len() {
        let mut broken = second.clone();
        broken[i] ^= 0x01;
        assert!(apple::verify_assertion(&broken, &data, &key, &rp, 1, &[1]).is_err());
    }
}

#[test]
fn cbor_is_strict_and_bounded() {
    use apple::cbor;
    // Non-shortest integers, indefinite lengths, tags and floats.
    for bad in [
        &[0x18, 0x01][..],
        &[0x5f, 0xff],
        &[0xc0, 0x00],
        &[0xf9, 0x00, 0x00],
        &[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        &[0xa2, 0x01, 0x01, 0x01, 0x02],
        &[0x42, 0x00],
    ] {
        assert!(cbor(bad).is_err(), "{bad:x?}");
    }
    // Deep nesting is refused, not recursed without bound.
    assert!(cbor(&[0x81; 64]).is_err());
    // Wide containers are refused before any per-entry work.
    let mut wide = vec![0xb8, 65];
    for k in 0..65u8 {
        wide.extend_from_slice(&[0x18, 24 + k, 0x00]);
    }
    assert!(cbor(&wide).is_err());
    let mut array = vec![0x98, 65];
    array.extend(std::iter::repeat_n(0x00, 65));
    assert!(cbor(&array).is_err());
    let mut fits = vec![0x98, 64];
    fits.extend(std::iter::repeat_n(0x00, 64));
    assert!(cbor(&fits).is_ok());
}

#[test]
fn bindings_are_pinned() {
    let hex = |d: [u8; 32]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    // Independently: sha256(label || 0x01*32 || 0x02*32 || 0x03*32).
    let mut expected = Sha256::new();
    expected.update(b"notdot/step-up/attest/android/v1");
    expected.update([1; 32]);
    expected.update([2; 32]);
    expected.update([3; 32]);
    assert_eq!(
        hex(attestation_challenge(DESKTOP, PHONE, &[3; 32])),
        hex(expected.finalize().into())
    );
    assert_ne!(
        app_attest_client_data_hash(DESKTOP, PHONE, &[3; 32], &[4; 65]),
        app_attest_client_data_hash(DESKTOP, PHONE, &[3; 32], &[5; 65])
    );
    // Evidence round-trips through JSON strictly.
    let roots = TestRoots::new(NOW);
    let (_, evidence) = roots.android_evidence([0; 32]);
    let json = serde_json::to_string(&evidence).unwrap();
    assert_eq!(serde_json::from_str::<Evidence>(&json).unwrap(), evidence);
    assert!(
        serde_json::from_str::<Evidence>(r#"{"platform":"android_keystore","chain":[],"extra":1}"#)
            .is_err()
    );
}
