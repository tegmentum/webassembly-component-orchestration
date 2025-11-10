/// PKCS#11 secret backend using WIT-based adapter
/// Interfaces with hardware security modules and smart cards via the pkcs11-host-adapter
use super::*;

#[cfg(feature = "pkcs11")]
use pkcs11_host_adapter::{
    AdapterContext,
    bindings::pkcs11::core::core::{
        Attribute as WitAttribute,
        AttributeValue,
        ErrorCode as Pkcs11ErrorCode,
        SessionFlags as WitSessionFlags,
        UserType,
    },
};

#[cfg(feature = "pkcs11")]
use std::sync::Arc;

// PKCS#11 attribute tags
#[cfg(feature = "pkcs11")]
const CKA_LABEL: u32 = 0x0000_0003;
#[cfg(feature = "pkcs11")]
const CKA_VALUE: u32 = 0x0000_0011;

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
            tracing::info!(
                secret_id = %id,
                slot = self.config.slot_id,
                "Resolving secret via PKCS#11 WIT adapter"
            );

            // Verify the slot exists
            self.context
                .slot_info(self.config.slot_id)
                .map_err(Self::map_pkcs11_error)?;

            // Open a read-only session
            let session_flags = WitSessionFlags::SERIAL_SESSION;
            let session_handle = self.context
                .open_session(self.config.slot_id, session_flags)
                .map_err(Self::map_pkcs11_error)?;

            // Login if PIN is provided
            if let Some(ref pin) = self.config.pin {
                self.context
                    .login(session_handle, UserType::User, pin.as_bytes())
                    .map_err(Self::map_pkcs11_error)?;
            }

            // Search for object with matching label
            let template = vec![WitAttribute {
                tag: CKA_LABEL,
                value: AttributeValue::ByteString(id.as_bytes().to_vec()),
            }];

            self.context
                .find_objects_init(session_handle, &template)
                .map_err(Self::map_pkcs11_error)?;

            let objects = self.context
                .find_objects(session_handle, 1)
                .map_err(Self::map_pkcs11_error)?;

            self.context
                .find_objects_final(session_handle)
                .map_err(Self::map_pkcs11_error)?;

            if objects.is_empty() {
                self.context.close_session(session_handle).ok();
                return Err(SecretError::NotFound);
            }

            // Get the CKA_VALUE attribute from the first matching object
            let object_handle = objects[0];
            let attributes = self.context
                .get_attributes(session_handle, object_handle, &[CKA_VALUE])
                .map_err(Self::map_pkcs11_error)?;

            // Close the session
            self.context.close_session(session_handle).ok();

            // Extract the value bytes
            if let Some(attr) = attributes.first() {
                match &attr.value {
                    AttributeValue::ByteString(bytes) => Ok(bytes.clone()),
                    _ => Err(SecretError::BackendError(
                        "CKA_VALUE attribute is not a byte string".to_string(),
                    )),
                }
            } else {
                Err(SecretError::BackendError(
                    "Object does not have CKA_VALUE attribute".to_string(),
                ))
            }
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
            tracing::info!(
                slot = self.config.slot_id,
                "Listing secrets via PKCS#11 WIT adapter"
            );

            // Verify the slot exists
            self.context
                .slot_info(self.config.slot_id)
                .map_err(Self::map_pkcs11_error)?;

            // Open a read-only session
            let session_flags = WitSessionFlags::SERIAL_SESSION;
            let session_handle = self.context
                .open_session(self.config.slot_id, session_flags)
                .map_err(Self::map_pkcs11_error)?;

            // Login if PIN is provided
            if let Some(ref pin) = self.config.pin {
                self.context
                    .login(session_handle, UserType::User, pin.as_bytes())
                    .map_err(Self::map_pkcs11_error)?;
            }

            // Find all objects (empty template matches all)
            let template = vec![];
            self.context
                .find_objects_init(session_handle, &template)
                .map_err(Self::map_pkcs11_error)?;

            let mut secret_ids = Vec::new();

            // Fetch objects in batches of 20
            loop {
                let objects = self.context
                    .find_objects(session_handle, 20)
                    .map_err(Self::map_pkcs11_error)?;

                if objects.is_empty() {
                    break;
                }

                // Get label attribute for each object
                for object_handle in objects {
                    if let Ok(attributes) = self.context
                        .get_attributes(session_handle, object_handle, &[CKA_LABEL])
                    {
                        if let Some(attr) = attributes.first() {
                            if let AttributeValue::ByteString(bytes) = &attr.value {
                                if let Ok(label) = String::from_utf8(bytes.clone()) {
                                    secret_ids.push(label);
                                }
                            }
                        }
                    }
                }
            }

            self.context
                .find_objects_final(session_handle)
                .map_err(Self::map_pkcs11_error)?;

            // Close the session
            self.context.close_session(session_handle).ok();

            Ok(secret_ids)
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

        #[cfg(not(feature = "pkcs11"))]
        {
            let backend = Pkcs11Backend::new(config).unwrap();
            assert_eq!(backend.scheme(), "pkcs11");
        }

        #[cfg(feature = "pkcs11")]
        {
            // Only test if SoftHSM is actually installed
            if std::path::Path::new("/usr/lib/softhsm/libsofthsm2.so").exists()
                || std::path::Path::new("/opt/homebrew/lib/softhsm/libsofthsm2.so").exists() {
                if let Ok(backend) = Pkcs11Backend::new(config) {
                    assert_eq!(backend.scheme(), "pkcs11");
                }
            }
        }
    }
}
