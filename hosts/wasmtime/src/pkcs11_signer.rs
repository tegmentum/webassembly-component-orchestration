//! PKCS#11-backed attestation signer.
//!
//! Implements [`compose_core::host::Signer`] by driving the composed
//! `keys:keystore` component (keystore-pkcs11 adapter + pkcs11-provider +
//! softhsm, all wasm) via wasmtime. The orchestrator's attestation key
//! lives inside a software HSM in the sandbox; the private key never
//! crosses the component boundary. This is the production-shaped
//! alternative to [`compose_core::host::SoftwareSigner`].
//!
//! The whole PKCS#11 ceremony (token init, login, key generation,
//! C_Sign) is hidden behind `keys:keystore/signer`, so this host code
//! just resolves the key once at construction and calls `sign` per
//! attestation.
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use compose_core::host::{SharedSigner, Signer, SignerError, ALG_ED25519};
use wasmtime::component::{Component, Linker, ResourceAny, ResourceTable};
use wasmtime::{Engine, Store};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    path: "wit/keystore",
    world: "keystore-signer",
});

use exports::keys::keystore::signer::Error as KsError;

/// Configuration for the PKCS#11 attestation signer.
#[derive(Debug, Clone)]
pub struct Pkcs11SignerConfig {
    /// Path to the composed `keys:keystore` component
    /// (e.g. `keystore-softhsm.wasm`).
    pub component_path: PathBuf,
    /// Path to the SoftHSM2 config file to expose to the component
    /// (its `directories.tokendir` must be `/data/tokens`).
    pub conf_path: PathBuf,
    /// Host directory used for token storage; mapped to `/data` in the
    /// sandbox so keys persist across runs.
    pub token_dir: PathBuf,
    /// Keystore label of the attestation key (created on first use).
    pub key_label: String,
    /// User PIN for the token.
    pub pin: String,
    /// Security-officer PIN (used only when provisioning a fresh token).
    pub so_pin: String,
}

impl Default for Pkcs11SignerConfig {
    fn default() -> Self {
        Self {
            component_path: PathBuf::from("keystore-softhsm.wasm"),
            conf_path: PathBuf::from("softhsm2-wasi.conf"),
            token_dir: PathBuf::from(".compose/pkcs11/data"),
            key_label: "attest".to_string(),
            pin: "1234".to_string(),
            so_pin: "1234".to_string(),
        }
    }
}

/// Host state for the keystore component instance.
struct KsState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
}

impl WasiView for KsState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

// The component imports a pkcs11:util pin-provider (referenced by the
// credential type). The attestation flow only ever passes inline PINs, so
// these are never invoked — they exist solely to satisfy the import.
use pkcs11::util::util::PinProvider;
impl pkcs11::util::util::Host for KsState {}
impl pkcs11::util::util::HostPinProvider for KsState {
    fn request_secret(
        &mut self,
        _self_: wasmtime::component::Resource<PinProvider>,
        _label: Option<String>,
        _attempts_remaining: Option<u8>,
    ) -> Vec<u8> {
        Vec::new()
    }
    fn clear(&mut self, _self_: wasmtime::component::Resource<PinProvider>) {}
    fn drop(&mut self, _rep: wasmtime::component::Resource<PinProvider>) -> wasmtime::Result<()> {
        Ok(())
    }
}

struct HasSelf<T>(std::marker::PhantomData<T>);
impl<T: 'static> wasmtime::component::HasData for HasSelf<T> {
    type Data<'a> = &'a mut T;
}

struct Inner {
    store: Store<KsState>,
    bindings: KeystoreSigner,
    key: ResourceAny,
}

/// A [`Signer`] whose private key lives in a software HSM inside the wasm
/// sandbox, accessed through the `keys:keystore` interface.
pub struct Pkcs11Signer {
    inner: Mutex<Inner>,
    algorithm: String,
    public_key: Vec<u8>,
}

