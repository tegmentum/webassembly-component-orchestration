/// Secret management with pluggable backends
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub mod dev;
#[cfg(feature = "pkcs11")]
pub mod pkcs11;

/// Secret identifier
pub type SecretId = String;

/// Opaque secret token
pub type SecretToken = String;

/// Backend URI
pub type BackendUri = String;

/// Secret error types
#[derive(Debug, Clone)]
pub enum SecretError {
    NotFound,
    AccessDenied,
    BackendError(String),
    InvalidToken,
    Expired,
    InvalidBackend,
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::NotFound => write!(f, "secret not found"),
            SecretError::AccessDenied => write!(f, "access denied"),
            SecretError::BackendError(e) => write!(f, "backend error: {}", e),
            SecretError::InvalidToken => write!(f, "invalid token"),
            SecretError::Expired => write!(f, "secret expired"),
            SecretError::InvalidBackend => write!(f, "invalid backend"),
        }
    }
}

impl std::error::Error for SecretError {}

/// Secret metadata
#[derive(Debug, Clone)]
pub struct SecretMetadata {
    pub id: SecretId,
    pub backend: BackendUri,
    pub created_at: Option<u64>,
    pub expires_at: Option<u64>,
    pub metadata: Option<String>,
}

/// Secret backend trait
pub trait SecretBackend: Send + Sync {
    /// Get the backend URI scheme (e.g., "dev://", "pkcs11://")
    fn scheme(&self) -> &str;

    /// Resolve a secret ID to get its value
    fn resolve(&self, id: &SecretId) -> Result<Vec<u8>, SecretError>;

    /// Get metadata for a secret
    fn get_metadata(&self, id: &SecretId) -> Result<SecretMetadata, SecretError>;

    /// List available secret IDs
    fn list_secrets(&self) -> Result<Vec<SecretId>, SecretError>;
}

/// Token entry for tracking issued tokens
#[derive(Debug, Clone)]
struct TokenEntry {
    secret_id: SecretId,
    backend_uri: BackendUri,
    issued_at: u64,
    expires_at: Option<u64>,
}

/// Secret manager with pluggable backends
pub struct SecretManager {
    backends: Arc<Mutex<HashMap<String, Box<dyn SecretBackend>>>>,
    tokens: Arc<Mutex<HashMap<SecretToken, TokenEntry>>>,
    token_counter: Arc<Mutex<u64>>,
}

impl SecretManager {
    /// Create a new secret manager
    pub fn new() -> Self {
        Self {
            backends: Arc::new(Mutex::new(HashMap::new())),
            tokens: Arc::new(Mutex::new(HashMap::new())),
            token_counter: Arc::new(Mutex::new(0)),
        }
    }

    /// Register a secret backend
    pub fn register_backend(&self, backend: Box<dyn SecretBackend>) -> Result<()> {
        let scheme = backend.scheme().to_string();
        self.backends.lock().unwrap().insert(scheme, backend);
        Ok(())
    }

    /// Resolve a secret ID to an opaque token
    pub fn resolve(&self, id: &SecretId, backend_uri: &BackendUri) -> Result<SecretToken, SecretError> {
        // Extract scheme from URI
        let scheme = Self::extract_scheme(backend_uri)?;

        // Get backend
        let backends = self.backends.lock().unwrap();
        let backend = backends
            .get(&scheme)
            .ok_or_else(|| SecretError::InvalidBackend)?;

        // Verify secret exists
        backend.resolve(id)?;

        // Generate token
        let token = self.generate_token();
        let entry = TokenEntry {
            secret_id: id.clone(),
            backend_uri: backend_uri.clone(),
            issued_at: current_timestamp(),
            expires_at: None, // TODO: Support expiration
        };

        self.tokens.lock().unwrap().insert(token.clone(), entry);

        Ok(token)
    }

