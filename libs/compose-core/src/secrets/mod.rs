/// Secret management with pluggable backends
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub mod dev;

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

/// Statistics about the token store
#[derive(Debug, Clone)]
pub struct TokenStats {
    /// Total number of tokens in the store
    pub total: usize,
    /// Number of active (non-expired) tokens
    pub active: usize,
    /// Number of expired tokens
    pub expired: usize,
    /// Number of tokens that never expire
    pub never_expire: usize,
}

/// Token expiration configuration
#[derive(Debug, Clone, Copy)]
pub enum TokenTtl {
    /// Token never expires
    Never,
    /// Token expires after the specified number of seconds
    Seconds(u64),
}

impl TokenTtl {
    /// Calculate the expiration timestamp from now
    fn expires_at(&self) -> Option<u64> {
        match self {
            TokenTtl::Never => None,
            TokenTtl::Seconds(ttl) => Some(current_timestamp() + ttl),
        }
    }
}

impl Default for TokenTtl {
    fn default() -> Self {
        // Default to 1 hour expiration
        TokenTtl::Seconds(3600)
    }
}

/// Secret manager with pluggable backends
pub struct SecretManager {
    backends: Arc<Mutex<HashMap<String, Box<dyn SecretBackend>>>>,
    tokens: Arc<Mutex<HashMap<SecretToken, TokenEntry>>>,
    token_counter: Arc<Mutex<u64>>,
    token_ttl: TokenTtl,
}

impl SecretManager {
    /// Create a new secret manager with default token TTL (1 hour)
    pub fn new() -> Self {
        Self::new_with_ttl(TokenTtl::default())
    }

    /// Create a new secret manager with custom token TTL
    pub fn new_with_ttl(token_ttl: TokenTtl) -> Self {
        Self {
            backends: Arc::new(Mutex::new(HashMap::new())),
            tokens: Arc::new(Mutex::new(HashMap::new())),
            token_counter: Arc::new(Mutex::new(0)),
            token_ttl,
        }
    }

    /// Get the current token TTL setting
    pub fn token_ttl(&self) -> TokenTtl {
        self.token_ttl
    }

