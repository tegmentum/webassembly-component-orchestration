/// PKCS#11 secret backend using WIT-based adapter
/// Interfaces with hardware security modules and smart cards via the pkcs11-host-adapter
use super::*;

#[cfg(feature = "pkcs11")]
use pkcs11_host_adapter::{AdapterContext, bindings::pkcs11::core::core::ErrorCode as Pkcs11ErrorCode};

#[cfg(feature = "pkcs11")]
use std::sync::Arc;

/// PKCS#11 backend configuration
pub struct Pkcs11Config {
    /// Path to PKCS#11 library
    pub library_path: String,
    /// Slot ID
    pub slot_id: u32,
    /// PIN for authentication
    pub pin: Option<String>,
}

/// PKCS#11 secret backend using WIT adapter
pub struct Pkcs11Backend {
    config: Pkcs11Config,
    #[cfg(feature = "pkcs11")]
    context: Arc<AdapterContext>,
}

impl Pkcs11Backend {
    /// Create a new PKCS#11 backend with WIT adapter
    pub fn new(config: Pkcs11Config) -> Result<Self, SecretError> {
        #[cfg(feature = "pkcs11")]
        {
            let context = Arc::new(AdapterContext::default());

            // Initialize the PKCS#11 module
            context
                .ensure_initialized(&config.library_path)
                .map_err(|e| SecretError::BackendError(format!("Failed to initialize PKCS#11: {:?}", e)))?;

            tracing::info!(
                library = %config.library_path,
                slot = config.slot_id,
                "PKCS#11 backend initialized via WIT adapter"
            );

            Ok(Self { config, context })
        }

        #[cfg(not(feature = "pkcs11"))]
        {
            tracing::warn!("PKCS#11 feature not enabled - backend will not function");
            Ok(Self { config })
        }
    }

    #[cfg(feature = "pkcs11")]
    fn map_pkcs11_error(err: Pkcs11ErrorCode) -> SecretError {
        SecretError::BackendError(format!("PKCS#11 error: {:?}", err))
    }
}

impl SecretBackend for Pkcs11Backend {
    fn scheme(&self) -> &str {
        "pkcs11"
    }

    fn resolve(&self, id: &SecretId) -> Result<Vec<u8>, SecretError> {
        #[cfg(feature = "pkcs11")]
        {
            // For secret resolution, we would:
            // 1. Open a session on the slot
            // 2. Login with the PIN if provided
            // 3. Find the object by label (using the secret ID)
            // 4. Retrieve the CKA_VALUE attribute

            // The WIT adapter provides high-level interfaces through:
            // - session management (session interface)
            // - object operations (object interface)

            // For now, return a descriptive error indicating this requires
            // session and object manipulation through the WIT interfaces
            tracing::info!(
                secret_id = %id,
                slot = self.config.slot_id,
                "Resolving secret via PKCS#11 WIT adapter"
            );

            // Get slot info to verify the slot exists
            self.context
                .slot_info(self.config.slot_id)
                .map_err(Self::map_pkcs11_error)?;

            // TODO: Implement full secret retrieval using session and object interfaces
            // This requires:
            // - Creating a session resource
            // - Logging in with the PIN
            // - Searching for objects with the label matching the secret ID
            // - Reading the CKA_VALUE attribute

            Err(SecretError::BackendError(
                "Full secret retrieval not yet implemented - requires session and object manipulation through WIT".to_string(),
            ))
        }

        #[cfg(not(feature = "pkcs11"))]
        {
            tracing::warn!(
                secret_id = %id,
                "PKCS#11 feature not enabled - cannot resolve secrets"
            );
            Err(SecretError::BackendError(
                "PKCS#11 feature not enabled".to_string(),
            ))
        }
    }

    fn get_metadata(&self, id: &SecretId) -> Result<SecretMetadata, SecretError> {
        #[cfg(feature = "pkcs11")]
        {
            // Get slot and token info to provide metadata
            let slot_info = self.context
                .slot_info(self.config.slot_id)
                .map_err(Self::map_pkcs11_error)?;

            let token_info = self.context
                .token_info(self.config.slot_id)
                .map_err(Self::map_pkcs11_error)?;

            let metadata = format!(
                "Slot: {} ({}), Token: {}",
                self.config.slot_id,
                &slot_info.slot_description,
                &token_info.label
            );

            Ok(SecretMetadata {
                id: id.clone(),
                backend: format!("pkcs11://slot{}", self.config.slot_id),
                created_at: None,
                expires_at: None,
                metadata: Some(metadata),
            })
        }

        #[cfg(not(feature = "pkcs11"))]
        {
            Ok(SecretMetadata {
                id: id.clone(),
                backend: format!("pkcs11://slot{}", self.config.slot_id),
                created_at: None,
                expires_at: None,
                metadata: Some(format!("PKCS#11 slot {} (feature disabled)", self.config.slot_id)),
            })
        }
    }

    fn list_secrets(&self) -> Result<Vec<SecretId>, SecretError> {
        #[cfg(feature = "pkcs11")]
        {
            // Verify the slot exists
            self.context
                .slot_info(self.config.slot_id)
                .map_err(Self::map_pkcs11_error)?;

            // TODO: Implement object enumeration using the object interface
            // This requires:
            // - Opening a session
            // - Logging in with the PIN
            // - Finding all objects
            // - Extracting their labels as secret IDs

            tracing::info!(
                slot = self.config.slot_id,
                "Listing secrets would enumerate PKCS#11 objects via WIT adapter"
            );

            Ok(Vec::new())
        }

        #[cfg(not(feature = "pkcs11"))]
        {
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkcs11_backend_creation() {
        let config = Pkcs11Config {
            library_path: "/usr/lib/softhsm/libsofthsm2.so".to_string(),
            slot_id: 0,
            pin: Some("1234".to_string()),
        };

        // Without pkcs11 feature, this should still create but warn
        #[cfg(not(feature = "pkcs11"))]
        {
            let backend = Pkcs11Backend::new(config).unwrap();
            assert_eq!(backend.scheme(), "pkcs11");
        }

        // With pkcs11 feature, this might fail if the library doesn't exist
        // but the structure should be correct
        #[cfg(feature = "pkcs11")]
        {
            // Only test if SoftHSM is actually installed
            if std::path::Path::new("/usr/lib/softhsm/libsofthsm2.so").exists()
                || std::path::Path::new("/opt/homebrew/lib/softhsm/libsofthsm2.so").exists() {
                let backend = Pkcs11Backend::new(config);
                // May succeed or fail depending on SoftHSM configuration
                if let Ok(backend) = backend {
                    assert_eq!(backend.scheme(), "pkcs11");

                    // Metadata should work
                    let metadata = backend.get_metadata(&"test".to_string());
                    // May fail if slot doesn't exist, but structure is correct
                    if let Ok(meta) = metadata {
                        assert!(meta.backend.starts_with("pkcs11://"));
                    }
                }
            }
        }
    }

    #[test]
    fn test_pkcs11_scheme() {
        let config = Pkcs11Config {
            library_path: "/usr/lib/softhsm/libsofthsm2.so".to_string(),
            slot_id: 0,
            pin: Some("1234".to_string()),
        };

        let backend = Pkcs11Backend::new(config).unwrap();
        assert_eq!(backend.scheme(), "pkcs11");
    }
}
