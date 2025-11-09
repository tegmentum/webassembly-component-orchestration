/// PKCS#11 secret backend
/// Interfaces with hardware security modules and smart cards
use super::*;

#[cfg(feature = "pkcs11")]
use cryptoki::{
    context::{CInitializeArgs, Pkcs11},
    object::Attribute,
    session::UserType as Pkcs11UserType,
    types::AuthPin,
};

#[cfg(feature = "pkcs11")]
use std::sync::Arc;

/// PKCS#11 backend configuration
pub struct Pkcs11Config {
    /// Path to PKCS#11 library
    pub library_path: String,
    /// Slot ID
    pub slot_id: u64,
    /// PIN for authentication
    pub pin: Option<String>,
}

/// PKCS#11 secret backend
pub struct Pkcs11Backend {
    config: Pkcs11Config,
    #[cfg(feature = "pkcs11")]
    pkcs11: Arc<Pkcs11>,
}

impl Pkcs11Backend {
    /// Create a new PKCS#11 backend
    pub fn new(config: Pkcs11Config) -> Result<Self, SecretError> {
        #[cfg(feature = "pkcs11")]
        {
            let pkcs11 = Pkcs11::new(&config.library_path)
                .map_err(|e| SecretError::BackendError(format!("Failed to load PKCS#11 library: {}", e)))?;

            pkcs11
                .initialize(CInitializeArgs::OsThreads)
                .map_err(|e| SecretError::BackendError(format!("Failed to initialize PKCS#11: {}", e)))?;

            Ok(Self {
                config,
                pkcs11: Arc::new(pkcs11),
            })
        }

        #[cfg(not(feature = "pkcs11"))]
        {
            tracing::warn!("PKCS#11 feature not enabled - backend will not function");
            Ok(Self { config })
        }
    }

    #[cfg(feature = "pkcs11")]
    fn get_session(&self) -> Result<cryptoki::session::Session, SecretError> {
        // Get all slots and find the one matching our slot ID
        let slots = self.pkcs11
            .get_all_slots()
            .map_err(|e| SecretError::BackendError(format!("Failed to get slots: {}", e)))?;

        let slot = slots
            .get(self.config.slot_id as usize)
            .ok_or_else(|| SecretError::BackendError(format!("Slot {} not found", self.config.slot_id)))?;

        let session = self.pkcs11
            .open_ro_session(*slot)
            .map_err(|e| SecretError::BackendError(format!("Failed to open session: {}", e)))?;

        if let Some(ref pin) = self.config.pin {
            let auth_pin = AuthPin::new(pin.clone());
            session
                .login(Pkcs11UserType::User, Some(&auth_pin))
                .map_err(|e| SecretError::BackendError(format!("Failed to login: {}", e)))?;
        }

        Ok(session)
    }
}

impl SecretBackend for Pkcs11Backend {
    fn scheme(&self) -> &str {
        "pkcs11"
    }

    fn resolve(&self, id: &SecretId) -> Result<Vec<u8>, SecretError> {
        #[cfg(feature = "pkcs11")]
        {
            let session = self.get_session()?;

            // Find object by label (using the secret ID as the label)
            let template = vec![Attribute::Label(id.as_bytes().to_vec())];

            let objects = session
                .find_objects(&template)
                .map_err(|e| SecretError::BackendError(format!("Failed to find object: {}", e)))?;

            if objects.is_empty() {
                return Err(SecretError::NotFound);
            }

            // Get the value attribute
            use cryptoki::object::AttributeType;
            let value_attrs = session
                .get_attributes(objects[0], &[AttributeType::Value])
                .map_err(|e| SecretError::BackendError(format!("Failed to get value: {}", e)))?;

            for attr in value_attrs {
                if let Attribute::Value(val) = attr {
                    return Ok(val);
                }
            }

            Err(SecretError::BackendError(
                "Object found but has no value attribute".to_string(),
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
        Ok(SecretMetadata {
            id: id.clone(),
            backend: format!("pkcs11://slot{}", self.config.slot_id),
            created_at: None,
            expires_at: None,
            metadata: Some(format!("PKCS#11 slot {}", self.config.slot_id)),
        })
    }

    fn list_secrets(&self) -> Result<Vec<SecretId>, SecretError> {
        #[cfg(feature = "pkcs11")]
        {
            let session = self.get_session()?;

            // Find all objects
            let template = vec![];
            let objects = session
                .find_objects(&template)
                .map_err(|e| SecretError::BackendError(format!("Failed to list objects: {}", e)))?;

            let mut secrets = Vec::new();
            for obj in objects {
                use cryptoki::object::AttributeType;
                let attrs = session
                    .get_attributes(obj, &[AttributeType::Label])
                    .map_err(|e| SecretError::BackendError(format!("Failed to get label: {}", e)))?;

                for attr in attrs {
                    if let Attribute::Label(label) = attr {
                        if let Ok(label_str) = String::from_utf8(label) {
                            secrets.push(label_str);
                        }
                    }
                }
            }

            Ok(secrets)
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
    fn test_pkcs11_stub() {
        let config = Pkcs11Config {
            library_path: "/usr/lib/softhsm/libsofthsm2.so".to_string(),
            slot_id: 0,
            pin: Some("1234".to_string()),
        };

        let backend = Pkcs11Backend::new(config).unwrap();
        assert_eq!(backend.scheme(), "pkcs11");

        // Metadata should work
        let metadata = backend.get_metadata(&"test".to_string()).unwrap();
        assert!(metadata.backend.starts_with("pkcs11://"));
    }
}
