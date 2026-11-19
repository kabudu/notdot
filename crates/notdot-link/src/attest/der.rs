//! A small strict DER reader and the X.509 checks attestation needs.
//!
//! Only definite, minimally encoded lengths are accepted. Tag numbers may use
//! the high-tag-number form, which Android's attestation extension needs
//! (tags such as `[709]`) and general-purpose DER crates often lack.
use super::Rejected;

pub const SEQUENCE: u8 = 0x30;
pub const SET: u8 = 0x31;
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const NULL: u8 = 0x05;
pub const OID: u8 = 0x06;
pub const ENUMERATED: u8 = 0x0a;
pub const BOOLEAN: u8 = 0x01;
pub const UTC_TIME: u8 = 0x17;
pub const GENERALIZED_TIME: u8 = 0x18;

/// One element: class/constructed bits, tag number, contents.
#[derive(Clone, Copy, Debug)]
pub struct Tlv<'a> {
    /// The first identifier octet with the tag-number bits cleared.
    pub class: u8,
    pub number: u32,
    pub value: &'a [u8],
    /// The whole encoding, identifier to end of contents.
    pub raw: &'a [u8],
}
impl<'a> Tlv<'a> {
    /// The single-octet identifier for universal tags (e.g. `SEQUENCE`).
    pub fn tag(&self) -> Option<u8> {
        (self.number < 31).then_some(self.class | self.number as u8)
    }
    pub fn is(&self, tag: u8) -> bool {
        self.tag() == Some(tag)
    }
    pub fn expect(self, tag: u8) -> Result<&'a [u8], Rejected> {
        if self.is(tag) {
            Ok(self.value)
        } else {
            Err(Rejected::Malformed("unexpected DER tag"))
        }
    }
    /// A context-specific constructed tag `[n]`.
    pub fn is_context(&self, number: u32) -> bool {
        self.class == 0xa0 && self.number == number
    }
}

/// Reads consecutive elements from a byte slice.
pub struct Reader<'a> {
    rest: &'a [u8],
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }
    pub fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }
    pub fn peek(&self) -> Option<Tlv<'a>> {
        Reader { rest: self.rest }.next().ok()
    }
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Tlv<'a>, Rejected> {
        let bad = Rejected::Malformed("bad DER");
        let start = self.rest;
        let (&first, mut rest) = self.rest.split_first().ok_or(bad.clone())?;
        let mut number = u32::from(first & 0x1f);
        if number == 0x1f {
            number = 0;
            loop {
                let (&byte, tail) = rest.split_first().ok_or(bad.clone())?;
                rest = tail;
                // Minimal: no leading 0x80 octet; and at most 4 octets.
                if number == 0 && byte == 0x80 || number > (u32::MAX >> 7) {
                    return Err(bad);
                }
                number = (number << 7) | u32::from(byte & 0x7f);
                if byte & 0x80 == 0 {
                    break;
                }
            }
            if number < 31 {
                return Err(bad);
            }
        }
        let (&len0, mut rest2) = rest.split_first().ok_or(bad.clone())?;
        let len = if len0 < 0x80 {
            usize::from(len0)
        } else {
            let count = usize::from(len0 & 0x7f);
            // 0x80 is BER's indefinite length; more than 4 octets is absurd here.
            if count == 0 || count > 4 || rest2.len() < count {
                return Err(bad);
            }
            let (octets, tail) = rest2.split_at(count);
            rest2 = tail;
            if octets[0] == 0 {
                return Err(bad);
            }
            let len = octets
                .iter()
                .fold(0usize, |n, &b| (n << 8) | usize::from(b));
            if len < 0x80 {
                return Err(bad);
            }
            len
        };
        if rest2.len() < len {
            return Err(bad);
        }
        let (value, tail) = rest2.split_at(len);
        let header = start.len() - rest2.len();
        self.rest = tail;
        Ok(Tlv {
            class: first & 0xe0,
            number,
            value,
            raw: &start[..header + len],
        })
    }
    pub fn take(&mut self, tag: u8) -> Result<&'a [u8], Rejected> {
        self.next()?.expect(tag)
    }
    /// The next element if it is the context tag `[n]`.
    pub fn optional_context(&mut self, number: u32) -> Result<Option<Tlv<'a>>, Rejected> {
        match self.peek() {
            Some(tlv) if tlv.is_context(number) => self.next().map(Some),
            _ => Ok(None),
        }
    }
    pub fn finish(&self) -> Result<(), Rejected> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(Rejected::Malformed("trailing DER bytes"))
        }
    }
}

