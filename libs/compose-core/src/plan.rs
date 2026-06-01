/// Plan serialization, deserialization, and validation
use crate::blobs::{compute_digest, BlobStore};
use crate::types::{Digest, Error, ErrorCode, PlanV1};
use std::collections::{HashMap, HashSet};

/// Serialize a plan to canonical CBOR bytes.
///
/// Pure function — no I/O, no blob store needed. Exposed at module
/// level so callers (including the wasm orchestrator) can use it
/// without constructing a [`PlanValidator`].
pub fn serialize(plan: &PlanV1) -> Result<Vec<u8>, Error> {
    let mut buffer = Vec::new();
    ciborium::into_writer(plan, &mut buffer).map_err(|e| {
        Error::new(
            ErrorCode::PlanInvalidCbor,
            format!("failed to serialize plan: {}", e),
        )
    })?;
    Ok(buffer)
}

/// Deserialize a plan from canonical CBOR bytes. Pure function.
pub fn deserialize(bytes: &[u8]) -> Result<PlanV1, Error> {
    ciborium::from_reader(bytes).map_err(|e| {
        Error::new(
            ErrorCode::PlanInvalidCbor,
            format!("failed to deserialize plan: {}", e),
        )
    })
}

/// Compute the canonical digest of a plan: SHA-256 of its canonical
/// CBOR encoding. Pure function.
pub fn compute_plan_digest(plan: &PlanV1) -> Result<Digest, Error> {
    let bytes = serialize(plan)?;
    Ok(compute_digest(&bytes))
}

/// Plan validator with incremental validation pipeline
pub struct PlanValidator {
    blobs: BlobStore,
}

impl PlanValidator {
    /// Create a new plan validator
    pub fn new(blobs: BlobStore) -> Self {
        Self { blobs }
    }

    /// Serialize a plan to canonical CBOR bytes
    pub fn serialize(&self, plan: &PlanV1) -> Result<Vec<u8>, Error> {
        serialize(plan)
    }

    /// Deserialize a plan from canonical CBOR bytes
    pub fn deserialize(&self, bytes: &[u8]) -> Result<PlanV1, Error> {
        deserialize(bytes)
    }

    /// Validate a plan structure and graph
    pub fn validate(&self, plan: &PlanV1) -> Result<(), Error> {
        // Phase 1: Schema validation
        self.validate_schema(plan)?;

        // Phase 2: Blob availability
        self.validate_blobs(plan)?;

        // Phase 3: Graph structure
        self.validate_graph(plan)?;

        // Phase 4: Bindings
        self.validate_bindings(plan)?;

        // Phase 5: Linkage constraints
        self.validate_linkage(plan)?;

        Ok(())
    }

    /// Reject plan/linkage combinations the runtime can't honor. Runtime
    /// linking is a non-deterministic operation, so it is incompatible
    /// with strict determinism — fail fast here rather than at exec.
    fn validate_linkage(&self, plan: &PlanV1) -> Result<(), Error> {
        use crate::types::{DeterminismMode, Linkage};
        if plan.linkage == Linkage::Runtime
            && plan.policy.determinism == DeterminismMode::Strict
        {
            return Err(Error::new(
                ErrorCode::PlanInvalidGraph,
                "runtime linkage is incompatible with strict determinism",
            ));
        }
        Ok(())
    }

    /// Compute the canonical digest of a plan
    pub fn compute_digest(&self, plan: &PlanV1) -> Result<Digest, Error> {
        compute_plan_digest(plan)
    }

    /// Phase 1: Validate schema and basic structure
    fn validate_schema(&self, plan: &PlanV1) -> Result<(), Error> {
        // Check version
        if plan.version != "1" {
            return Err(Error::new(
                ErrorCode::PlanInvalidSchema,
                format!("unsupported plan version: {}", plan.version),
            ));
        }

        // Check root exists
        if plan.root.is_empty() {
            return Err(Error::new(
                ErrorCode::PlanMissingField,
                "plan root component ID is empty",
            ));
        }

        // Check components
        if plan.components.is_empty() {
            return Err(Error::new(
                ErrorCode::PlanMissingField,
                "plan has no components",
            ));
        }

        // Check for duplicate component IDs
        let mut seen = HashSet::new();
        for comp in &plan.components {
            if !seen.insert(&comp.id) {
                return Err(Error::new(
                    ErrorCode::PlanInvalidSchema,
                    format!("duplicate component ID: {}", comp.id),
                ));
            }

            // Validate digest is 32 bytes (SHA-256)
            if comp.digest.len() != 32 {
                return Err(Error::new(
                    ErrorCode::PlanInvalidSchema,
                    format!(
                        "component {} has invalid digest length: {}",
                        comp.id,
                        comp.digest.len()
                    ),
                ));
            }
        }

        Ok(())
    }

