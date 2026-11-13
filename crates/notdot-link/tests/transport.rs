//! End-to-end over real Iroh QUIC endpoints on this machine, with a local
//! relay (iroh's test relay, self-signed). Synthetic profiles and data only.
//! Nothing here contacts a third-party relay or discovery service.
use iroh::{EndpointAddr, RelayMap, TransportAddr, test_utils::run_relay_server};
use notdot_link::{
    LinkError,
    pairing::{DeviceProfile, OFFER_TTL_MS, ProfileRef, Refusal},
    protocol::{Op, PairFrame, RequestId, ServerFrame},
    service::system_clock,
    transport::{Desktop, NetConfig, Phone, Route, desktop_addr},
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);

fn net(relays: &RelayMap, direct: bool) -> NetConfig {
    NetConfig {
        relays: Some(relays.clone()),
        direct,
        insecure_test_relay: true,
        relay_roots: Vec::new(),
    }
}

fn owner() -> ProfileRef {
    ProfileRef {
        id: "owner".into(),
        epoch: 1,
    }
}

/// Address of the desktop with only its relay (no IP hints).
fn relay_only(addr: &EndpointAddr) -> EndpointAddr {
    EndpointAddr::from_parts(
        addr.id,
        addr.addrs
            .iter()
            .filter(|a| matches!(a, TransportAddr::Relay(_)))
            .cloned(),
    )
}

async fn paired(relays: &RelayMap, profile: DeviceProfile) -> (Desktop, Phone) {
    let desktop = Desktop::bind(None, &net(relays, true), system_clock())
        .await
        .unwrap();
    desktop.set_profile_epoch("owner", 1);
    desktop.online().await;
    let phone = Phone::bind(None, &net(relays, true)).await.unwrap();
    let qr = desktop
        .create_offer(owner(), profile, OFFER_TTL_MS)
        .unwrap();
    let qr = notdot_link::pairing::QrPayload::decode(&qr.encode()).unwrap();
    let pairing = phone
        .pair(&qr, desktop_addr(&qr).unwrap(), "Synthetic phone", &[])
        .await
        .unwrap();
    let pending = desktop.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].sas, pairing.sas, "both screens show one code");
    assert_eq!(pending[0].device, phone.id());
    desktop.approve(&pending[0]).unwrap();
    assert!(matches!(
        pairing.outcome().await.unwrap(),
        PairFrame::Approved { .. }
    ));
    (desktop, phone)
}

