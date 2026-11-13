//! Test tool: a synthetic desktop for phone (simulator/emulator) tests.
//!
//! `cargo run -p notdot-link --features test-relay --example desktop_sim -- <qr-file> [bind-ip]`
//!
//! Starts a plain-HTTP iroh relay on `bind-ip` (default 127.0.0.1; use the
//! host's LAN address for an Android emulator), a desktop endpoint, and one
//! pairing offer written to `<qr-file>`. Every pairing request is approved
//! automatically: never use this outside tests. Synthetic profile only.
//!
//! Step-up enrolment uses the genuine verifier with Google's and Apple's
//! roots (never test roots). Which apps it accepts comes from the
//! environment, so no local signing data is committed:
//! - `NOTDOT_SIM_ANDROID_APP=<package>:<signing cert SHA-256 hex>[,<hex>…]`
//! - `NOTDOT_SIM_APPLE_APP=<team id>.<bundle id>` and
//!   `NOTDOT_SIM_APPLE_CATEGORIES=<n>[,<n>…]`; add
//!   `NOTDOT_SIM_APPLE_DEVELOPMENT=1` for a development-signed build
//! - `NOTDOT_SIM_STATUS_LIST=<file>`: Google's attestation status list,
//!   fetched no more than two days earlier. Its age is taken from the file's
//!   modification time, which is trusted here: this is a test tool, and a
//!   real desktop records when it fetched the list itself.
//! - `NOTDOT_SIM_GRANT_MINUTES=<n>`: stand in for the owner and allow
//!   sensitive actions from a phone for `n` minutes once its key verifies.
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use notdot_link::{
    attest::{
        AttestationPolicy,
        android::{AndroidApp, StatusList},
        apple::AppleApp,
    },
    pairing::{DeviceProfile, OFFER_TTL_MS, ProfileRef},
    service::system_clock,
    transport::{Desktop, NetConfig, relay_map},
};
use std::{net::IpAddr, time::Duration};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let qr_file = args.next().expect("usage: desktop_sim <qr-file> [bind-ip]");
    let ip: IpAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1".into())
        .parse()
        .expect("bind ip");
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new((ip, 0)));
    let relay = Server::spawn(config).await.expect("relay");
    let url = format!("http://{}", relay.http_addr().expect("http relay"));
    let net = NetConfig {
        relays: Some(relay_map(&url, true).expect("relay url")),
        direct: true,
        insecure_test_relay: false,
        relay_roots: Vec::new(),
    };
    let desktop = Desktop::bind(None, &net, system_clock())
        .await
        .expect("desktop");
    desktop.set_profile_epoch("synthetic", 1);
    desktop.set_attestation_policy(policy_from_env());
    desktop.online().await;
    let qr = desktop
        .create_offer(
            ProfileRef {
                id: "synthetic".into(),
                epoch: 1,
            },
            DeviceProfile::Owner,
            OFFER_TTL_MS,
        )
        .expect("offer");
    std::fs::write(&qr_file, qr.encode()).expect("write QR");
    eprintln!("desktop_sim: relay {url}; QR written; auto-approving");
    let grant_ms = std::env::var("NOTDOT_SIM_GRANT_MINUTES")
        .ok()
        .and_then(|m| m.parse::<u64>().ok())
        .map(|m| m * 60_000);
    let mut phones = Vec::new();
    let mut reported = std::collections::HashMap::new();
    loop {
        for pending in desktop.pending() {
            eprintln!("desktop_sim: approving code {}", pending.sas);
            desktop.approve(&pending).expect("approve");
            phones.push(pending.device);
        }
        for phone in &phones {
            if let (Some(ms), Some(state)) = (grant_ms, desktop.step_up_state(*phone))
                && state.granted_until_ms.is_none()
            {
                let until = system_clock()() + ms;
                if desktop.grant_sensitive(*phone, until).is_ok() {
                    eprintln!(
                        "\ndesktop_sim: granted sensitive actions (test stand-in for the owner)"
                    );
                }
            }
            let state = desktop.step_up_state(*phone);
            if reported.insert(*phone, state.clone()) != Some(state.clone()) {
                eprintln!("\ndesktop_sim: step-up state {state:?}");
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let notes = desktop.service().profile("synthetic").notes.len();
        eprint!("\rdesktop_sim: {notes} note(s)   ");
    }
}

fn policy_from_env() -> AttestationPolicy {
    let mut policy = AttestationPolicy::default();
    if let Ok(app) = std::env::var("NOTDOT_SIM_ANDROID_APP") {
        let (package, signers) = app.split_once(':').expect("package:hex");
        let signers = signers
            .split(',')
            .map(|hex| {
                assert!(
                    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
                    "NOTDOT_SIM_ANDROID_APP: each signer must be 64 hex digits"
                );
                (0..32)
                    .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
                    .collect::<Vec<_>>()
                    .try_into()
                    .expect("32 bytes")
            })
            .collect();
        policy.android_apps = vec![AndroidApp {
            package: package.into(),
            signers,
        }];
    }
    if let Ok(app) = std::env::var("NOTDOT_SIM_APPLE_APP") {
        let (team_id, bundle_id) = app.split_once('.').expect("team.bundle");
        policy.apple.apps = vec![AppleApp {
            team_id: team_id.into(),
            bundle_id: bundle_id.into(),
        }];
        policy.apple.validation_categories = std::env::var("NOTDOT_SIM_APPLE_CATEGORIES")
            .unwrap_or_default()
            .split(',')
            .filter_map(|n| n.trim().parse().ok())
            .collect();
        policy.apple.allow_development = std::env::var("NOTDOT_SIM_APPLE_DEVELOPMENT").is_ok();
    }
    if let Ok(path) = std::env::var("NOTDOT_SIM_STATUS_LIST") {
        let fetched = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .expect("status list file")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_millis() as u64;
        let json = std::fs::read(&path).expect("status list");
        policy.set_status_list(StatusList::parse(&json, fetched).expect("status list format"));
    }
    eprintln!(
        "desktop_sim: genuine attestation roots; {} Android app(s), {} Apple app(s), status list {}",
        policy.android_apps.len(),
        policy.apple.apps.len(),
        if std::env::var("NOTDOT_SIM_STATUS_LIST").is_ok() {
            "loaded"
        } else {
            "missing (Android refused)"
        }
    );
    policy
}
