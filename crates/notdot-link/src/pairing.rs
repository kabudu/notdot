//! QR pairing between a desktop and a phone.
//!
//! Established primitives only: identities are the transport's Ed25519 keys
//! (authenticated by the QUIC/TLS handshake, so a peer's [`DeviceId`] is never
//! taken from a message), the one-time secret is proved with HMAC-SHA256, and
//! the comparison code is derived with HKDF-SHA256.
//!
//! 1. The desktop creates an [`Offer`]; its [`QrPayload`] carries the desktop
//!    key (pinned by the phone), an offer id, a 256-bit one-time secret and
//!    an expiry.
//! 2. The phone connects to that exact key and sends a [`PairRequest`] whose
//!    proof binds the secret to both keys, the phone's name and its step-up
//!    key. A copied request is useless from any other phone key.
//! 3. Both sides show the same six-digit code. The owner compares them and
//!    approves on the desktop; voice can never approve.
//! 4. An offer is single-use, expires, and burns after repeated bad proofs.
use crate::{DeviceId, LinkError, random_bytes};
use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{BTreeSet, HashMap, HashSet};

const QR_PREFIX: &str = "notdot-pair:1:";
const PROOF_LABEL: &[u8] = b"notdot/pair/v1/proof";
const SAS_LABEL: &[u8] = b"notdot/pair/v1/sas";
/// Default lifetime of a QR code.
pub const OFFER_TTL_MS: u64 = 120_000;
/// Bad proofs tolerated before an offer is destroyed.
pub const MAX_PROOF_FAILURES: u8 = 5;
/// Open offers per desktop. Each is a few hundred bytes.
pub const MAX_PENDING_OFFERS: usize = 4;
/// Longest phone name accepted, in bytes.
pub const MAX_NAME_BYTES: usize = 64;
/// Paired phones per desktop.
pub const MAX_DEVICES: usize = 16;

pub type OfferId = [u8; 16];

/// What a paired phone may do. A device profile grants a fixed set;
/// [`Scope::Sensitive`] is never part of a profile and needs an explicit
/// owner grant on the desktop plus a biometric step-up per action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Converse,
    Notes,
    Tasks,
    OwnMemory,
    ReadSettings,
    StageSettings,
    Sensitive,
}

/// Device-profile presets chosen on the desktop when the QR is created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceProfile {
    /// The owner's phone: staged settings changes, no sensitive actions
    /// until separately granted.
    Owner,
    /// A trusted member's phone: their own context only.
    Member,
}
impl DeviceProfile {
    pub fn scopes(self) -> BTreeSet<Scope> {
        let mut scopes = BTreeSet::from([
            Scope::Converse,
            Scope::Notes,
            Scope::Tasks,
            Scope::OwnMemory,
            Scope::ReadSettings,
        ]);
        if self == Self::Owner {
            scopes.insert(Scope::StageSettings);
        }
        scopes
    }
}

/// A household profile and its security epoch. Removing a member or
/// changing identity mode bumps the epoch and invalidates its phones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRef {
    pub id: String,
    pub epoch: u64,
}

/// Contents of the pairing QR code. Network hints are only hints: the
/// desktop is authenticated by `desktop`, never by where it was found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QrPayload {
    pub desktop: DeviceId,
    #[serde(with = "crate::b64")]
    pub offer: Vec<u8>,
    #[serde(with = "crate::b64")]
    pub secret: Vec<u8>,
    pub expires_ms: u64,
    /// Relay URL the desktop is reachable through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
    /// Direct addresses seen on the desktop (LAN hints).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub direct: Vec<std::net::SocketAddr>,
}
impl QrPayload {
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("QR payload serialises");
        QR_PREFIX.to_owned() + &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }
    /// Strict parse: known version, exact field set, correct lengths.
    pub fn decode(text: &str) -> Result<Self, LinkError> {
        let body = text
            .strip_prefix(QR_PREFIX)
            .ok_or(LinkError::BadQr("not a notdot pairing code"))?;
        if body.len() > 2_048 {
            return Err(LinkError::BadQr("too long"));
        }
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| LinkError::BadQr("not base64url"))?;
        let payload: Self =
            serde_json::from_slice(&json).map_err(|_| LinkError::BadQr("malformed"))?;
        if payload.offer.len() != 16 || payload.secret.len() != 32 || payload.direct.len() > 8 {
            return Err(LinkError::BadQr("wrong field length"));
        }
        Ok(payload)
    }
    pub fn offer_id(&self) -> OfferId {
        self.offer.as_slice().try_into().expect("checked length")
    }
}

