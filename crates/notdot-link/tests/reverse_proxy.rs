//! Relay behind a TLS-terminating reverse proxy, on the local machine.
//!
//! Run with the containers from `deploy/relay` (see its README):
//! `NOTDOT_RELAY_URL=https://localhost:8443 NOTDOT_RELAY_CA=<relay.crt>
//!  cargo test -p notdot-link --test reverse_proxy -- --ignored --test-threads=1`
//!
//! The relay certificate is verified against the given root (no
//! verification is skipped). Synthetic profiles and data only.
use notdot_link::{
    LinkError,
    pairing::{DeviceProfile, OFFER_TTL_MS, ProfileRef},
    protocol::{Op, PairFrame, RequestId, ServerFrame},
    service::system_clock,
    transport::{Desktop, NetConfig, Phone, Route, desktop_addr, relay_map, relay_only},
};
use std::time::Duration;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required; see deploy/relay/README.md"))
}

fn root() -> Vec<u8> {
    let pem = std::fs::read_to_string(env("NOTDOT_RELAY_CA")).unwrap();
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .unwrap()
}

fn net(url: &str, direct: bool) -> NetConfig {
    NetConfig {
        relays: Some(relay_map(url, false).unwrap()),
        direct,
        insecure_test_relay: false,
        relay_roots: vec![root()],
    }
}

async fn paired_over_relay(url: &str) -> (Desktop, Phone) {
    let desktop = Desktop::bind(None, &net(url, false), system_clock())
        .await
        .unwrap();
    desktop.set_profile_epoch("owner", 1);
    tokio::time::timeout(Duration::from_secs(15), desktop.online())
        .await
        .expect("desktop reaches the relay through the proxy");
    let phone = Phone::bind(None, &net(url, false)).await.unwrap();
    let qr = desktop
        .create_offer(
            ProfileRef {
                id: "owner".into(),
                epoch: 1,
            },
            DeviceProfile::Owner,
            OFFER_TTL_MS,
        )
        .unwrap();
    let addr = relay_only(&desktop_addr(&qr).unwrap());
    let pairing = phone.pair(&qr, addr, "Synthetic phone", &[]).await.unwrap();
    let pending = desktop.pending();
    assert_eq!(pending[0].sas, pairing.sas);
    desktop.approve(&pending[0]).unwrap();
    assert!(matches!(
        pairing.outcome().await.unwrap(),
        PairFrame::Approved { .. }
    ));
    (desktop, phone)
}

fn docker(args: &[&str]) {
    let status = std::process::Command::new("docker")
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "docker {args:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the local deploy/relay containers"]
async fn positive_pairing_and_calls_through_the_proxy() {
    let url = env("NOTDOT_RELAY_URL");
    let (desktop, phone) = paired_over_relay(&url).await;
    let link = phone.connect(relay_only(&desktop.addr())).await.unwrap();
    assert_eq!(link.route(), Route::Relay);
    let started = std::time::Instant::now();
    for n in 0..20 {
        let reply = link
            .call(
                RequestId(n),
                Op::AddNote {
                    text: format!("synthetic note {n}"),
                },
                None,
            )
            .await
            .unwrap();
        assert!(matches!(reply, ServerFrame::Done { .. }));
    }
    eprintln!(
        "20 relayed calls through the proxy in {:?} (local loopback; not a WAN latency claim)",
        started.elapsed()
    );
    assert_eq!(desktop.service().profile("owner").notes.len(), 20);
    desktop.close().await;
    phone.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the local deploy/relay containers"]
async fn negative_a_proxy_without_upgrade_forwarding_cannot_relay() {
    // Same nginx and relay, but the 8444 server block drops Upgrade headers.
    let good = env("NOTDOT_RELAY_URL");
    let broken = good.replace(":8443", ":8444");
    assert_ne!(good, broken);
    let desktop = Desktop::bind(None, &net(&broken, false), system_clock())
        .await
        .unwrap();
    let online = tokio::time::timeout(Duration::from_secs(8), desktop.online()).await;
    assert!(
        online.is_err(),
        "relay must not come online without Upgrade"
    );
    let phone = Phone::bind(None, &net(&broken, false)).await.unwrap();
    let attempt = tokio::time::timeout(
        Duration::from_secs(10),
        phone.connect(relay_only(&desktop.addr())),
    )
    .await;
    assert!(
        !matches!(attempt, Ok(Ok(_))),
        "a relay-only connection must fail through the broken proxy"
    );
    desktop.close().await;
    phone.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the local deploy/relay containers; restarts the proxy container"]
async fn reconnect_after_proxy_restart_commits_once() {
    let url = env("NOTDOT_RELAY_URL");
    let proxy = std::env::var("NOTDOT_PROXY_CONTAINER")
        .unwrap_or_else(|_| "notdot-relay-poc-proxy-1".into());
    let (desktop, phone) = paired_over_relay(&url).await;
    let link = phone.connect(relay_only(&desktop.addr())).await.unwrap();
    let id = RequestId::random();
    let task = Op::RunTask {
        label: "synthetic tidy".into(),
        steps: 600,
    };
    let first = {
        let link = link.clone();
        let task = task.clone();
        tokio::spawn(async move { link.call(id, task, None).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A real outage: the proxy is gone for a while, then returns.
    let outage: u64 = std::env::var("NOTDOT_PROXY_OUTAGE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    docker(&["stop", "--timeout", "0", &proxy]);
    tokio::time::sleep(Duration::from_secs(outage)).await;
    docker(&["start", &proxy]);
    // The in-flight call either fails with the connection or completes if
    // QUIC rode out the gap; either way, a retry must not repeat it.
    let first = tokio::time::timeout(Duration::from_secs(60), first).await;
    eprintln!("first attempt after proxy restart: {first:?}");
    let mut outcome = None;
    for attempt in 0..30 {
        match phone.connect(relay_only(&desktop.addr())).await {
            Ok(fresh) => match fresh.call(id, task.clone(), None).await {
                Ok(reply) => {
                    eprintln!("retry succeeded on attempt {attempt}");
                    outcome = Some(reply);
                    break;
                }
                Err(e) => eprintln!("retry {attempt}: {e}"),
            },
            Err(LinkError::Refused(r)) => panic!("refused: {r}"),
            Err(e) => eprintln!("reconnect {attempt}: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(
        matches!(outcome, Some(ServerFrame::Done { .. })),
        "{outcome:?}"
    );
    assert_eq!(desktop.service().profile("owner").tasks_completed, 1);
    desktop.close().await;
    phone.close().await;
}
