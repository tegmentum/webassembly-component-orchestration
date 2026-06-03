/// Secret management with pluggable backends
use crate::host::{SharedClock, SystemClock};
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
    /// Retained for token bookkeeping/debugging; not read yet.
    #[allow(dead_code)]
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
    /// Calculate the expiration timestamp given the current time (seconds).
    fn expires_at(&self, now: u64) -> Option<u64> {
        match self {
            TokenTtl::Never => None,
            TokenTtl::Seconds(ttl) => Some(now + ttl),
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
    token_ttl: TokenTtl,
    clock: SharedClock,
}

impl SecretManager {
    /// Create a new secret manager with default token TTL (1 hour).
    pub fn new(clock: SharedClock) -> Self {
        Self::new_with_ttl(TokenTtl::default(), clock)
    }

    /// Create a new secret manager with custom token TTL.
    pub fn new_with_ttl(token_ttl: TokenTtl, clock: SharedClock) -> Self {
        Self {
            backends: Arc::new(Mutex::new(HashMap::new())),
            tokens: Arc::new(Mutex::new(HashMap::new())),
            token_ttl,
            clock,
        }
    }

    fn now_secs(&self) -> u64 {
        self.clock.now_unix_secs()
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
    pub fn resolve(
        &self,
        id: &SecretId,
        backend_uri: &BackendUri,
    ) -> Result<SecretToken, SecretError> {
        // Extract scheme from URI
        let scheme = Self::extract_scheme(backend_uri)?;

        // Get backend
        let backends = self.backends.lock().unwrap();
        let backend = backends.get(&scheme).ok_or(SecretError::InvalidBackend)?;

        // Verify secret exists
        backend.resolve(id)?;

        // Generate token
        let token = self.generate_token();
        let entry = TokenEntry {
            secret_id: id.clone(),
            backend_uri: backend_uri.clone(),
            issued_at: self.now_secs(),
            expires_at: self.token_ttl.expires_at(self.now_secs()),
        };

        self.tokens.lock().unwrap().insert(token.clone(), entry);

        Ok(token)
    }

    /// Get secret value by token (host-only)
    pub fn get_value(&self, token: &SecretToken) -> Result<Vec<u8>, SecretError> {
        let tokens = self.tokens.lock().unwrap();
        let entry = tokens.get(token).ok_or(SecretError::InvalidToken)?;

        // Check expiration
        if let Some(expires_at) = entry.expires_at {
            if self.now_secs() > expires_at {
                return Err(SecretError::Expired);
            }
        }

        // Get backend
        let scheme = Self::extract_scheme(&entry.backend_uri)?;
        let backends = self.backends.lock().unwrap();
        let backend = backends.get(&scheme).ok_or(SecretError::InvalidBackend)?;

        // Get secret value
        backend.resolve(&entry.secret_id)
    }

    /// Validate that a token is still valid
    pub fn validate_token(&self, token: &SecretToken) -> Result<bool, SecretError> {
        let tokens = self.tokens.lock().unwrap();
        if let Some(entry) = tokens.get(token) {
            if let Some(expires_at) = entry.expires_at {
                Ok(self.now_secs() <= expires_at)
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
        let current_time = self.now_secs();
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
        let current_time = self.now_secs();
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
        let entry = tokens.get(token).ok_or(SecretError::InvalidToken)?;

        let scheme = Self::extract_scheme(&entry.backend_uri)?;
        let backends = self.backends.lock().unwrap();
        let backend = backends.get(&scheme).ok_or(SecretError::InvalidBackend)?;

        backend.get_metadata(&entry.secret_id)
    }

    /// List available secret IDs
    pub fn list_secrets(
        &self,
        backend_uri: Option<&BackendUri>,
    ) -> Result<Vec<SecretId>, SecretError> {
        if let Some(uri) = backend_uri {
            let scheme = Self::extract_scheme(uri)?;
            let backends = self.backends.lock().unwrap();
            let backend = backends.get(&scheme).ok_or(SecretError::InvalidBackend)?;
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

    /// Generate an opaque, unguessable bearer token.
    ///
    /// The token is the only thing gating [`get_value`](Self::get_value),
    /// so it must not be derivable from the timestamp or a counter — those
    /// are predictable and would let an attacker forge a valid token. We
    /// draw 256 bits from the OS / wasi entropy source (via `getrandom`,
    /// which has a wasm32-wasip2 backend) and hex-encode them.
    fn generate_token(&self) -> SecretToken {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("secret token entropy source unavailable");
        format!("st_{}", hex::encode(bytes))
    }
}

impl Default for SecretManager {
    fn default() -> Self {
        Self::new(SystemClock::shared())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Clock, ManualClock};

    fn now_secs() -> u64 {
        SystemClock.now_unix_secs()
    }

    #[test]
    fn test_extract_scheme() {
        assert_eq!(SecretManager::extract_scheme("dev://").unwrap(), "dev");
        assert_eq!(
            SecretManager::extract_scheme("pkcs11://slot0").unwrap(),
            "pkcs11"
        );
        assert!(SecretManager::extract_scheme("invalid").is_err());
    }

    #[test]
    fn test_token_ttl_default() {
        let ttl = TokenTtl::default();
        let expires = ttl.expires_at(now_secs());
        assert!(expires.is_some());
        let expected_min = now_secs() + 3599;
        let expected_max = now_secs() + 3601;
        let actual = expires.unwrap();
        assert!(actual >= expected_min && actual <= expected_max);
    }

    #[test]
    fn test_token_ttl_never() {
        let ttl = TokenTtl::Never;
        assert!(ttl.expires_at(now_secs()).is_none());
    }

    #[test]
    fn test_token_ttl_custom() {
        let ttl = TokenTtl::Seconds(60);
        let expires = ttl.expires_at(now_secs());
        assert!(expires.is_some());
        let expected_min = now_secs() + 59;
        let expected_max = now_secs() + 61;
        let actual = expires.unwrap();
        assert!(actual >= expected_min && actual <= expected_max);
    }

    #[test]
    fn test_token_expiration() {
        // Use a manual clock so we can deterministically advance past the TTL.
        let clock = ManualClock::shared(1_000);
        let manager = SecretManager::new_with_ttl(TokenTtl::Seconds(1), clock.clone());

        let backend = dev::DevBackend::new(clock.clone());
        backend.add_secret("test", b"test-value");
        manager.register_backend(Box::new(backend)).unwrap();

        let token = manager
            .resolve(&"test".to_string(), &"dev://".to_string())
            .unwrap();

        // Valid immediately.
        assert!(manager.validate_token(&token).unwrap());
        assert!(manager.get_value(&token).is_ok());

        // Advance past expiration.
        clock.advance(2);

        assert!(!manager.validate_token(&token).unwrap());
        let value = manager.get_value(&token);
        assert!(matches!(value.unwrap_err(), SecretError::Expired));
    }

    #[test]
    fn test_tokens_are_random_and_unguessable() {
        let clock = ManualClock::shared(1_000);
        let manager = SecretManager::new_with_ttl(TokenTtl::Never, clock.clone());
        let backend = dev::DevBackend::new(clock.clone());
        backend.add_secret("s", b"value");
        manager.register_backend(Box::new(backend)).unwrap();

        // Many resolves of the *same* secret at the *same* clock time must
        // still produce distinct, high-entropy tokens — a counter/timestamp
        // scheme would collide or be predictable here.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            let token = manager
                .resolve(&"s".to_string(), &"dev://".to_string())
                .unwrap();
            // Shape: "st_" + 64 lowercase hex chars (256 bits of entropy).
            let hex = token.strip_prefix("st_").expect("token has st_ prefix");
            assert_eq!(hex.len(), 64, "256-bit token");
            assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(seen.insert(token), "tokens must be unique");
        }
    }

    #[test]
    fn test_token_never_expires() {
        let clock = ManualClock::shared(1_000);
        let manager = SecretManager::new_with_ttl(TokenTtl::Never, clock.clone());

        let backend = dev::DevBackend::new(clock.clone());
        backend.add_secret("test", b"test-value");
        manager.register_backend(Box::new(backend)).unwrap();

        let token = manager
            .resolve(&"test".to_string(), &"dev://".to_string())
            .unwrap();

        assert!(manager.validate_token(&token).unwrap());

        // Even after an arbitrarily large jump, the token is still valid.
        clock.advance(1_000_000);
        assert!(manager.validate_token(&token).unwrap());
        assert!(manager.get_value(&token).is_ok());
    }

    #[test]
    fn test_cleanup_expired_tokens() {
        let clock = ManualClock::shared(1_000);
        let manager = SecretManager::new_with_ttl(TokenTtl::Seconds(1), clock.clone());

        let backend = dev::DevBackend::new(clock.clone());
        backend.add_secret("test1", b"value1");
        backend.add_secret("test2", b"value2");
        backend.add_secret("test3", b"value3");
        manager.register_backend(Box::new(backend)).unwrap();

        let token1 = manager
            .resolve(&"test1".to_string(), &"dev://".to_string())
            .unwrap();
        let token2 = manager
            .resolve(&"test2".to_string(), &"dev://".to_string())
            .unwrap();
        let token3 = manager
            .resolve(&"test3".to_string(), &"dev://".to_string())
            .unwrap();

        let stats = manager.token_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.active, 3);
        assert_eq!(stats.expired, 0);

        clock.advance(2);

        let stats = manager.token_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.expired, 3);

        let removed = manager.cleanup_expired_tokens();
        assert_eq!(removed, 3);

        let stats = manager.token_stats();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.expired, 0);

        assert!(!manager.validate_token(&token1).unwrap());
        assert!(!manager.validate_token(&token2).unwrap());
        assert!(!manager.validate_token(&token3).unwrap());
    }

    #[test]
    fn test_token_stats_mixed() {
        let clock = ManualClock::shared(1_000);
        let mut manager = SecretManager::new_with_ttl(TokenTtl::Seconds(1), clock.clone());

        let backend = dev::DevBackend::new(clock.clone());
        backend.add_secret("test1", b"value1");
        backend.add_secret("test2", b"value2");
        backend.add_secret("test3", b"value3");
        manager.register_backend(Box::new(backend)).unwrap();

        // Two expiring tokens, one never-expiring.
        let _token1 = manager
            .resolve(&"test1".to_string(), &"dev://".to_string())
            .unwrap();
        let _token2 = manager
            .resolve(&"test2".to_string(), &"dev://".to_string())
            .unwrap();
        manager.set_token_ttl(TokenTtl::Never);
        let _token3 = manager
            .resolve(&"test3".to_string(), &"dev://".to_string())
            .unwrap();

        let stats = manager.token_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.active, 2);
        assert_eq!(stats.never_expire, 1);
        assert_eq!(stats.expired, 0);

        clock.advance(2);

        let stats = manager.token_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.never_expire, 1);
        assert_eq!(stats.expired, 2);

        let removed = manager.cleanup_expired_tokens();
        assert_eq!(removed, 2);

        let stats = manager.token_stats();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.never_expire, 1);
        assert_eq!(stats.expired, 0);
    }
}