/// What a phone keeps about the desktop it is paired with, so it can
/// reconnect after a restart without the QR. It holds no pairing secret
/// (the offer and its secret are single-use and spent): only the desktop's
/// public key, which is what the phone authenticates, and reachability
/// hints. Store it with the phone's identity key; on its own it grants
/// nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedDesktop {
    pub desktop: DeviceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub direct: Vec<std::net::SocketAddr>,
}
const PAIRED_PREFIX: &str = "notdot-paired:1:";
impl PairedDesktop {
    pub fn from_qr(qr: &QrPayload) -> Self {
        Self {
            desktop: qr.desktop,
            relay: qr.relay.clone(),
            direct: qr.direct.clone(),
        }
    }
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("paired record serialises");
        PAIRED_PREFIX.to_owned() + &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }
    /// Strict parse, with the same limits as a QR code.
    pub fn decode(text: &str) -> Result<Self, LinkError> {
        let body = text
            .strip_prefix(PAIRED_PREFIX)
            .ok_or(LinkError::BadQr("not a notdot paired record"))?;
        if body.len() > 2_048 {
            return Err(LinkError::BadQr("too long"));
        }
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| LinkError::BadQr("not base64url"))?;
        let record: Self =
            serde_json::from_slice(&json).map_err(|_| LinkError::BadQr("malformed"))?;
        if record.direct.len() > 8 {
            return Err(LinkError::BadQr("wrong field length"));
        }
        Ok(record)
    }
}

/// Sent by the phone over a connection already authenticated to `desktop`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    #[serde(with = "crate::b64")]
    pub offer: Vec<u8>,
    pub name: String,
    #[serde(with = "crate::b64")]
    pub proof: Vec<u8>,
}

/// The six-digit comparison code shown on both screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sas(pub u32);
impl std::fmt::Display for Sas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:03} {:03}", self.0 / 1000, self.0 % 1000)
    }
}

fn proof_mac(
    secret: &[u8],
    offer: &[u8],
    desktop: DeviceId,
    phone: DeviceId,
    name: &str,
) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(PROOF_LABEL);
    mac.update(offer);
    mac.update(&desktop.0);
    mac.update(&phone.0);
    // Length-prefix variable fields so boundaries cannot shift.
    mac.update(&(name.len() as u32).to_be_bytes());
    mac.update(name.as_bytes());
    mac
}

/// Derive the comparison code for a desktop/phone pair.
pub fn sas(secret: &[u8], offer: &[u8], desktop: DeviceId, phone: DeviceId) -> Sas {
    let hk = Hkdf::<Sha256>::new(Some(offer), secret);
    let mut info = SAS_LABEL.to_vec();
    info.extend_from_slice(&desktop.0);
    info.extend_from_slice(&phone.0);
    let mut out = [0u8; 4];
    hk.expand(&info, &mut out)
        .expect("4 bytes is a valid HKDF length");
    Sas(u32::from_be_bytes(out) % 1_000_000)
}

