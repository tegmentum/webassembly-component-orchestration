//! Integration test for the PKCS#11-backed attestation signer.
//!
//! Drives a real composed `keys:keystore` (softhsm) component through
//! [`Pkcs11Signer`] and verifies the produced ed25519 signature with the
//! orchestrator's own `verify_ed25519`. The composed artifact lives in the
//! sibling softhsm-wasm repo, so the test is opt-in: it runs only when
//! `KEYSTORE_TEST_COMPONENT` (and `KEYSTORE_TEST_CONF`) point at the built
//! `keystore-softhsm.wasm` and its SoftHSM config; otherwise it skips.
use compose_core::host::{verify_ed25519, Signer};
use compose_host_wasmtime::attest::{Algorithm, Claim};
use compose_host_wasmtime::{CompositorHost, HostConfig, Pkcs11Signer, Pkcs11SignerConfig};
use wasmtime::{Config, Engine};

/// Resolve the opt-in artifact paths, or `None` to skip.
fn test_paths() -> Option<(String, String)> {
    let component = std::env::var("KEYSTORE_TEST_COMPONENT").ok()?;
    let conf = std::env::var("KEYSTORE_TEST_CONF")
        .expect("KEYSTORE_TEST_CONF must be set with KEYSTORE_TEST_COMPONENT");
    Some((component, conf))
}

#[test]
fn pkcs11_signer_signs_and_verifies() {
    let Ok(component_path) = std::env::var("KEYSTORE_TEST_COMPONENT") else {
        eprintln!(
            "skipping pkcs11_signer test: set KEYSTORE_TEST_COMPONENT to the \
             composed keystore-softhsm.wasm path (and KEYSTORE_TEST_CONF)"
        );
        return;
    };
    let conf_path = std::env::var("KEYSTORE_TEST_CONF")
        .expect("KEYSTORE_TEST_CONF must be set with the component");
    let token_dir = std::env::temp_dir().join(format!("pkcs11-signer-test-{}", std::process::id()));

    let mut cfg = Config::new();
    cfg.wasm_component_model(true);
    let engine = Engine::new(&cfg).expect("engine");

    let config = Pkcs11SignerConfig {
        component_path: component_path.into(),
        conf_path: conf_path.into(),
        token_dir,
        key_label: "attest".to_string(),
        pin: "1234".to_string(),
        so_pin: "1234".to_string(),
    };

    let signer = Pkcs11Signer::open(&engine, &config).expect("open pkcs11 attestation signer");
    assert_eq!(signer.algorithm(), "ed25519");

    let public_key = signer.public_key();
    assert_eq!(public_key.len(), 32, "ed25519 public key is 32 bytes");

    let message = b"orchestrator attestation payload";
    let signature = signer.sign(message).expect("sign");
    assert_eq!(signature.len(), 64, "ed25519 signature is 64 bytes");

    assert!(
        verify_ed25519(&public_key, message, &signature).expect("verify"),
        "signature from the PKCS#11/softhsm keystore must verify"
    );

    // A second signature over different data should also verify (the
    // session/key handle stays valid across calls).
    let message2 = b"a different attestation";
    let signature2 = signer.sign(message2).expect("sign again");
    assert!(verify_ed25519(&public_key, message2, &signature2).expect("verify2"));
}

/// Boot a full `CompositorHost` configured to attest with the PKCS#11
/// signer, produce an attestation through its `AttestationService`, and
/// verify it — proving the orchestrator's attestation pipeline is signed
/// by a key inside the softhsm component.
#[test]
fn compositor_host_attests_with_pkcs11_signer() {
    let Some((component_path, conf_path)) = test_paths() else {
        eprintln!(
            "skipping compositor_host_attests_with_pkcs11_signer: set KEYSTORE_TEST_COMPONENT/CONF"
        );
        return;
    };
    let work = tempfile::tempdir().expect("tempdir");
    let root = work.path();

    let config = HostConfig {
        blob_dir: root.join("blobs"),
        cache_dir: root.join("cache"),
        trust_dir: root.join("trust"),
        audit_dir: root.join("audit"),
        max_blob_size: 100 * 1024 * 1024,
        attest_pkcs11: Some(Pkcs11SignerConfig {
            component_path: component_path.into(),
            conf_path: conf_path.into(),
            token_dir: root.join("pkcs11"),
            key_label: "attest".to_string(),
            pin: "1234".to_string(),
            so_pin: "1234".to_string(),
        }),
    };

    let host = CompositorHost::new(config).expect("boot CompositorHost with PKCS#11 signer");

    let claim = Claim {
        claim_type: "execution".to_string(),
        plan_digest: vec![0xaa; 32],
        artifact_digest: vec![0xbb; 32],
        exec_key: None,
        timestamp: 1,
        host_id: host.attestation.host_id().to_string(),
        custom_claims: None,
    };

    let attestation = host
        .attestation
        .attest(claim, Algorithm::Ed25519)
        .expect("attest via PKCS#11 signer");
    assert_eq!(
        attestation.public_key.len(),
        32,
        "softhsm ed25519 public key"
    );

    let result = host
        .attestation
        .verify(&attestation)
        .expect("verify attestation");
    assert!(
        result.valid,
        "attestation signed by the softhsm key must verify"
    );
}