async fn wait_for_route(link: &notdot_link::transport::Link, route: Route) -> bool {
    for _ in 0..100 {
        if link.route() == route {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_then_direct_lan_calls() {
    tokio::time::timeout(TIMEOUT, async {
        let (relays, _url, _relay) = run_relay_server().await.unwrap();
        let (desktop, phone) = paired(&relays, DeviceProfile::Owner).await;
        let link = phone.connect(desktop.addr()).await.unwrap();
        let reply = link
            .call(
                RequestId(1),
                Op::AddNote {
                    text: "oat milk".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert!(matches!(reply, ServerFrame::Done { .. }));
        assert!(
            wait_for_route(&link, Route::Direct).await,
            "same-machine endpoints should use a direct path, got {:?}",
            link.route()
        );
        assert_eq!(desktop.service().profile("owner").notes.len(), 1);
        desktop.close().await;
        phone.close().await;
    })
    .await
    .expect("test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn forced_relay_carries_calls_and_a_retry_after_reconnect_does_not_duplicate() {
    tokio::time::timeout(TIMEOUT, async {
        let (relays, _url, _relay) = run_relay_server().await.unwrap();
        let (desktop, phone) = paired(&relays, DeviceProfile::Owner).await;
        let phone_key = phone.id();
        // Start on the LAN, commit a task, and lose the connection before
        // reading the outcome (a Wi-Fi to cellular switch).
        let lan = phone.connect(desktop.addr()).await.unwrap();
        let id = RequestId::random();
        let task = Op::RunTask {
            label: "tidy notes".into(),
            steps: 60,
        };
        let lost =
            tokio::time::timeout(Duration::from_millis(50), lan.call(id, task.clone(), None)).await;
        assert!(lost.is_err(), "call should still be running");
        lan.close();
        drop(lan);
        // The same phone key, now reachable only through the relay.
        let secret = phone_secret(phone).await;
        let cellular = Phone::bind(Some(secret), &net(&relays, false))
            .await
            .unwrap();
        assert_eq!(cellular.id(), phone_key);
        cellular.online().await;
        let relayed = cellular.connect(relay_only(&desktop.addr())).await.unwrap();
        assert_eq!(relayed.route(), Route::Relay);
        let retried = relayed.call(id, task.clone(), None).await.unwrap();
        assert!(matches!(retried, ServerFrame::Done { .. }), "{retried:?}");
        let again = relayed.call(id, task, None).await.unwrap();
        assert_eq!(retried, again);
        assert_eq!(desktop.service().profile("owner").tasks_completed, 1);
        // Cancellation over the relay.
        let long = RequestId::random();
        let running = {
            let relayed = relayed.clone();
            tokio::spawn(async move {
                relayed
                    .call(
                        long,
                        Op::RunTask {
                            label: "long".into(),
                            steps: 1_000,
                        },
                        None,
                    )
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(relayed.cancel(long).await.unwrap());
        assert_eq!(
            running.await.unwrap().unwrap(),
            ServerFrame::Cancelled { id: long }
        );
        assert_eq!(desktop.service().profile("owner").tasks_completed, 1);
        assert_eq!(relayed.route(), Route::Relay);
        desktop.close().await;
        cellular.close().await;
    })
    .await
    .expect("test timed out");
}

/// Recover a phone's secret key so a new endpoint can use the same
/// identity, as an app restart would from its keychain.
async fn phone_secret(phone: Phone) -> iroh::SecretKey {
    let secret = phone.secret();
    phone.close().await;
    secret
}

#[tokio::test(flavor = "multi_thread")]
async fn unpaired_revoked_and_stale_phones_are_refused() {
    tokio::time::timeout(TIMEOUT, async {
        let (relays, _url, _relay) = run_relay_server().await.unwrap();
        let (desktop, phone) = paired(&relays, DeviceProfile::Member).await;
        let stranger = Phone::bind(None, &net(&relays, true)).await.unwrap();
        match stranger.connect(desktop.addr()).await {
            Err(LinkError::Refused(Refusal::Unknown)) => {}
            other => panic!("stranger: {:?}", other.err()),
        }
        let link = phone.connect(desktop.addr()).await.unwrap();
        // A member phone cannot stage the owner's settings.
        let staged = link
            .call(
                RequestId(1),
                Op::StageSetting {
                    key: "hue".into(),
                    value: "10".into(),
                    base_revision: 0,
                },
                None,
            )
            .await
            .unwrap();
        assert!(matches!(staged, ServerFrame::Failed { .. }));
        // Revocation drops the live connection with a reason.
        let closed = {
            let link = link.clone();
            tokio::spawn(async move { link.closed().await })
        };
        desktop.revoke(phone.id());
        assert_eq!(closed.await.unwrap(), Some(Refusal::Revoked));
        match phone.connect(desktop.addr()).await {
            Err(LinkError::Refused(Refusal::Revoked)) => {}
            other => panic!("revoked: {:?}", other.err()),
        }
        desktop.close().await;
        phone.close().await;
        stranger.close().await;

        // A profile epoch change (e.g. identity mode changed) invalidates.
        let (desktop, phone) = paired(&relays, DeviceProfile::Owner).await;
        desktop.set_profile_epoch("owner", 2);
        match phone.connect(desktop.addr()).await {
            Err(LinkError::Refused(Refusal::StaleProfile)) => {}
            other => panic!("stale: {:?}", other.err()),
        }
        desktop.close().await;
        phone.close().await;
    })
    .await
    .expect("test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_qr_cannot_be_reused_or_redirected() {
    tokio::time::timeout(TIMEOUT, async {
        let (relays, _url, _relay) = run_relay_server().await.unwrap();
        let desktop = Desktop::bind(None, &net(&relays, true), system_clock())
            .await
            .unwrap();
        desktop.set_profile_epoch("owner", 1);
        let qr = desktop
            .create_offer(owner(), DeviceProfile::Owner, OFFER_TTL_MS)
            .unwrap();
        // A QR pointing at a different key than its address is refused
        // before any network traffic.
        let other = Phone::bind(None, &net(&relays, true)).await.unwrap();
        let mut wrong = desktop_addr(&qr).unwrap();
        wrong.id = iroh::SecretKey::generate().public();
        assert!(other.pair(&qr, wrong, "x", &[]).await.is_err());
        // First phone uses it; a second phone is refused.
        let first = Phone::bind(None, &net(&relays, true)).await.unwrap();
        let pairing = first
            .pair(&qr, desktop_addr(&qr).unwrap(), "first", &[])
            .await
            .unwrap();
        match other
            .pair(&qr, desktop_addr(&qr).unwrap(), "second", &[])
            .await
        {
            Err(LinkError::Connection(reason)) => assert!(reason.contains("already used")),
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("second phone accepted"),
        }
        desktop.deny(&desktop.pending()[0]);
        assert_eq!(pairing.outcome().await.unwrap(), PairFrame::Denied);
        desktop.close().await;
        first.close().await;
        other.close().await;
    })
    .await
    .expect("test timed out");
}