    /// Get secret value by token (host-only)
    pub fn get_value(&self, token: &SecretToken) -> Result<Vec<u8>, SecretError> {
        let tokens = self.tokens.lock().unwrap();
        let entry = tokens
            .get(token)
            .ok_or(SecretError::InvalidToken)?;

        // Check expiration
        if let Some(expires_at) = entry.expires_at {
            if current_timestamp() > expires_at {
                return Err(SecretError::Expired);
            }
        }

        // Get backend
        let scheme = Self::extract_scheme(&entry.backend_uri)?;
        let backends = self.backends.lock().unwrap();
        let backend = backends
            .get(&scheme)
            .ok_or_else(|| SecretError::InvalidBackend)?;

        // Get secret value
        backend.resolve(&entry.secret_id)
    }

    /// Validate that a token is still valid
    pub fn validate_token(&self, token: &SecretToken) -> Result<bool, SecretError> {
        let tokens = self.tokens.lock().unwrap();
        if let Some(entry) = tokens.get(token) {
            if let Some(expires_at) = entry.expires_at {
                Ok(current_timestamp() <= expires_at)
            } else {
                Ok(true)
            }
        } else {
            Ok(false)
        }
    }

    /// Revoke a token
    pub fn revoke_token(&self, token: &SecretToken) -> Result<(), SecretError> {
        self.tokens.lock().unwrap().remove(token);
        Ok(())
    }

    /// Get metadata about a secret
    pub fn get_metadata(&self, token: &SecretToken) -> Result<SecretMetadata, SecretError> {
        let tokens = self.tokens.lock().unwrap();
        let entry = tokens
            .get(token)
            .ok_or(SecretError::InvalidToken)?;

        let scheme = Self::extract_scheme(&entry.backend_uri)?;
        let backends = self.backends.lock().unwrap();
        let backend = backends
            .get(&scheme)
            .ok_or_else(|| SecretError::InvalidBackend)?;

        backend.get_metadata(&entry.secret_id)
    }

    /// List available secret IDs
    pub fn list_secrets(&self, backend_uri: Option<&BackendUri>) -> Result<Vec<SecretId>, SecretError> {
        if let Some(uri) = backend_uri {
            let scheme = Self::extract_scheme(uri)?;
            let backends = self.backends.lock().unwrap();
            let backend = backends
                .get(&scheme)
                .ok_or_else(|| SecretError::InvalidBackend)?;
            backend.list_secrets()
        } else {
            // List from all backends
            let mut all_secrets = Vec::new();
            let backends = self.backends.lock().unwrap();
            for backend in backends.values() {
                if let Ok(secrets) = backend.list_secrets() {
                    all_secrets.extend(secrets);
                }
            }
            Ok(all_secrets)
        }
    }

    /// Check if a backend is registered
    pub fn has_backend(&self, backend_uri: &BackendUri) -> bool {
        if let Ok(scheme) = Self::extract_scheme(backend_uri) {
            self.backends.lock().unwrap().contains_key(&scheme)
        } else {
            false
        }
    }

    /// List registered backends
    pub fn list_backends(&self) -> Vec<String> {
        self.backends
            .lock()
            .unwrap()
            .keys()
            .map(|s| format!("{}://", s))
            .collect()
    }

    /// Get a reference to a specific backend (for testing/admin)
    pub fn get_backend(&self, _scheme: &str) -> Option<Box<dyn SecretBackend>> {
        // This is a workaround for testing - in production, backends wouldn't be exposed
        // For now, return None since we can't easily clone trait objects
        None
    }

    /// Extract scheme from backend URI
    fn extract_scheme(uri: &str) -> Result<String, SecretError> {
        if let Some(pos) = uri.find("://") {
            Ok(uri[..pos].to_string())
        } else {
            Err(SecretError::InvalidBackend)
        }
    }

    /// Generate a unique token
    fn generate_token(&self) -> SecretToken {
        let mut counter = self.token_counter.lock().unwrap();
        *counter += 1;
        let timestamp = current_timestamp();
        format!("token_{}_{}", timestamp, *counter)
    }
}

impl Default for SecretManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Get current timestamp in seconds since epoch
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_scheme() {
        assert_eq!(
            SecretManager::extract_scheme("dev://").unwrap(),
            "dev"
        );
        assert_eq!(
            SecretManager::extract_scheme("pkcs11://slot0").unwrap(),
            "pkcs11"
        );
        assert!(SecretManager::extract_scheme("invalid").is_err());
    }
}
