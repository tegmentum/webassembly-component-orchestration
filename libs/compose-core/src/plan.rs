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

        // Phase 6: Explicit exports
        self.validate_explicit_exports(plan)?;

        Ok(())
    }

    /// Validate a plan *file*'s structure without requiring its component
    /// blobs to be staged: schema, canonical component order, graph, bindings,
    /// and linkage. Blob availability (phase 2 of [`validate`]) is an
    /// emit/exec-time concern — a plan file can be linted on its own. Used by
    /// `composectl plan validate`.
    pub fn validate_structure(&self, plan: &PlanV1) -> Result<(), Error> {
        self.validate_schema(plan)?;
        self.validate_graph(plan)?;
        self.validate_bindings(plan)?;
        self.validate_linkage(plan)?;
        self.validate_explicit_exports(plan)?;
        Ok(())
    }

    /// Explicit re-exports must reference a component in the plan and
    /// name a non-empty interface. Duplicates (same source + interface)
    /// are a plan authoring bug — catch them here rather than at emit
    /// where they surface as a wac graph error.
    fn validate_explicit_exports(&self, plan: &PlanV1) -> Result<(), Error> {
        let comp_ids: HashSet<_> = plan.components.iter().map(|c| c.id.as_str()).collect();
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        for ee in &plan.explicit_exports {
            if ee.source_instance.is_empty() || ee.interface_name.is_empty() {
                return Err(Error::new(
                    ErrorCode::PlanInvalidSchema,
                    "explicit-export has empty source_instance or interface_name",
                ));
            }
            if !comp_ids.contains(ee.source_instance.as_str()) {
                return Err(Error::new(
                    ErrorCode::PlanInvalidGraph,
                    format!(
                        "explicit-export source {} not found in components",
                        ee.source_instance
                    ),
                ));
            }
            if !seen.insert((ee.source_instance.as_str(), ee.interface_name.as_str())) {
                return Err(Error::new(
                    ErrorCode::PlanInvalidSchema,
                    format!(
                        "duplicate explicit-export {}::{}",
                        ee.source_instance, ee.interface_name
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Components must appear in canonical (ascending `id`) order so a plan has
    /// a single canonical encoding. Enforced as part of schema validation, so
    /// it applies to every validation path (`validate` and `validate_structure`).
    fn validate_canonical_order(&self, plan: &PlanV1) -> Result<(), Error> {
        if plan.components.windows(2).any(|w| w[0].id > w[1].id) {
            return Err(Error::new(
                ErrorCode::PlanInvalidSchema,
                "components are not in canonical (ascending id) order",
            ));
        }
        Ok(())
    }

    /// Reject plan/linkage combinations the runtime can't honor. Runtime
    /// linking is a non-deterministic operation, so it is incompatible
    /// with strict determinism — fail fast here rather than at exec.
    fn validate_linkage(&self, plan: &PlanV1) -> Result<(), Error> {
        use crate::types::{DeterminismMode, Linkage};
        if plan.linkage == Linkage::Runtime && plan.policy.determinism == DeterminismMode::Strict {
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

        // Components must be in canonical (ascending id) order.
        self.validate_canonical_order(plan)?;

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
        let comp_map: HashMap<_, _> = plan.components.iter().map(|c| (c.id.as_str(), c)).collect();

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

        if self.has_cycle(
            &plan.root,
            &plan.bindings,
            &comp_map,
            &mut visited,
            &mut rec_stack,
        )? {
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
    #[allow(clippy::only_used_in_recursion)]
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
    use crate::types::{ComponentSpec, Policy};
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
            explicit_exports: vec![],
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
        assert!(matches!(
            result.unwrap_err().code,
            ErrorCode::PlanInvalidSchema
        ));
    }

    /// The canonical CBOR encoding must be byte-identical between a plan
    /// with the pre-`explicit_exports` field-set (i.e. `explicit_exports:
    /// vec![]`) and a plan whose encoder pre-dated the field. This is
    /// what makes the schema extension a backward-compatible wire-format
    /// change rather than a breaking one.
    #[test]
    fn empty_explicit_exports_is_omitted_from_canonical_encoding() {
        // Same shape as `hello-plan.cbor` in conformance/vectors — the
        // vector's committed sha256 doubles as an on-disk snapshot of
        // "before the field existed", so this test asserts the pre- and
        // post-field encodings agree.
        let plan = PlanV1 {
            version: "1".into(),
            root: "app".into(),
            components: vec![ComponentSpec {
                id: "app".into(),
                digest: vec![0x11; 32],
                source: None,
            }],
            bindings: vec![],
            secrets: vec![],
            policy: Policy::default(),
            linkage: Default::default(),
            explicit_exports: vec![],
        };
        let bytes = serialize(&plan).unwrap();
        // Hex-decoded canonical encoding of `hello-plan.cbor` (checked in
        // under conformance/vectors, sha256 `d5cff6…`). Any drift here
        // means the empty-list serde skip is broken — a real wire break.
        let expected = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/vectors/hello-plan.cbor"
        ))
        .expect("read hello-plan.cbor");
        assert_eq!(
            bytes, expected,
            "canonical encoding for empty explicit_exports must match pre-field vector"
        );
    }

    #[test]
    fn explicit_exports_round_trip_preserves_entries() {
        use crate::types::ExplicitExport;
        let mut plan = create_test_plan();
        plan.explicit_exports = vec![
            ExplicitExport {
                source_instance: "root".into(),
                interface_name: "sqlite:extension/types@0.1.0".into(),
            },
            ExplicitExport {
                source_instance: "root".into(),
                interface_name: "sqlink:wasm/dispatch-bridge@0.1.0".into(),
            },
        ];
        let bytes = serialize(&plan).unwrap();
        let restored = deserialize(&bytes).unwrap();
        assert_eq!(plan.explicit_exports, restored.explicit_exports);
    }

    #[test]
    fn explicit_exports_referencing_unknown_component_are_rejected() {
        use crate::types::ExplicitExport;
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let validator = PlanValidator::new(blobs);
        let mut plan = create_test_plan();
        plan.explicit_exports = vec![ExplicitExport {
            source_instance: "ghost".into(),
            interface_name: "foo:bar/baz@0.1.0".into(),
        }];
        let err = validator.validate_structure(&plan).unwrap_err();
        assert!(matches!(err.code, ErrorCode::PlanInvalidGraph), "{:?}", err);
    }

    #[test]
    fn duplicate_explicit_exports_are_rejected() {
        use crate::types::ExplicitExport;
        let dir = tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();
        let validator = PlanValidator::new(blobs);
        let mut plan = create_test_plan();
        plan.explicit_exports = vec![
            ExplicitExport {
                source_instance: "root".into(),
                interface_name: "foo:bar/baz@0.1.0".into(),
            },
            ExplicitExport {
                source_instance: "root".into(),
                interface_name: "foo:bar/baz@0.1.0".into(),
            },
        ];
        let err = validator.validate_structure(&plan).unwrap_err();
        assert!(
            matches!(err.code, ErrorCode::PlanInvalidSchema),
            "{:?}",
            err
        );
    }
}