/// Invisible Unicode format characters (bidi overrides, zero-width marks)
/// that could make a phone name look like another on the approval screen.
fn is_format(c: char) -> bool {
    matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// Phone side: build the request for a scanned QR and the code to display.
/// `phone` must be the key the phone's transport will authenticate with.
pub fn phone_request(qr: &QrPayload, phone: DeviceId, name: &str) -> (PairRequest, Sas) {
    let proof = proof_mac(&qr.secret, &qr.offer, qr.desktop, phone, name)
        .finalize()
        .into_bytes()
        .to_vec();
    (
        PairRequest {
            offer: qr.offer.clone(),
            name: name.to_owned(),
            proof,
        },
        sas(&qr.secret, &qr.offer, qr.desktop, phone),
    )
}

/// A phone the desktop has approved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub device: DeviceId,
    pub name: String,
    pub profile: ProfileRef,
    pub device_profile: DeviceProfile,
    pub scopes: BTreeSet<Scope>,
    pub paired_ms: u64,
}
impl DeviceRecord {
    pub fn allows(&self, scope: Scope) -> bool {
        match scope {
            // Only an owner phone may ask; each action still needs an
            // attested step-up key, a live owner grant and a fresh step-up,
            // which the service checks.
            Scope::Sensitive => self.device_profile == DeviceProfile::Owner,
            scope => self.scopes.contains(&scope),
        }
    }
}

/// Why a connection was refused. Sent to the phone so it can explain, and
/// wipe its keys when revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    #[error("this phone is not paired with this computer")]
    Unknown,
    #[error("this phone was removed from this computer")]
    Revoked,
    #[error("this phone's profile changed; pair it again")]
    StaleProfile,
}

/// The desktop's set of paired phones. The desktop transport persists
/// public identities and scopes in its private state directory; private
/// identity keys are held separately in the OS credential store.
#[derive(Debug, Default)]
pub struct DeviceRegistry {
    devices: HashMap<DeviceId, DeviceRecord>,
    revoked: HashSet<DeviceId>,
}
impl DeviceRegistry {
    /// Look up an authenticated peer. `epoch` returns a profile's current
    /// security epoch, or `None` if the profile no longer exists.
    pub fn authorize(
        &self,
        peer: DeviceId,
        epoch: impl Fn(&str) -> Option<u64>,
    ) -> Result<&DeviceRecord, Refusal> {
        if self.revoked.contains(&peer) {
            return Err(Refusal::Revoked);
        }
        let record = self.devices.get(&peer).ok_or(Refusal::Unknown)?;
        if epoch(&record.profile.id) != Some(record.profile.epoch) {
            return Err(Refusal::StaleProfile);
        }
        Ok(record)
    }
    fn insert(&mut self, record: DeviceRecord) -> Result<(), LinkError> {
        if !self.devices.contains_key(&record.device) && self.devices.len() >= MAX_DEVICES {
            return Err(LinkError::TooManyDevices);
        }
        // An owner re-pairing a removed phone with a new QR is deliberate.
        self.revoked.remove(&record.device);
        self.devices.insert(record.device, record);
        Ok(())
    }
    /// Put back a record removed by an unsaved approval.
    pub(crate) fn insert_restored(&mut self, record: DeviceRecord) -> Result<(), LinkError> {
        self.insert(record)
    }

    /// Undo an approval that could not be saved.
    pub(crate) fn remove(&mut self, device: &DeviceId) {
        self.devices.remove(device);
    }

    /// Remove a phone. Its key stays refused so it can be told to wipe.
    pub fn revoke(&mut self, device: DeviceId) -> bool {
        self.revoked.insert(device);
        self.devices.remove(&device).is_some()
    }
    pub fn snapshot(&self) -> crate::store::RegistrySnapshot {
        let mut devices: Vec<_> = self.devices.values().cloned().collect();
        devices.sort_by_key(|d| d.device.0);
        let mut revoked: Vec<_> = self.revoked.iter().copied().collect();
        revoked.sort_by_key(|d| d.0);
        crate::store::RegistrySnapshot {
            devices,
            revoked,
            port: None,
        }
    }