/// Parse exactly one element spanning all of `bytes`.
pub fn single(bytes: &[u8], tag: u8) -> Result<&[u8], Rejected> {
    let mut reader = Reader::new(bytes);
    let value = reader.take(tag)?;
    reader.finish()?;
    Ok(value)
}

/// A non-negative INTEGER that fits in u64.
pub fn uint(value: &[u8]) -> Result<u64, Rejected> {
    let bad = Rejected::Malformed("bad integer");
    match value {
        [] => Err(bad),
        [first, ..] if first & 0x80 != 0 => Err(bad),
        [0, second, ..] if second & 0x80 == 0 => Err(bad),
        _ => {
            let digits = if value[0] == 0 { &value[1..] } else { value };
            if digits.len() > 8 {
                return Err(bad);
            }
            Ok(digits.iter().fold(0u64, |n, &b| (n << 8) | u64::from(b)))
        }
    }
}

pub fn boolean(value: &[u8]) -> Result<bool, Rejected> {
    match value {
        [0x00] => Ok(false),
        [0xff] => Ok(true),
        _ => Err(Rejected::Malformed("bad boolean")),
    }
}

/// Encoded OID contents (after tag and length) for comparison.
pub mod oids {
    /// 1.3.6.1.4.1.11129.2.1.17: Android key attestation.
    pub const ANDROID_KEY_ATTESTATION: &[u8] = &[0x2b, 6, 1, 4, 1, 0xd6, 0x79, 2, 1, 17];
    /// 1.2.840.113635.100.8.2: App Attest nonce.
    pub const APPLE_NONCE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x63, 0x64, 8, 2];
    pub const BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
    pub const KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
    pub const EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
    pub const CERTIFICATE_POLICIES: &[u8] = &[0x55, 0x1d, 0x20];
    pub const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 2, 1];
    pub const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 3, 1, 7];
    pub const P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
    pub const RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 1];
    pub const ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 4, 3, 2];
    pub const ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 4, 3, 3];
    pub const RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 11];
    pub const RSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 12];
    pub const RSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 13];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    P256,
    P384,
    Rsa,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignatureKind {
    EcdsaSha256,
    EcdsaSha384,
    RsaSha256,
    RsaSha384,
    RsaSha512,
}

pub struct Extension<'a> {
    pub oid: &'a [u8],
    pub critical: bool,
    pub value: &'a [u8],
}

/// The parts of a certificate the verifiers use.
pub struct Certificate<'a> {
    pub tbs: &'a [u8],
    signature_kind: SignatureKind,
    signature: &'a [u8],
    /// Serial number contents (big-endian, as encoded).
    pub serial: &'a [u8],
    pub issuer: &'a [u8],
    pub subject: &'a [u8],
    pub not_before_ms: i64,
    pub not_after_ms: i64,
    /// The whole SubjectPublicKeyInfo, for pinning.
    pub spki: &'a [u8],
    pub key_kind: Option<KeyKind>,
    /// The key bits: an uncompressed EC point, or a PKCS#1 RSA key.
    pub key: &'a [u8],
    pub extensions: Vec<Extension<'a>>,
}

fn signature_kind(algorithm: &[u8]) -> Result<SignatureKind, Rejected> {
    let mut reader = Reader::new(algorithm);
    let oid = reader.take(OID)?;
    // RSA algorithms carry an explicit NULL parameter; ECDSA ones none.
    let kind = match oid {
        oids::ECDSA_SHA256 => SignatureKind::EcdsaSha256,
        oids::ECDSA_SHA384 => SignatureKind::EcdsaSha384,
        oids::RSA_SHA256 => SignatureKind::RsaSha256,
        oids::RSA_SHA384 => SignatureKind::RsaSha384,
        oids::RSA_SHA512 => SignatureKind::RsaSha512,
        _ => return Err(Rejected::Unsupported("certificate signature algorithm")),
    };
    if matches!(
        kind,
        SignatureKind::RsaSha256 | SignatureKind::RsaSha384 | SignatureKind::RsaSha512
    ) && !reader.is_empty()
    {
        reader.take(NULL)?;
    }
    reader.finish()?;
    Ok(kind)
}