    /// Update the token TTL (affects newly issued tokens only)
    pub fn set_token_ttl(&mut self, ttl: TokenTtl) {
        self.token_ttl = ttl;
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
            expires_at: self.token_ttl.expires_at(),
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

    /// Remove all expired tokens from the token store
    /// Returns the number of tokens removed
    pub fn cleanup_expired_tokens(&self) -> usize {
        let mut tokens = self.tokens.lock().unwrap();
        let current_time = current_timestamp();
        let initial_count = tokens.len();

        tokens.retain(|_, entry| {
            if let Some(expires_at) = entry.expires_at {
                current_time <= expires_at
            } else {
                true // Keep tokens that never expire
            }
        });

        initial_count - tokens.len()
    }

    /// Get statistics about the token store
    pub fn token_stats(&self) -> TokenStats {
        let tokens = self.tokens.lock().unwrap();
        let current_time = current_timestamp();
        let mut expired = 0;
        let mut active = 0;
        let mut never_expire = 0;

        for entry in tokens.values() {
            if let Some(expires_at) = entry.expires_at {
                if current_time > expires_at {
                    expired += 1;
                } else {
                    active += 1;
                }
            } else {
                never_expire += 1;
            }
        }

        TokenStats {
            total: tokens.len(),
            active,
            expired,
            never_expire,
        }
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
    use std::thread;
    use std::time::Duration;

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

    #[test]
    fn test_token_ttl_default() {
        let ttl = TokenTtl::default();
        let expires = ttl.expires_at();
        assert!(expires.is_some());
        let expected_min = current_timestamp() + 3599;
        let expected_max = current_timestamp() + 3601;
        let actual = expires.unwrap();
        assert!(actual >= expected_min && actual <= expected_max);
    }

    #[test]
    fn test_token_ttl_never() {
        let ttl = TokenTtl::Never;
        assert!(ttl.expires_at().is_none());
    }

    #[test]
    fn test_token_ttl_custom() {
        let ttl = TokenTtl::Seconds(60);
        let expires = ttl.expires_at();
        assert!(expires.is_some());
        let expected_min = current_timestamp() + 59;
        let expected_max = current_timestamp() + 61;
        let actual = expires.unwrap();
        assert!(actual >= expected_min && actual <= expected_max);
    }

    #[test]
    fn test_token_expiration() {
        // Create manager with 1-second TTL for testing
        let manager = SecretManager::new_with_ttl(TokenTtl::Seconds(1));

        // Register dev backend and add a test secret
        let backend = dev::DevBackend::new();
        backend.add_secret("test", b"test-value");
        manager.register_backend(Box::new(backend)).unwrap();

        // Resolve a secret to get a token
        let token = manager.resolve(&"test".to_string(), &"dev://".to_string()).unwrap();

        // Verify token is valid immediately
        assert!(manager.validate_token(&token).unwrap());

        // Get value should work
        let value = manager.get_value(&token);
        assert!(value.is_ok());

        // Wait for token to expire (sleep longer than TTL to account for timing)
        thread::sleep(Duration::from_millis(1500));

        // Token should be invalid now
        assert!(!manager.validate_token(&token).unwrap());

        // Getting value should fail with Expired error
        let value = manager.get_value(&token);
        assert!(value.is_err());
        assert!(matches!(value.unwrap_err(), SecretError::Expired));
    }

    #[test]
    fn test_token_never_expires() {
        // Create manager with no expiration
        let manager = SecretManager::new_with_ttl(TokenTtl::Never);

        // Register dev backend and add a test secret
        let backend = dev::DevBackend::new();
        backend.add_secret("test", b"test-value");
        manager.register_backend(Box::new(backend)).unwrap();

        // Resolve a secret to get a token
        let token = manager.resolve(&"test".to_string(), &"dev://".to_string()).unwrap();

        // Token should always be valid
        assert!(manager.validate_token(&token).unwrap());

        // Even after waiting, token should still be valid
        thread::sleep(Duration::from_millis(100));
        assert!(manager.validate_token(&token).unwrap());

        // Get value should always work
        assert!(manager.get_value(&token).is_ok());
    }

    #[test]
    fn test_cleanup_expired_tokens() {
        // Create manager with 1-second TTL
        let manager = SecretManager::new_with_ttl(TokenTtl::Seconds(1));

        // Register dev backend and add test secrets
        let backend = dev::DevBackend::new();
        backend.add_secret("test1", b"value1");
        backend.add_secret("test2", b"value2");
        backend.add_secret("test3", b"value3");
        manager.register_backend(Box::new(backend)).unwrap();

        // Create multiple tokens
        let token1 = manager.resolve(&"test1".to_string(), &"dev://".to_string()).unwrap();
        let token2 = manager.resolve(&"test2".to_string(), &"dev://".to_string()).unwrap();
        let token3 = manager.resolve(&"test3".to_string(), &"dev://".to_string()).unwrap();

        // All tokens should be active
        let stats = manager.token_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.active, 3);
        assert_eq!(stats.expired, 0);

        // Wait for tokens to expire (sleep longer than TTL to account for timing)
        thread::sleep(Duration::from_millis(1500));

        // Check stats - tokens should be expired
        let stats = manager.token_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.expired, 3);

        // Cleanup expired tokens
        let removed = manager.cleanup_expired_tokens();
        assert_eq!(removed, 3);

        // Stats should show empty store
        let stats = manager.token_stats();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.expired, 0);

        // Tokens should no longer be valid
        assert!(!manager.validate_token(&token1).unwrap());
        assert!(!manager.validate_token(&token2).unwrap());
        assert!(!manager.validate_token(&token3).unwrap());
    }

    #[test]
    fn test_token_stats_mixed() {
        // Create manager with 1-second TTL for some tokens
        let mut manager = SecretManager::new_with_ttl(TokenTtl::Seconds(1));

        // Register dev backend and add test secrets
        let backend = dev::DevBackend::new();
        backend.add_secret("test1", b"value1");
        backend.add_secret("test2", b"value2");
        backend.add_secret("test3", b"value3");
        manager.register_backend(Box::new(backend)).unwrap();

        // Create expiring tokens
        let _token1 = manager.resolve(&"test1".to_string(), &"dev://".to_string()).unwrap();
        let _token2 = manager.resolve(&"test2".to_string(), &"dev://".to_string()).unwrap();

        // Switch to never-expiring
        manager.set_token_ttl(TokenTtl::Never);
        let _token3 = manager.resolve(&"test3".to_string(), &"dev://".to_string()).unwrap();

        // Initial stats
        let stats = manager.token_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.active, 2);
        assert_eq!(stats.never_expire, 1);
        assert_eq!(stats.expired, 0);

        // Wait for expiring tokens to expire (sleep longer than TTL to account for timing)
        thread::sleep(Duration::from_millis(1500));

        // Stats after expiration
        let stats = manager.token_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.never_expire, 1);
        assert_eq!(stats.expired, 2);

        // Cleanup should only remove expired tokens
        let removed = manager.cleanup_expired_tokens();
        assert_eq!(removed, 2);

        // Final stats
        let stats = manager.token_stats();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.never_expire, 1);
        assert_eq!(stats.expired, 0);
    }
}