    pub fn restore(snapshot: crate::store::RegistrySnapshot) -> Self {
        Self {
            devices: snapshot
                .devices
                .into_iter()
                .map(|d| (d.device, d))
                .collect(),
            revoked: snapshot.revoked.into_iter().collect(),
        }
    }

    pub fn list(&self) -> Vec<DeviceRecord> {
        self.snapshot().devices
    }

    pub fn get(&self, device: &DeviceId) -> Option<&DeviceRecord> {
        self.devices.get(device)
    }
    pub fn len(&self) -> usize {
        self.devices.len()
    }
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }
}

/// A phone waiting for the owner to compare codes and approve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub offer: OfferId,
    pub device: DeviceId,
    pub name: String,
    pub sas: Sas,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfferState {
    Open,
    AwaitingApproval(PendingApproval),
    Approved(DeviceId),
    Denied,
}

#[derive(Debug)]
struct Offer {
    secret: [u8; 32],
    expires_ms: u64,
    profile: ProfileRef,
    device_profile: DeviceProfile,
    failures: u8,
    state: OfferState,
}

/// Desktop pairing state: open offers and decisions.
#[derive(Debug)]
pub struct PairingDesk {
    desktop: DeviceId,
    offers: HashMap<OfferId, Offer>,
}
impl PairingDesk {
    pub fn new(desktop: DeviceId) -> Self {
        Self {
            desktop,
            offers: HashMap::new(),
        }
    }

    fn prune(&mut self, now_ms: u64) {
        // Decided offers are kept until expiry so a retry sees the decision.
        self.offers.retain(|_, offer| now_ms < offer.expires_ms);
    }