impl Pkcs11Signer {
    /// Instantiate the keystore component, resolve the attestation key,
    /// and cache its algorithm and public key.
    pub fn open(engine: &Engine, config: &Pkcs11SignerConfig) -> Result<Self> {
        let component = Component::from_file(engine, &config.component_path).map_err(|e| {
            anyhow!(
                "loading keystore component {}: {e}",
                config.component_path.display()
            )
        })?;

        // Sandbox filesystem: /config/softhsm2-wasi.conf + /data/tokens.
        let config_dir = config.token_dir.join("config");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(config.token_dir.join("tokens"))?;
        let conf = std::fs::read(&config.conf_path)
            .with_context(|| format!("reading softhsm conf {}", config.conf_path.display()))?;
        std::fs::write(config_dir.join("softhsm2-wasi.conf"), conf)?;

        let mut wasi = WasiCtxBuilder::new();
        wasi.env("SOFTHSM2_CONF", "/config/softhsm2-wasi.conf")
            .env("KEYSTORE_PIN", &config.pin)
            .env("KEYSTORE_SO_PIN", &config.so_pin)
            .preopened_dir(&config_dir, "/config", DirPerms::READ, FilePerms::READ)?
            .preopened_dir(&config.token_dir, "/data", DirPerms::all(), FilePerms::all())?;

        let mut linker: Linker<KsState> = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| anyhow!("add wasi to keystore linker: {e}"))?;
        pkcs11::util::util::add_to_linker::<KsState, HasSelf<KsState>>(&mut linker, |s| s)
            .map_err(|e| anyhow!("add pkcs11:util to keystore linker: {e}"))?;

        let state = KsState {
            wasi_ctx: wasi.build(),
            wasi_table: ResourceTable::new(),
        };
        let mut store = Store::new(engine, state);
        let bindings = KeystoreSigner::instantiate(&mut store, &component, &linker)
            .map_err(|e| anyhow!("instantiate keystore component: {e}"))?;

        let signer = bindings.keys_keystore_signer();
        let key = signer
            .call_get_key(&mut store, &config.key_label)
            .map_err(|e| anyhow!("trap during keystore get-key: {e}"))?
            .map_err(|e: KsError| anyhow!("keystore get-key failed: {e:?}"))?;

        let algorithm = signer
            .key()
            .call_algorithm(&mut store, key)
            .map_err(|e| anyhow!("trap during key.algorithm: {e}"))?;
        let public_key = signer
            .key()
            .call_public_key(&mut store, key)
            .map_err(|e| anyhow!("trap during key.public-key: {e}"))?;

        Ok(Self {
            inner: Mutex::new(Inner {
                store,
                bindings,
                key,
            }),
            algorithm,
            public_key,
        })
    }

    /// Convenience: build a [`SharedSigner`].
    pub fn shared(engine: &Engine, config: &Pkcs11SignerConfig) -> Result<SharedSigner> {
        Ok(std::sync::Arc::new(Self::open(engine, config)?))
    }
}

impl Signer for Pkcs11Signer {
    fn algorithm(&self) -> &str {
        &self.algorithm
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SignerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| SignerError::Backend("keystore signer mutex poisoned".to_string()))?;
        let Inner {
            store,
            bindings,
            key,
        } = &mut *inner;
        bindings
            .keys_keystore_signer()
            .key()
            .call_sign(store, *key, message)
            .map_err(|t| SignerError::Backend(format!("keystore sign trap: {t}")))?
            .map_err(|e: KsError| SignerError::Backend(format!("keystore sign failed: {e:?}")))
    }

    fn public_key(&self) -> Vec<u8> {
        self.public_key.clone()
    }
}

/// Sanity helper: the keystore signer must produce ed25519 signatures for
/// the attestation pipeline to verify them.
pub fn is_ed25519(signer: &Pkcs11Signer) -> bool {
    signer.algorithm == ALG_ED25519
}
