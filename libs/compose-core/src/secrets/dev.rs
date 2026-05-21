/// Development/in-memory secret backend
/// For testing and development only - stores secrets in memory
use super::*;
use std::sync::Arc;
use std::sync::Mutex;

/// Development secret backend
pub struct DevBackend {
    secrets: Arc<Mutex<HashMap<SecretId, Vec<u8>>>>,
}

impl DevBackend {
    /// Create a new dev backend
    pub fn new() -> Self {
        Self {
            secrets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Add a secret to the backend
    pub fn add_secret(&self, id: impl Into<String>, value: impl AsRef<[u8]>) {
        self.secrets
            .lock()
            .unwrap()
            .insert(id.into(), value.as_ref().to_vec());
    }

    /// Remove a secret from the backend
    pub fn remove_secret(&self, id: &str) {
        self.secrets.lock().unwrap().remove(id);
    }

    /// Clear all secrets
    pub fn clear(&self) {
        self.secrets.lock().unwrap().clear();
    }
}

impl Default for DevBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretBackend for DevBackend {
    fn scheme(&self) -> &str {
        "dev"
    }

    fn resolve(&self, id: &SecretId) -> Result<Vec<u8>, SecretError> {
        self.secrets
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or(SecretError::NotFound)
    }

    fn get_metadata(&self, id: &SecretId) -> Result<SecretMetadata, SecretError> {
        if self.secrets.lock().unwrap().contains_key(id) {
            Ok(SecretMetadata {
                id: id.clone(),
                backend: "dev://".to_string(),
                created_at: Some(current_timestamp()),
                expires_at: None,
                metadata: Some("dev backend - not for production".to_string()),
            })
        } else {
            Err(SecretError::NotFound)
        }
    }

    fn list_secrets(&self) -> Result<Vec<SecretId>, SecretError> {
        Ok(self.secrets.lock().unwrap().keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dev_backend() {
        let backend = DevBackend::new();

        // Add a secret
        backend.add_secret("api-key", b"secret123");

        // Resolve it
        let value = backend.resolve(&"api-key".to_string()).unwrap();
        assert_eq!(value, b"secret123");

        // Get metadata
        let metadata = backend.get_metadata(&"api-key".to_string()).unwrap();
        assert_eq!(metadata.id, "api-key");
        assert_eq!(metadata.backend, "dev://");

        // List secrets
        let secrets = backend.list_secrets().unwrap();
        assert_eq!(secrets.len(), 1);
        assert!(secrets.contains(&"api-key".to_string()));

        // Remove secret
        backend.remove_secret("api-key");
        assert!(backend.resolve(&"api-key".to_string()).is_err());
    }

    #[test]
    fn test_not_found() {
        let backend = DevBackend::new();
        let result = backend.resolve(&"nonexistent".to_string());
        assert!(matches!(result, Err(SecretError::NotFound)));
    }
}