    /// Create a QR code for one phone. Owner action on the desktop.
    pub fn create_offer(
        &mut self,
        profile: ProfileRef,
        device_profile: DeviceProfile,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<QrPayload, LinkError> {
        self.prune(now_ms);
        if self.offers.len() >= MAX_PENDING_OFFERS {
            return Err(LinkError::TooManyOffers);
        }
        let offer: OfferId = random_bytes();
        let secret: [u8; 32] = random_bytes();
        let expires_ms = now_ms.saturating_add(ttl_ms.min(OFFER_TTL_MS));
        self.offers.insert(
            offer,
            Offer {
                secret,
                expires_ms,
                profile,
                device_profile,
                failures: 0,
                state: OfferState::Open,
            },
        );
        Ok(QrPayload {
            desktop: self.desktop,
            offer: offer.to_vec(),
            secret: secret.to_vec(),
            expires_ms,
            relay: None,
            direct: Vec::new(),
        })
    }

    /// Check a phone's request. `peer` is the key its connection
    /// authenticated with. On success the offer waits for approval.
    pub fn submit(
        &mut self,
        peer: DeviceId,
        request: &PairRequest,
        now_ms: u64,
    ) -> Result<PendingApproval, LinkError> {
        self.prune(now_ms);
        let offer_id: OfferId = request
            .offer
            .as_slice()
            .try_into()
            .map_err(|_| LinkError::BadRequest("offer id length"))?;
        let desktop = self.desktop;
        let offer = self
            .offers
            .get_mut(&offer_id)
            .ok_or(LinkError::OfferUnavailable)?;
        if offer.state != OfferState::Open {
            return Err(LinkError::OfferUsed);
        }
        if request.name.trim().is_empty()
            || request.name.len() > MAX_NAME_BYTES
            || request.name.chars().any(|c| c.is_control() || is_format(c))
        {
            return Err(LinkError::BadRequest("phone name"));
        }
        let mac = proof_mac(&offer.secret, &request.offer, desktop, peer, &request.name);
        if mac.verify_slice(&request.proof).is_err() {
            offer.failures += 1;
            if offer.failures >= MAX_PROOF_FAILURES {
                self.offers.remove(&offer_id);
            }
            return Err(LinkError::BadProof);
        }
        let pending = PendingApproval {
            offer: offer_id,
            device: peer,
            name: request.name.clone(),
            sas: sas(&offer.secret, &request.offer, desktop, peer),
        };
        offer.state = OfferState::AwaitingApproval(pending.clone());
        Ok(pending)
    }

    /// Owner approves on the desktop after comparing codes.
    pub fn approve(
        &mut self,
        offer_id: &OfferId,
        registry: &mut DeviceRegistry,
        now_ms: u64,
    ) -> Result<DeviceRecord, LinkError> {
        self.prune(now_ms);
        let offer = self
            .offers
            .get_mut(offer_id)
            .ok_or(LinkError::OfferUnavailable)?;
        let OfferState::AwaitingApproval(pending) = &offer.state else {
            return Err(LinkError::OfferUsed);
        };
        let record = DeviceRecord {
            device: pending.device,
            name: pending.name.clone(),
            profile: offer.profile.clone(),
            device_profile: offer.device_profile,
            scopes: offer.device_profile.scopes(),
            paired_ms: now_ms,
        };
        registry.insert(record.clone())?;
        offer.state = OfferState::Approved(record.device);
        Ok(record)
    }

    pub fn deny(&mut self, offer_id: &OfferId) {
        if let Some(offer) = self.offers.get_mut(offer_id) {
            offer.state = OfferState::Denied;
        }
    }

    pub fn state(&self, offer_id: &OfferId) -> Option<&OfferState> {
        self.offers.get(offer_id).map(|offer| &offer.state)
    }

    pub fn expires_ms(&self, offer_id: &OfferId) -> Option<u64> {
        self.offers.get(offer_id).map(|offer| offer.expires_ms)
    }

    pub fn pending(&self) -> impl Iterator<Item = &PendingApproval> {
        self.offers.values().filter_map(|offer| match &offer.state {
            OfferState::AwaitingApproval(pending) => Some(pending),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_paired_record_round_trips_and_carries_no_pairing_secret() {
        let qr = QrPayload {
            desktop: DeviceId([7; 32]),
            offer: vec![1; 16],
            secret: vec![2; 32],
            expires_ms: 5,
            relay: Some("https://relay.example.test/".into()),
            direct: vec!["192.0.2.1:4433".parse().unwrap()],
        };
        let record = PairedDesktop::from_qr(&qr);
        let text = record.encode();
        assert_eq!(PairedDesktop::decode(&text).unwrap(), record);
        let json = String::from_utf8(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(text.strip_prefix(PAIRED_PREFIX).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(
            !json.contains("secret") && !json.contains("offer"),
            "{json}"
        );
        // A QR is not a record, unknown fields and oversize input are refused.
        assert!(PairedDesktop::decode(&qr.encode()).is_err());
        let extra = PAIRED_PREFIX.to_owned()
            + &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.replacen(
                '{',
                r#"{"secret":"AA","#,
                1,
            ));
        assert!(PairedDesktop::decode(&extra).is_err());
        assert!(PairedDesktop::decode(&(PAIRED_PREFIX.to_owned() + &"A".repeat(2_049))).is_err());
    }

    use super::*;

    const DESKTOP: DeviceId = DeviceId([1; 32]);
    const PHONE: DeviceId = DeviceId([2; 32]);
    const THIEF: DeviceId = DeviceId([3; 32]);

    fn owner() -> ProfileRef {
        ProfileRef {
            id: "owner".into(),
            epoch: 1,
        }
    }

    fn desk_with_offer(now: u64) -> (PairingDesk, QrPayload) {
        let mut desk = PairingDesk::new(DESKTOP);
        let qr = desk
            .create_offer(owner(), DeviceProfile::Owner, now, OFFER_TTL_MS)
            .unwrap();
        (desk, qr)
    }

    #[test]
    fn qr_round_trips_strictly() {
        let (_, qr) = desk_with_offer(0);
        let text = qr.encode();
        assert!(text.len() < 300, "{}", text.len());
        assert_eq!(QrPayload::decode(&text).unwrap(), qr);
        assert!(QrPayload::decode("notdot-pair:2:abc").is_err());
        assert!(QrPayload::decode(&text[..text.len() - 4]).is_err());
        let mut extra: serde_json::Value = serde_json::to_value(&qr).unwrap();
        extra["admin"] = true.into();
        let tampered = QR_PREFIX.to_owned()
            + &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&extra).unwrap());
        assert!(QrPayload::decode(&tampered).is_err());
    }

    #[test]
    fn approved_pairing_registers_profile_scopes_and_matching_codes() {
        let (mut desk, qr) = desk_with_offer(0);
        let (request, phone_sas) = phone_request(&qr, PHONE, "Ama's phone");
        let pending = desk.submit(PHONE, &request, 10).unwrap();
        assert_eq!(pending.sas, phone_sas);
        assert_eq!(phone_sas.to_string().len(), 7);
        let mut registry = DeviceRegistry::default();
        let record = desk.approve(&pending.offer, &mut registry, 20).unwrap();
        assert_eq!(record.scopes, DeviceProfile::Owner.scopes());
        // An owner phone may ask; the service still needs an attested key,
        // a grant and a step-up for every sensitive action.
        assert!(record.allows(Scope::Sensitive));
        assert_eq!(
            registry.authorize(PHONE, |_| Some(1)).unwrap().name,
            "Ama's phone"
        );
    }

    #[test]
    fn a_copied_request_is_useless_from_another_key() {
        let (mut desk, qr) = desk_with_offer(0);
        let (request, _) = phone_request(&qr, PHONE, "phone");
        assert!(matches!(
            desk.submit(THIEF, &request, 1),
            Err(LinkError::BadProof)
        ));
        // The genuine phone can still use the offer.
        desk.submit(PHONE, &request, 2).unwrap();
    }

    #[test]
    fn a_photographed_qr_yields_a_different_code_for_the_thief() {
        let (mut desk, qr) = desk_with_offer(0);
        let (thief_request, thief_sas) = phone_request(&qr, THIEF, "phone");
        let shown_on_desktop = desk.submit(THIEF, &thief_request, 1).unwrap().sas;
        assert_eq!(shown_on_desktop, thief_sas);
        let (_, owner_sees) = phone_request(&qr, PHONE, "phone");
        assert_ne!(shown_on_desktop, owner_sees, "owner must see a mismatch");
        // And the owner's own phone is now refused: one phone per code.
        let (request, _) = phone_request(&qr, PHONE, "phone");
        assert!(matches!(
            desk.submit(PHONE, &request, 2),
            Err(LinkError::OfferUsed)
        ));
    }

    #[test]
    fn offers_are_single_use_expire_and_burn_after_bad_proofs() {
        let (mut desk, qr) = desk_with_offer(0);
        let (request, _) = phone_request(&qr, PHONE, "phone");
        let pending = desk.submit(PHONE, &request, 1).unwrap();
        let mut registry = DeviceRegistry::default();
        desk.approve(&pending.offer, &mut registry, 2).unwrap();
        assert!(matches!(
            desk.submit(PHONE, &request, 3),
            Err(LinkError::OfferUsed)
        ));
        assert!(matches!(
            desk.approve(&pending.offer, &mut registry, 3),
            Err(LinkError::OfferUsed)
        ));

        let (mut desk, qr) = desk_with_offer(0);
        let (request, _) = phone_request(&qr, PHONE, "phone");
        assert!(matches!(
            desk.submit(PHONE, &request, OFFER_TTL_MS),
            Err(LinkError::OfferUnavailable)
        ));

        let (mut desk, qr) = desk_with_offer(0);
        let (mut request, _) = phone_request(&qr, PHONE, "phone");
        let good = request.proof.clone();
        request.proof[0] ^= 1;
        for _ in 0..MAX_PROOF_FAILURES {
            assert!(matches!(
                desk.submit(PHONE, &request, 1),
                Err(LinkError::BadProof)
            ));
        }
        request.proof = good;
        assert!(matches!(
            desk.submit(PHONE, &request, 2),
            Err(LinkError::OfferUnavailable)
        ));
    }

    #[test]
    fn approval_needs_a_submitted_request_and_denial_is_final() {
        let (mut desk, qr) = desk_with_offer(0);
        let offer = qr.offer_id();
        let mut registry = DeviceRegistry::default();
        assert!(desk.approve(&offer, &mut registry, 1).is_err());
        let (request, _) = phone_request(&qr, PHONE, "phone");
        desk.submit(PHONE, &request, 1).unwrap();
        desk.deny(&offer);
        assert!(desk.approve(&offer, &mut registry, 2).is_err());
        assert!(registry.is_empty());
    }

    #[test]
    fn pending_offers_and_names_are_bounded() {
        let mut desk = PairingDesk::new(DESKTOP);
        for _ in 0..MAX_PENDING_OFFERS {
            desk.create_offer(owner(), DeviceProfile::Member, 0, OFFER_TTL_MS)
                .unwrap();
        }
        assert!(matches!(
            desk.create_offer(owner(), DeviceProfile::Member, 0, OFFER_TTL_MS),
            Err(LinkError::TooManyOffers)
        ));
        // Expired offers free their slots.
        desk.create_offer(owner(), DeviceProfile::Member, OFFER_TTL_MS, OFFER_TTL_MS)
            .unwrap();
        let (mut desk, qr) = desk_with_offer(0);
        let (request, _) = phone_request(&qr, PHONE, &"x".repeat(MAX_NAME_BYTES + 1));
        assert!(desk.submit(PHONE, &request, 1).is_err());
        for name in [
            "bad\u{7}name",
            "Ama\u{202E}enohp",
            "zero\u{200B}width",
            "   ",
        ] {
            let (request, _) = phone_request(&qr, PHONE, name);
            assert!(desk.submit(PHONE, &request, 1).is_err(), "{name:?}");
        }
    }

    #[test]
    fn revocation_and_profile_epochs_refuse_connections() {
        let (mut desk, qr) = desk_with_offer(0);
        let (request, _) = phone_request(&qr, PHONE, "phone");
        let pending = desk.submit(PHONE, &request, 1).unwrap();
        let mut registry = DeviceRegistry::default();
        desk.approve(&pending.offer, &mut registry, 2).unwrap();
        assert_eq!(
            registry.authorize(PHONE, |_| Some(2)).unwrap_err(),
            Refusal::StaleProfile
        );
        assert_eq!(
            registry.authorize(PHONE, |_| None).unwrap_err(),
            Refusal::StaleProfile
        );
        assert_eq!(
            registry.authorize(THIEF, |_| Some(1)).unwrap_err(),
            Refusal::Unknown
        );
        assert!(registry.revoke(PHONE));
        assert_eq!(
            registry.authorize(PHONE, |_| Some(1)).unwrap_err(),
            Refusal::Revoked
        );
    }

    #[test]
    fn only_owner_phones_may_ask_for_sensitive_actions() {
        let mut registry = DeviceRegistry::default();
        for (device, profile) in [
            (PHONE, DeviceProfile::Owner),
            (THIEF, DeviceProfile::Member),
        ] {
            let (mut desk, _) = desk_with_offer(0);
            let qr = desk
                .create_offer(owner(), profile, 0, OFFER_TTL_MS)
                .unwrap();
            let (request, _) = phone_request(&qr, device, "phone");
            let pending = desk.submit(device, &request, 1).unwrap();
            desk.approve(&pending.offer, &mut registry, 1).unwrap();
        }
        assert!(registry.get(&PHONE).unwrap().allows(Scope::Sensitive));
        let member = registry.get(&THIEF).unwrap();
        assert!(!member.allows(Scope::Sensitive));
        assert!(!member.allows(Scope::StageSettings));
    }
}
