# Attestation test vectors

Genuine platform data, used only to check that the verifiers parse and
validate real formats and signatures. None of it belongs to this project's
owner or devices, and none of it is accepted as enrolment evidence: each
was made for someone else's challenge and app.

- `apple_app_attest_sample.b64`: the sample attestation object from Apple's
  "Attestation Object Validation Guide" (DeviceCheck documentation), for the
  example App ID `1234567890.com.example.myapp` and challenge
  `example_server_challenge`. Its certificates were valid from 2026-04-20 to
  2026-04-23.
- `android_*.pem`: key attestation chains from Pixel devices, copied from
  the `testdata` directory of <https://github.com/android/keyattestation>
  (Apache License 2.0, Copyright Google LLC). Leaf first.
  - `caiman_sdk36_TEE_EC_RKP`, `caiman_sdk36_SB_EC_RKP`: TEE and StrongBox
    keys under the RSA root (RKP intermediates valid 2025-09-24 to
    2025-10-03).
  - `tegu_sdk36_TEE_EC_2026_ROOT`: a TEE key under the ECDSA P-384 root
    (valid 2026-02-22 to 2026-03-08).
  - `invalid_*`: chains Google marks invalid (authorization tags out of
    order; malformed root of trust).

The roots in `../roots/` are Google's current attestation roots from
<https://android.googleapis.com/attestation/root> and the Apple App
Attestation Root CA from <https://www.apple.com/certificateauthority/private/>.