    /// Phase 2: Validate all component blobs are available
    fn validate_blobs(&self, plan: &PlanV1) -> Result<(), Error> {
        for comp in &plan.components {
            if !self.blobs.has(&comp.digest) {
                return Err(Error::new(
                    ErrorCode::EmitMissingBlob,
                    format!(
                        "component {} blob not found: {}",
                        comp.id,
                        hex::encode(&comp.digest)
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Phase 3: Validate graph structure
    fn validate_graph(&self, plan: &PlanV1) -> Result<(), Error> {
        // Build component ID map
        let comp_map: HashMap<_, _> = plan
            .components
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect();

        // Check root exists
        if !comp_map.contains_key(plan.root.as_str()) {
            return Err(Error::new(
                ErrorCode::PlanInvalidGraph,
                format!("root component {} not found in components", plan.root),
            ));
        }

        // Check for cycles using DFS
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();

        if self.has_cycle(&plan.root, &plan.bindings, &comp_map, &mut visited, &mut rec_stack)? {
            return Err(Error::new(
                ErrorCode::PlanCycleDetected,
                "circular dependency detected in component graph",
            ));
        }

        Ok(())
    }

    /// Phase 4: Validate bindings
    fn validate_bindings(&self, plan: &PlanV1) -> Result<(), Error> {
        let comp_ids: HashSet<_> = plan.components.iter().map(|c| c.id.as_str()).collect();

        for binding in &plan.bindings {
            // Check provider exists
            if !comp_ids.contains(binding.provider_id.as_str()) {
                return Err(Error::new(
                    ErrorCode::PlanInvalidGraph,
                    format!("binding provider {} not found", binding.provider_id),
                ));
            }

            // Validate names are not empty
            if binding.import_name.is_empty() || binding.export_name.is_empty() {
                return Err(Error::new(
                    ErrorCode::PlanInvalidSchema,
                    "binding has empty import or export name",
                ));
            }
        }

        Ok(())
    }

    /// Check for cycles in the dependency graph using DFS
    fn has_cycle(
        &self,
        node: &str,
        bindings: &[crate::types::ImportBinding],
        comp_map: &HashMap<&str, &crate::types::ComponentSpec>,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
    ) -> Result<bool, Error> {
        if rec_stack.contains(node) {
            return Ok(true); // Cycle detected
        }

        if visited.contains(node) {
            return Ok(false); // Already processed
        }

        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());

        // Find all dependencies of this node
        // Only consider bindings where this node is the consumer
        let deps: Vec<_> = bindings
            .iter()
            .filter_map(|b| {
                // Find bindings where this component imports from others
                if b.consumer_id.as_deref() == Some(node) {
                    Some(b.provider_id.as_str())
                } else {
                    None
                }
            })
            .collect();

        for dep in deps {
            if self.has_cycle(dep, bindings, comp_map, visited, rec_stack)? {
                return Ok(true);
            }
        }

        rec_stack.remove(node);
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ComponentSpec, ImportBinding, Policy, SecretBinding};
    use tempfile::tempdir;

    fn create_test_plan() -> PlanV1 {
        PlanV1 {
            version: "1".to_string(),
            root: "root".to_string(),
            components: vec![ComponentSpec {
                id: "root".to_string(),
                digest: vec![0u8; 32], // Dummy digest
                source: None,
            }],
            bindings: vec![],
            secrets: vec![],
            policy: Policy::default(),
            linkage: Default::default(),
        }
    }

    #[test]
    fn test_serialize_deserialize() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let validator = PlanValidator::new(blobs);

        let plan = create_test_plan();
        let bytes = validator.serialize(&plan).unwrap();
        let deserialized = validator.deserialize(&bytes).unwrap();

        assert_eq!(plan.version, deserialized.version);
        assert_eq!(plan.root, deserialized.root);
    }

    #[test]
    fn test_validate_schema() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let validator = PlanValidator::new(blobs);

        let mut plan = create_test_plan();
        plan.version = "2".to_string();

        let result = validator.validate_schema(&plan);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err().code, ErrorCode::PlanInvalidSchema));
    }
}