/// Unix milliseconds for a UTCTime or GeneralizedTime in `Z` form.
fn time(tlv: Tlv<'_>) -> Result<i64, Rejected> {
    let bad = Rejected::Malformed("bad time");
    // ASCII only, so the fixed-offset slicing below cannot split a character.
    if !tlv.value.is_ascii() {
        return Err(bad);
    }
    let text = std::str::from_utf8(tlv.value).map_err(|_| bad.clone())?;
    let digits = |s: &str| -> Result<i64, Rejected> {
        if s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse().map_err(|_| bad.clone())
        } else {
            Err(bad.clone())
        }
    };
    let (year, rest) = match (tlv.tag(), text.len()) {
        (Some(UTC_TIME), 13) => {
            let yy = digits(&text[..2])?;
            (if yy < 50 { 2000 + yy } else { 1900 + yy }, &text[2..])
        }
        (Some(GENERALIZED_TIME), 15) => (digits(&text[..4])?, &text[4..]),
        _ => return Err(bad),
    };
    if !rest.ends_with('Z') {
        return Err(bad);
    }
    let field = |i: usize| digits(&rest[i..i + 2]);
    let (month, day, hour, minute, second) =
        (field(0)?, field(2)?, field(4)?, field(6)?, field(8)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(bad);
    }
    Ok((days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second) * 1000)
}

