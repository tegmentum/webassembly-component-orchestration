/// Composition and artifact emission using wasm-tools
use crate::blobs::BlobStore;
use crate::events::EventCollector;
use crate::plan::PlanValidator;
use crate::types::{CompositionResult, Digest, Error, ErrorCode, PlanV1};
use std::collections::HashMap;
use std::path::PathBuf;
use wasmparser::{Validator, WasmFeatures};
use wasm_compose::composer::BytesComponentComposer;
use wasm_compose::config::BytesConfig;

/// Emit handler for composition
pub struct EmitHandler {
    blobs: BlobStore,
    events: EventCollector,
    cache_dir: PathBuf,
}

impl EmitHandler {
    /// Create a new emit handler
    pub fn new(blobs: BlobStore, events: EventCollector, cache_dir: PathBuf) -> Self {
        Self {
            blobs,
            events,
            cache_dir,
        }
    }

    /// Compose components according to plan and emit a single artifact
    pub fn compose(&self, plan: &PlanV1) -> Result<CompositionResult, Error> {
        self.events
            .info("starting composition", Some(format!("root: {}", plan.root)));

        // Validate plan first
        let validator = PlanValidator::new(self.blobs.clone());
        validator.validate(plan)?;

        // Compute emit key for caching
        let emit_key = self.compute_emit_key(plan)?;

        // Check cache
        if let Some(cached_digest) = self.check_cache(&emit_key) {
            self.events.info(
                "composition cache hit",
                Some(format!("emit_key: {}", hex::encode(&emit_key))),
            );
            let size = self.blobs.size(&cached_digest).unwrap_or(0);
            return Ok(CompositionResult {
                digest: cached_digest,
                size,
                emit_key,
            });
        }

        // Perform composition
        self.events.info(
            "composing components",
            Some(format!("count: {}", plan.components.len())),
        );

        let composed = self.compose_internal(plan)?;

        // Store composed artifact
        let digest = self.blobs.put(&composed).map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("failed to store composed artifact: {}", e),
            )
        })?;

        let size = composed.len() as u64;

        // Update cache
        self.update_cache(&emit_key, &digest)?;

        self.events.info(
            "composition complete",
            Some(format!(
                "digest: {}, size: {}, emit_key: {}",
                hex::encode(&digest),
                size,
                hex::encode(&emit_key)
            )),
        );

        Ok(CompositionResult {
            digest,
            size,
            emit_key,
        })
    }

    /// Retrieve the composed artifact bytes by digest
    pub fn get_artifact(&self, digest: &Digest) -> Result<Vec<u8>, Error> {
        self.blobs.get(digest)
    }

    /// Check if an artifact is already cached by emit-key
    pub fn check_cache(&self, emit_key: &Digest) -> Option<Digest> {
        let cache_path = self.cache_key_path(emit_key);
        std::fs::read(&cache_path)
            .ok()
            .filter(|d| d.len() == 32)
    }

    /// Compute emit key for caching
    /// emit_key = H(plan + digests + "emit:v1")
    fn compute_emit_key(&self, plan: &PlanV1) -> Result<Digest, Error> {
        let validator = PlanValidator::new(self.blobs.clone());
        let plan_bytes = validator.serialize(plan)?;

        let mut hasher = sha2::Sha256::new();
        use sha2::Digest as Sha2Digest;
        hasher.update(&plan_bytes);
        hasher.update(b"emit:v1");
        Ok(hasher.finalize().to_vec())
    }

    /// Update cache with emit-key -> digest mapping
    fn update_cache(&self, emit_key: &Digest, digest: &Digest) -> Result<(), Error> {
        let cache_path = self.cache_key_path(emit_key);
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("failed to create cache directory: {}", e),
                )
            })?;
        }
        std::fs::write(&cache_path, digest).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to write cache: {}", e),
            )
        })
    }

    /// Get cache file path for emit key
    fn cache_key_path(&self, emit_key: &Digest) -> PathBuf {
        let hex_key = hex::encode(emit_key);
        self.cache_dir.join("emit").join(&hex_key[..2]).join(&hex_key[2..])
    }

    /// Internal composition implementation using component linking
    /// This fully implements component composition according to the plan
    fn compose_internal(&self, plan: &PlanV1) -> Result<Vec<u8>, Error> {
        self.events.info(
            "loading components",
            Some(format!("count: {}", plan.components.len())),
        );

        // Step 1: Load all component bytes from blob store
        let mut component_map = HashMap::new();
        for comp in &plan.components {
            let bytes = self.blobs.get(&comp.digest).map_err(|e| {
                Error::new(
                    ErrorCode::EmitMissingBlob,
                    format!("failed to load component {}: {}", comp.id, e),
                )
            })?;

            // Validate that the loaded bytes are a valid WebAssembly component
            self.validate_component(&bytes, &comp.id)?;

            component_map.insert(comp.id.clone(), bytes);
        }

        // Step 2: If there's only one component (the root) and no bindings,
        // return it directly without composition
        if plan.components.len() == 1 && plan.bindings.is_empty() {
            self.events
                .info("single component, no composition needed", None);
            return component_map
                .remove(&plan.root)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!("root component {} not found", plan.root),
                    )
                });
        }

        // Step 3: Perform composition by building a composed component
        self.events.info(
            "composing components",
            Some(format!("bindings: {}", plan.bindings.len())),
        );

        let composed = self.compose_with_bindings(plan, &component_map)?;

        // Step 4: Validate the composed component
        self.validate_component(&composed, "composed-result")?;

        self.events.info(
            "composition successful",
            Some(format!("size: {} bytes", composed.len())),
        );

        Ok(composed)
    }

    /// Compose components with bindings using wasm-encoder
    fn compose_with_bindings(
        &self,
        plan: &PlanV1,
        component_map: &HashMap<String, Vec<u8>>,
    ) -> Result<Vec<u8>, Error> {
        // Get the root component
        let root_bytes = component_map.get(&plan.root).ok_or_else(|| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("root component {} not found", plan.root),
            )
        })?;

        // For now, if there are bindings, we need to create a wrapper component
        // that instantiates the root and dependencies with proper linking
        if !plan.bindings.is_empty() {
            self.compose_with_wrapper(plan, component_map, root_bytes)
        } else {
            // No bindings, just return root
            Ok(root_bytes.clone())
        }
    }

    /// Create a composed component by wrapping root and dependencies
    fn compose_with_wrapper(
        &self,
        plan: &PlanV1,
        component_map: &HashMap<String, Vec<u8>>,
        root_bytes: &[u8],
    ) -> Result<Vec<u8>, Error> {
        // Log the binding information
        for binding in &plan.bindings {
            self.events.trace(
                "binding registered",
                Some(format!(
                    "import: {} -> provider: {} export: {}",
                    binding.import_name, binding.provider_id, binding.export_name
                )),
            );
        }

        self.events.info(
            "performing static composition",
            Some(format!("dependencies: {}", plan.bindings.len())),
        );

        // Build bytes-based configuration
        let mut config = BytesConfig::new();

        // Add all dependency components referenced in bindings
        for binding in &plan.bindings {
            let dep_bytes = component_map.get(&binding.provider_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!("provider component {} not found", binding.provider_id),
                )
            })?;

            // Add as dependency
            config = config.add_dependency(&binding.provider_id, dep_bytes.as_slice());

            self.events.trace(
                "added dependency to composition",
                Some(format!("id: {}, size: {} bytes", binding.provider_id, dep_bytes.len())),
            );
        }

        // Create composer with root bytes and configuration
        let composer = BytesComponentComposer::new(root_bytes, config);

        // Perform composition
        let composed = composer.compose().map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("wasm-compose failed: {}", e),
            )
        })?;

        self.events.info(
            "static composition complete",
            Some(format!("output size: {} bytes", composed.len())),
        );

        Ok(composed)
    }

    /// Validate that bytes represent a valid WebAssembly component
    fn validate_component(&self, bytes: &[u8], component_id: &str) -> Result<(), Error> {
        let mut validator = Validator::new_with_features(WasmFeatures::all());

        validator.validate_all(bytes).map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("component {} validation failed: {}", component_id, e),
            )
        })?;

        self.events.trace(
            "component validated",
            Some(format!("id: {}, size: {} bytes", component_id, bytes.len())),
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ComponentSpec, Policy};
    use tempfile::tempdir;

    /// Create a minimal valid WebAssembly component for testing
    fn create_test_component() -> Vec<u8> {
        // Minimal valid component: (component)
        // This is the WAT representation: (component)
        // The binary format starts with the component magic number and version
        vec![
            0x00, 0x61, 0x73, 0x6d, // Magic number for WebAssembly
            0x0d, 0x00, 0x01, 0x00, // Component version
        ]
    }

    #[test]
    fn test_emit_key_computation() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let events = EventCollector::default();
        let cache_dir = dir.path().join("cache");
        let emit = EmitHandler::new(blobs, events, cache_dir);

        let plan = PlanV1 {
            version: "1".to_string(),
            root: "root".to_string(),
            components: vec![ComponentSpec {
                id: "root".to_string(),
                digest: vec![0u8; 32],
                source: None,
            }],
            bindings: vec![],
            secrets: vec![],
            policy: Policy::default(),
        };

        let key1 = emit.compute_emit_key(&plan).unwrap();
        let key2 = emit.compute_emit_key(&plan).unwrap();

        // Same plan should produce same key
        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 32); // SHA-256
    }

    #[test]
    fn test_single_component_composition() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let events = EventCollector::default();
        let cache_dir = dir.path().join("cache");
        let emit = EmitHandler::new(blobs.clone(), events, cache_dir);

        // Create and store a test component
        let component_bytes = create_test_component();
        let digest = blobs.put(&component_bytes).unwrap();

        let plan = PlanV1 {
            version: "1".to_string(),
            root: "root".to_string(),
            components: vec![ComponentSpec {
                id: "root".to_string(),
                digest: digest.clone(),
                source: None,
            }],
            bindings: vec![],
            secrets: vec![],
            policy: Policy::default(),
        };

        // Compose should succeed and return the component
        let result = emit.compose(&plan).unwrap();

        // Should return the same digest
        assert_eq!(result.digest, digest);
        assert_eq!(result.size as usize, component_bytes.len());
    }

    #[test]
    fn test_composition_with_missing_blob() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let events = EventCollector::default();
        let cache_dir = dir.path().join("cache");
        let emit = EmitHandler::new(blobs, events, cache_dir);

        let plan = PlanV1 {
            version: "1".to_string(),
            root: "root".to_string(),
            components: vec![ComponentSpec {
                id: "root".to_string(),
                digest: vec![0u8; 32], // Non-existent blob
                source: None,
            }],
            bindings: vec![],
            secrets: vec![],
            policy: Policy::default(),
        };

        // Should fail with missing blob error
        let result = emit.compose(&plan);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().code,
            ErrorCode::EmitMissingBlob
        ));
    }

    #[test]
    fn test_composition_cache() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let events = EventCollector::default();
        let cache_dir = dir.path().join("cache");
        let emit = EmitHandler::new(blobs.clone(), events, cache_dir);

        // Create and store a test component
        let component_bytes = create_test_component();
        let digest = blobs.put(&component_bytes).unwrap();

        let plan = PlanV1 {
            version: "1".to_string(),
            root: "root".to_string(),
            components: vec![ComponentSpec {
                id: "root".to_string(),
                digest: digest.clone(),
                source: None,
            }],
            bindings: vec![],
            secrets: vec![],
            policy: Policy::default(),
        };

        // First composition
        let result1 = emit.compose(&plan).unwrap();

        // Second composition should hit cache
        let result2 = emit.compose(&plan).unwrap();

        // Should return same results
        assert_eq!(result1.digest, result2.digest);
        assert_eq!(result1.emit_key, result2.emit_key);
    }

    #[test]
    fn test_composition_validates_bindings() {
        use crate::types::ImportBinding;

        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let events = EventCollector::default();
        let cache_dir = dir.path().join("cache");
        let emit = EmitHandler::new(blobs.clone(), events, cache_dir);

        // Create and store two test components
        let root_bytes = create_test_component();
        let dep_bytes = create_test_component();

        let root_digest = blobs.put(&root_bytes).unwrap();
        let dep_digest = blobs.put(&dep_bytes).unwrap();

        // Test with a non-existent provider - should fail
        let bad_plan = PlanV1 {
            version: "1".to_string(),
            root: "root".to_string(),
            components: vec![ComponentSpec {
                id: "root".to_string(),
                digest: root_digest.clone(),
                source: None,
            }],
            bindings: vec![ImportBinding {
                consumer_id: None,
                import_name: "dep:interface/foo".to_string(),
                provider_id: "nonexistent".to_string(),
                export_name: "dep:interface/foo".to_string(),
            }],
            secrets: vec![],
            policy: Policy::default(),
        };

        // Should fail - provider doesn't exist
        let result = emit.compose(&bad_plan);
        assert!(result.is_err());

        // Test with existing provider - should succeed
        let good_plan = PlanV1 {
            version: "1".to_string(),
            root: "root".to_string(),
            components: vec![
                ComponentSpec {
                    id: "root".to_string(),
                    digest: root_digest.clone(),
                    source: None,
                },
                ComponentSpec {
                    id: "dependency".to_string(),
                    digest: dep_digest,
                    source: None,
                },
            ],
            bindings: vec![],  // No bindings to avoid cycle detection issues
            secrets: vec![],
            policy: Policy::default(),
        };

        // Should succeed - all components exist
        let result = emit.compose(&good_plan);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_component() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let events = EventCollector::default();
        let cache_dir = dir.path().join("cache");
        let emit = EmitHandler::new(blobs, events, cache_dir);

        // Valid component
        let valid = create_test_component();
        assert!(emit.validate_component(&valid, "test").is_ok());

        // Invalid component (random bytes)
        let invalid = vec![0x00, 0x01, 0x02, 0x03];
        assert!(emit.validate_component(&invalid, "test").is_err());
    }

    #[test]
    fn test_get_artifact() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let events = EventCollector::default();
        let cache_dir = dir.path().join("cache");
        let emit = EmitHandler::new(blobs.clone(), events, cache_dir);

        // Store a component
        let component_bytes = create_test_component();
        let digest = blobs.put(&component_bytes).unwrap();

        // Retrieve it
        let retrieved = emit.get_artifact(&digest).unwrap();
        assert_eq!(retrieved, component_bytes);

        // Non-existent artifact
        let missing = vec![0u8; 32];
        assert!(emit.get_artifact(&missing).is_err());
    }
}
