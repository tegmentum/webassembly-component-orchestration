//! Offline Sigstore bundle verification tests against a real cosign-produced
//! bundle (see tests/fixtures/README.md). The bundle verifies via the
//! transparency-log integrated time, so these pass regardless of wall clock.
use compose_core::blobs::compute_digest;
use compose_core::host::SystemClock;
use compose_core::trust::TrustBackend;
use compose_core::types::ErrorCode;
use trust_backends::{SigStoreTrustBackend, SigstoreIdentity};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

#[test]
fn verifies_a_real_sigstore_bundle() {
    let artifact = fixture("cosign-v3-blob.txt");
    let bundle = fixture("cosign-v3-blob.sigstore.json");

    let backend = SigStoreTrustBackend::new(SystemClock::shared());
    let meta = backend
        .verify(&compute_digest(&artifact), &artifact, &bundle)
        .expect("a valid sigstore bundle must verify");
    assert_eq!(meta.backend, "sigstore");
    // Fulcio short-lived certs carry an identity; the transparency-log entry
    // gives an integrated timestamp.
    assert!(!meta.signer.is_empty(), "signer should be populated");
    assert!(meta.timestamp.is_some(), "integrated time should be set");
}

#[test]
fn rejects_when_required_identity_does_not_match() {
    let artifact = fixture("cosign-v3-blob.txt");
    let bundle = fixture("cosign-v3-blob.sigstore.json");

    let backend = SigStoreTrustBackend::production(
        vec![SigstoreIdentity {
            identity: "nobody@example.com".to_string(),
            issuer: "https://example.com/oauth".to_string(),
        }],
        SystemClock::shared(),
    )
    .expect("backend");
    let err = backend
        .verify(&compute_digest(&artifact), &artifact, &bundle)
        .expect_err("a non-matching identity policy must reject");
    assert!(matches!(err.code, ErrorCode::TrustSignatureInvalid));
}

#[test]
fn rejects_a_tampered_artifact() {
    let bundle = fixture("cosign-v3-blob.sigstore.json");
    let tampered = b"test content for cosign (tampered)".to_vec();

    let backend = SigStoreTrustBackend::new(SystemClock::shared());
    // The digest precondition passes (we hash the tampered bytes), so this
    // exercises the real signature check: the bundle signs the original blob.
    let err = backend
        .verify(&compute_digest(&tampered), &tampered, &bundle)
        .expect_err("a tampered artifact must reject");
    assert!(matches!(
        err.code,
        ErrorCode::TrustSignatureInvalid | ErrorCode::TrustVerificationFailed
    ));
}

#[test]
fn rejects_a_malformed_bundle() {
    let artifact = fixture("cosign-v3-blob.txt");
    let backend = SigStoreTrustBackend::new(SystemClock::shared());
    let err = backend
        .verify(&compute_digest(&artifact), &artifact, b"not a bundle")
        .expect_err("a malformed bundle must reject");
    assert!(matches!(err.code, ErrorCode::TrustSignatureInvalid));
}

#[test]
fn rejects_an_oversized_bundle() {
    let artifact = fixture("cosign-v3-blob.txt");
    let backend = SigStoreTrustBackend::new(SystemClock::shared());
    // > 1 MiB of JSON-looking bytes: rejected before parsing.
    let huge = vec![b'{'; (1 << 20) + 1];
    let err = backend
        .verify(&compute_digest(&artifact), &artifact, &huge)
        .expect_err("an oversized bundle must reject");
    assert!(matches!(err.code, ErrorCode::TrustSignatureInvalid));
}

#[test]
fn rejects_message_signature_without_hashedrekord_entry() {
    // Defense in depth: a MessageSignature bundle whose tlog entry is not a
    // hashedrekord would skip the only signature check. Take the real bundle
    // and rename the entry kind so no hashedrekord entry remains.
    let artifact = fixture("cosign-v3-blob.txt");
    let bundle = String::from_utf8(fixture("cosign-v3-blob.sigstore.json")).unwrap();
    assert!(bundle.contains("hashedrekord"), "fixture sanity");
    let mutated = bundle.replace("hashedrekord", "hashedrekordx");

    let backend = SigStoreTrustBackend::new(SystemClock::shared());
    let err = backend
        .verify(&compute_digest(&artifact), &artifact, mutated.as_bytes())
        .expect_err("message-signature bundle without a hashedrekord entry must reject");
    assert!(matches!(err.code, ErrorCode::TrustSignatureInvalid));
}
