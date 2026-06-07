# SigStore test fixtures

`cosign-v3-blob.sigstore.json` + `cosign-v3-blob.txt` are a real cosign-produced
Sigstore bundle (v0.3, message-signature over a blob) and the blob it signs.

Vendored from the `sigstore-verify` crate's test data (`test_data/bundles/`),
which sourced it from cosign v3.x. Apache-2.0. Used to exercise full offline
bundle verification (signature + Fulcio cert chain + Rekor inclusion proof + SCT)
against the embedded Sigstore production trust root. The bundle verifies via the
transparency-log integrated time, so it stays valid regardless of the wall clock.
