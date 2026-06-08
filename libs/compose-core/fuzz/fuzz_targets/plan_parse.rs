#![no_main]
//! Fuzz the untrusted plan CBOR parser (`plan::deserialize`), the boundary
//! that ingests attacker-controlled bytes (conformance vectors, `composectl
//! plan validate <file>`, trust/exec inputs). Any byte string must either be
//! rejected cleanly or decode to a `PlanV1` whose canonical re-encoding is
//! stable — never panic, hang, or OOM.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(plan) = compose_core::plan::deserialize(data) {
        // A successfully parsed plan must re-serialize, and that canonical
        // encoding must round-trip identically (idempotent encode/decode).
        let bytes =
            compose_core::plan::serialize(&plan).expect("a parsed plan must re-serialize");
        let reparsed = compose_core::plan::deserialize(&bytes)
            .expect("a serialized plan must re-parse");
        let rebytes = compose_core::plan::serialize(&reparsed)
            .expect("re-serialize must succeed");
        assert_eq!(bytes, rebytes, "canonical plan encoding is not idempotent");
    }
});