/// Days since 1970-01-01 (proleptic Gregorian), Howard Hinnant's algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl<'a> Certificate<'a> {
    pub fn parse(der: &'a [u8]) -> Result<Self, Rejected> {
        let mut outer = Reader::new(single(der, SEQUENCE)?);
        let tbs_tlv = outer.next()?;
        let tbs = tbs_tlv.raw;
        let signature_algorithm = outer.take(SEQUENCE)?;
        let signature_bits = outer.take(BIT_STRING)?;
        outer.finish()?;
        let signature_kind = signature_kind(signature_algorithm)?;
        let signature = match signature_bits {
            [0, rest @ ..] => rest,
            _ => return Err(Rejected::Malformed("bad signature bits")),
        };

        let mut fields = Reader::new(tbs_tlv.expect(SEQUENCE)?);
        // v3 only: extensions are required.
        let version = fields
            .optional_context(0)?
            .ok_or(Rejected::Malformed("certificate is not v3"))?;
        if uint(single(version.value, INTEGER)?)? != 2 {
            return Err(Rejected::Malformed("certificate is not v3"));
        }
        let serial = fields.take(INTEGER)?;
        // The inner algorithm must match the outer one.
        if fields.take(SEQUENCE)? != signature_algorithm {
            return Err(Rejected::Malformed("signature algorithm mismatch"));
        }
        let issuer = fields.next()?;
        issuer.expect(SEQUENCE)?;
        let mut validity = Reader::new(fields.take(SEQUENCE)?);
        let not_before_ms = time(validity.next()?)?;
        let not_after_ms = time(validity.next()?)?;
        validity.finish()?;
        let subject = fields.next()?;
        subject.expect(SEQUENCE)?;
        let spki_tlv = fields.next()?;
        let mut spki = Reader::new(spki_tlv.expect(SEQUENCE)?);
        let mut algorithm = Reader::new(spki.take(SEQUENCE)?);
        let key_oid = algorithm.take(OID)?;
        let key_kind = match key_oid {
            oids::EC_PUBLIC_KEY => match algorithm.take(OID)? {
                oids::P256 => Some(KeyKind::P256),
                oids::P384 => Some(KeyKind::P384),
                _ => None,
            },
            oids::RSA => Some(KeyKind::Rsa),
            // Other key types (e.g. ML-DSA leaves) parse but cannot sign here.
            _ => None,
        };
        let key = match spki.take(BIT_STRING)? {
            [0, rest @ ..] => rest,
            _ => return Err(Rejected::Malformed("bad key bits")),
        };
        spki.finish()?;
        let mut extensions = Vec::new();
        while let Some(tlv) = fields.peek() {
            if tlv.is_context(1) || tlv.is_context(2) {
                // Unique identifiers: unused in attestation certificates.
                return Err(Rejected::Malformed("unexpected unique identifier"));
            }
            if !tlv.is_context(3) {
                break;
            }
            fields.next()?;
            let mut list = Reader::new(single(tlv.value, SEQUENCE)?);
            while !list.is_empty() {
                let mut ext = Reader::new(list.take(SEQUENCE)?);
                let oid = ext.take(OID)?;
                let critical = match ext.peek() {
                    Some(tlv) if tlv.is(BOOLEAN) => boolean(ext.take(BOOLEAN)?)?,
                    _ => false,
                };
                let value = ext.take(OCTET_STRING)?;
                ext.finish()?;
                if extensions.iter().any(|e: &Extension<'_>| e.oid == oid) {
                    return Err(Rejected::Malformed("duplicate extension"));
                }
                extensions.push(Extension {
                    oid,
                    critical,
                    value,
                });
            }
        }
        fields.finish()?;
        Ok(Self {
            tbs,
            signature_kind,
            signature,
            serial,
            issuer: issuer.raw,
            subject: subject.raw,
            not_before_ms,
            not_after_ms,
            spki: spki_tlv.raw,
            key_kind,
            key,
            extensions,
        })
    }

    pub fn extension(&self, oid: &[u8]) -> Option<&Extension<'a>> {
        self.extensions.iter().find(|e| e.oid == oid)
    }

    /// Whether this certificate may issue others: basicConstraints cA is
    /// true and, if keyUsage is present, keyCertSign is set.
    pub fn is_ca(&self) -> Result<bool, Rejected> {
        let Some(bc) = self.extension(oids::BASIC_CONSTRAINTS) else {
            return Ok(false);
        };
        let mut fields = Reader::new(single(bc.value, SEQUENCE)?);
        let ca = match fields.peek() {
            Some(tlv) if tlv.is(BOOLEAN) => boolean(fields.take(BOOLEAN)?)?,
            _ => false,
        };
        if !ca {
            return Ok(false);
        }
        if let Some(ku) = self.extension(oids::KEY_USAGE) {
            let bits = single(ku.value, BIT_STRING)?;
            // Bit 5 (keyCertSign) is 0x04 in the first content octet.
            if bits.len() < 2 || bits[1] & 0x04 == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Check `self`'s signature with `issuer`'s key.
    pub fn signed_by(&self, issuer: &Certificate<'_>) -> Result<(), Rejected> {
        use ring::signature as sig;
        let algorithm: &dyn sig::VerificationAlgorithm =
            match (issuer.key_kind, self.signature_kind) {
                (Some(KeyKind::P256), SignatureKind::EcdsaSha256) => &sig::ECDSA_P256_SHA256_ASN1,
                (Some(KeyKind::P256), SignatureKind::EcdsaSha384) => &sig::ECDSA_P256_SHA384_ASN1,
                (Some(KeyKind::P384), SignatureKind::EcdsaSha256) => &sig::ECDSA_P384_SHA256_ASN1,
                (Some(KeyKind::P384), SignatureKind::EcdsaSha384) => &sig::ECDSA_P384_SHA384_ASN1,
                (Some(KeyKind::Rsa), SignatureKind::RsaSha256) => &sig::RSA_PKCS1_2048_8192_SHA256,
                (Some(KeyKind::Rsa), SignatureKind::RsaSha384) => &sig::RSA_PKCS1_2048_8192_SHA384,
                (Some(KeyKind::Rsa), SignatureKind::RsaSha512) => &sig::RSA_PKCS1_2048_8192_SHA512,
                _ => return Err(Rejected::Unsupported("issuer key and signature algorithm")),
            };
        sig::UnparsedPublicKey::new(algorithm, issuer.key)
            .verify(self.tbs, self.signature)
            .map_err(|_| Rejected::Chain("a certificate signature does not verify"))
    }
}

/// Extensions a verifier understands well enough to accept when critical.
const KNOWN_CRITICAL: [&[u8]; 4] = [
    oids::BASIC_CONSTRAINTS,
    oids::KEY_USAGE,
    oids::EXTENDED_KEY_USAGE,
    oids::CERTIFICATE_POLICIES,
];

/// Most certificates accepted in one chain.
pub const MAX_CHAIN: usize = 8;
/// Largest certificate accepted, in bytes.
pub const MAX_CERT_BYTES: usize = 16 * 1024;
/// Clock skew allowed against certificate validity.
pub const SKEW_MS: i64 = 5 * 60_000;

/// Verify a leaf-first chain whose last certificate is a root carrying one
/// of the pinned `anchors` (SubjectPublicKeyInfo DER) and signing itself.
///
/// Every certificate below the root must be within its validity period at
/// `now_ms`, name its issuer, and be signed by it; issuers must be CAs. The
/// root is trusted by its key, as both Apple and Google specify, so its own
/// validity dates are not checked.
pub fn verify_chain<'a>(
    certs: &'a [Vec<u8>],
    anchors: &[Vec<u8>],
    now_ms: i64,
) -> Result<Vec<Certificate<'a>>, Rejected> {
    if certs.len() < 2 || certs.len() > MAX_CHAIN {
        return Err(Rejected::Chain("certificate chain length"));
    }
    if certs.iter().any(|c| c.len() > MAX_CERT_BYTES) {
        return Err(Rejected::Chain("certificate too large"));
    }
    let parsed = certs
        .iter()
        .map(|der| Certificate::parse(der))
        .collect::<Result<Vec<_>, _>>()?;
    for cert in &parsed {
        if cert
            .extensions
            .iter()
            .any(|e| e.critical && !KNOWN_CRITICAL.contains(&e.oid))
        {
            return Err(Rejected::Chain("unknown critical extension"));
        }
    }
    let root = parsed.last().expect("non-empty");
    if !anchors.iter().any(|a| a.as_slice() == root.spki) || root.issuer != root.subject {
        return Err(Rejected::UntrustedRoot);
    }
    root.signed_by(root)?;
    for pair in parsed.windows(2) {
        let (cert, issuer) = (&pair[0], &pair[1]);
        if now_ms + SKEW_MS < cert.not_before_ms || now_ms - SKEW_MS > cert.not_after_ms {
            return Err(Rejected::Expired);
        }
        if cert.issuer != issuer.subject {
            return Err(Rejected::Chain("issuer name mismatch"));
        }
        if !issuer.is_ca()? {
            return Err(Rejected::Chain("issuer is not a CA"));
        }
        cert.signed_by(issuer)?;
    }
    Ok(parsed)
}

