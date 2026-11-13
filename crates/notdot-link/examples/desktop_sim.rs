//! Test tool: a synthetic desktop for phone (simulator/emulator) tests.
//!
//! `cargo run -p notdot-link --features test-relay --example desktop_sim -- <qr-file> [bind-ip]`
//!
//! Starts a plain-HTTP iroh relay on `bind-ip` (default 127.0.0.1; use the
//! host's LAN address for an Android emulator), a desktop endpoint, and one
//! pairing offer written to `<qr-file>`. Every pairing request is approved
//! automatically: never use this outside tests. Synthetic profile only.
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use notdot_link::{
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
    loop {
        for pending in desktop.pending() {
            eprintln!("desktop_sim: approving code {}", pending.sas);
            desktop.approve(&pending).expect("approve");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let notes = desktop.service().profile("synthetic").notes.len();
        eprint!("\rdesktop_sim: {notes} note(s)   ");
    }
}