/// The SubjectPublicKeyInfo of a DER certificate, for pinning anchors.
pub fn spki_of(der: &[u8]) -> Result<Vec<u8>, Rejected> {
    Ok(Certificate::parse(der)?.spki.to_vec())
}

/// Decode a PEM certificate (one block) to DER.
pub fn pem(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let body: String = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_and_tags_must_be_minimal_and_definite() {
        // Indefinite length.
        assert!(Reader::new(&[0x30, 0x80, 0, 0]).next().is_err());
        // Long form for a short length.
        assert!(Reader::new(&[0x04, 0x81, 0x01, 0xaa]).next().is_err());
        // Truncated.
        assert!(Reader::new(&[0x04, 0x05, 0xaa]).next().is_err());
        // High tag number written in high form but < 31.
        assert!(Reader::new(&[0xbf, 0x1e, 0x00]).next().is_err());
        // [709] = 0xbf 0x85 0x45.
        let tlv = Reader::new(&[0xbf, 0x85, 0x45, 0x02, 0x05, 0x00])
            .next()
            .unwrap();
        assert!(tlv.is_context(709));
        // Integers: no redundant leading zero, no negatives.
        assert!(uint(&[0x00, 0x01]).is_err());
        assert!(uint(&[0x80]).is_err());
        assert_eq!(uint(&[0x00, 0x80]).unwrap(), 128);
        assert!(boolean(&[0x01]).is_err());
    }

    #[test]
    fn times_convert_to_unix_milliseconds() {
        let utc = |s: &str| {
            let mut bytes = vec![UTC_TIME, s.len() as u8];
            bytes.extend_from_slice(s.as_bytes());
            time(Reader::new(&bytes).next().unwrap())
        };
        assert_eq!(utc("700101000000Z").unwrap(), 0);
        assert_eq!(utc("491231235959Z").unwrap(), 2_524_607_999_000);
        assert!(utc("701301000000Z").is_err());
        assert!(utc("700101000000+").is_err());
        // Non-ASCII bytes are refused, never sliced through.
        assert!(utc("1\u{e9}010100000Z").is_err());
        assert!(utc("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}Z").is_err());
        let mut g = vec![GENERALIZED_TIME, 15];
        g.extend_from_slice(b"20480101000000Z");
        assert_eq!(
            time(Reader::new(&g).next().unwrap()).unwrap(),
            2_461_449_600_000
        );
    }
}
