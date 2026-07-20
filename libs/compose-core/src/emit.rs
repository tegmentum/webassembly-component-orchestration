/// Composition and artifact emission using wasm-tools
use crate::blobs::BlobStore;
use crate::events::EventCollector;
use crate::plan::PlanValidator;
use crate::types::{CompositionResult, Digest, Error, ErrorCode, PlanV1};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use wasm_compose::composer::ComponentComposer;
use wasm_compose::config::{Config, Dependency, Instantiation};
use wasmparser::{Validator, WasmFeatures};

// WIT-level composition via wac-graph. wasm-compose's wiring loses
// resource type identity across plug/socket boundaries for components
// that import resource-bearing interfaces (e.g. openssl:component/tls,
// sqlite:wasm/high-level) — the resulting composed component fails
// wasmparser validation with "resource types are not the same". wac
// understands WIT semantics and threads the right type identities
// through, so this is the correct primitive for composition.
use wac_graph::{CompositionGraph, EncodeOptions, NodeId, PackageId};
use wac_types::{are_semver_compatible, Package, SubtypeChecker};

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
        std::fs::read(&cache_path).ok().filter(|d| d.len() == 32)
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
        self.cache_dir
            .join("emit")
            .join(&hex_key[..2])
            .join(&hex_key[2..])
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

        // Step 2: If there's only one component (the root), no bindings,
        // and no explicit re-exports, return it directly without
        // composition. Explicit re-exports need the wac-graph path so
        // each named interface is validated against the root's real
        // exports even when there's nothing to wire.
        if plan.components.len() == 1
            && plan.bindings.is_empty()
            && plan.explicit_exports.is_empty()
        {
            self.events
                .info("single component, no composition needed", None);
            return component_map.remove(&plan.root).ok_or_else(|| {
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

        // No bindings and no explicit re-exports → return root unchanged.
        if plan.bindings.is_empty() && plan.explicit_exports.is_empty() {
            return Ok(root_bytes.clone());
        }

        // If the plan needs explicit re-exports we must use the wac-graph
        // library path: neither `wac plug` nor `wasm-compose` can express
        // "alias-export this plug's interface at the composed component's
        // outer world" as a build primitive.
        if !plan.explicit_exports.is_empty() {
            return self.compose_with_wac_graph(plan, component_map);
        }

        // Use wac-graph's plug primitive (WIT-level wiring that preserves
        // resource type identity). Falls back to the old wasm-compose
        // wrapper if wac-plug returns NoPlugHappened — that happens
        // when none of the plug exports match any socket imports by
        // interface name, which is unusual but possible.
        match self.compose_with_wac_plug(plan, component_map) {
            Ok(bytes) => Ok(bytes),
            Err(e) => {
                self.events.info(
                    "wac-plug composition failed; falling back to wasm-compose wrapper",
                    Some(format!("{:?}", e.code)),
                );
                self.compose_with_wrapper(plan, component_map, root_bytes)
            }
        }
    }

    /// Compose using the `wac` CLI as a subprocess.
    ///
    /// Why not the library: `wac-graph::plug` handles plug→socket wiring
    /// correctly but doesn't iterate plug→plug edges. A plan with
    /// `arrow-csv-typed` (which imports zstd:compression/simple) and
    /// `zstd-wasm` (which exports it) leaves the arrow plug's zstd
    /// import unwired — and the encoder auto-promotes that to a
    /// component-level import of the composed output. wasmtime then
    /// traps at instantiation with "matching implementation was not
    /// found in the linker" because the host doesn't satisfy it.
    ///
    /// The `wac` CLI tool resolves this through multi-pass plug
    /// processing — it iterates until no plug's imports remain
    /// satisfiable by other plugs' exports. The cleanest way to share
    /// that logic without forking wac-graph is to invoke `wac plug` as
    /// a subprocess. Yes, it's an exec boundary; the runtime cost is
    /// negligible vs the composition itself (linker is the bottleneck).
    ///
    /// Falls back to the library implementation if `wac` is not on
    /// PATH — useful for tests / environments where we want
    /// deterministic behavior even though it doesn't handle inter-plug
    /// wiring.
    fn compose_with_wac_plug(
        &self,
        plan: &PlanV1,
        component_map: &HashMap<String, Vec<u8>>,
    ) -> Result<Vec<u8>, Error> {
        // Prefer the wac CLI if it's available — it handles inter-plug
        // wiring that the wac-graph library doesn't yet.
        if std::process::Command::new("wac")
            .arg("--version")
            .output()
            .is_ok()
        {
            return self.compose_with_wac_cli(plan, component_map);
        }
        self.compose_with_wac_graph(plan, component_map)
    }

    fn compose_with_wac_cli(
        &self,
        plan: &PlanV1,
        component_map: &HashMap<String, Vec<u8>>,
    ) -> Result<Vec<u8>, Error> {
        use std::process::Command;

        let work = StagingDir::new(&self.cache_dir)?;

        // Stage socket
        let socket_bytes = component_map.get(&plan.root).ok_or_else(|| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("root component {} not found", plan.root),
            )
        })?;
        let socket_path = work.path.join("socket.wasm");
        std::fs::write(&socket_path, socket_bytes).map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("stage socket: {}", e),
            )
        })?;

        // Stage plugs (one file per unique provider; same dedup as the
        // library path).
        let mut plug_paths: Vec<PathBuf> = Vec::new();
        let mut staged: HashSet<String> = HashSet::new();
        for binding in &plan.bindings {
            if !staged.insert(binding.provider_id.clone()) || binding.provider_id == plan.root {
                continue;
            }
            let dep_bytes = component_map.get(&binding.provider_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!("provider component {} not found", binding.provider_id),
                )
            })?;
            let dep_path = work.path.join(format!("plug_{}.wasm", binding.provider_id));
            std::fs::write(&dep_path, dep_bytes).map_err(|e| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!("stage plug {}: {}", binding.provider_id, e),
                )
            })?;
            plug_paths.push(dep_path);
        }

        let out_path = work.path.join("composed.wasm");
        let mut cmd = Command::new("wac");
        cmd.arg("plug").arg(&socket_path);
        for p in &plug_paths {
            cmd.arg("--plug").arg(p);
        }
        cmd.arg("-o").arg(&out_path);

        self.events.info(
            "invoking wac plug subprocess",
            Some(format!("plugs: {}", plug_paths.len())),
        );

        let out = cmd.output().map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("failed to spawn `wac plug`: {}", e),
            )
        })?;
        if !out.status.success() {
            return Err(Error::new(
                ErrorCode::EmitCompositionFailed,
                format!(
                    "wac plug failed (exit {}): {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                ),
            ));
        }
        let bytes = std::fs::read(&out_path).map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("read composed output: {}", e),
            )
        })?;
        self.events.info(
            "wac plug subprocess complete",
            Some(format!("output size: {} bytes", bytes.len())),
        );
        Ok(bytes)
    }

    fn compose_with_wac_graph(
        &self,
        plan: &PlanV1,
        component_map: &HashMap<String, Vec<u8>>,
    ) -> Result<Vec<u8>, Error> {
        let mut graph = CompositionGraph::new();

        // Register the socket (root component).
        let socket_bytes = component_map.get(&plan.root).ok_or_else(|| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("root component {} not found", plan.root),
            )
        })?;
        let socket_pkg = Package::from_bytes(
            &plan.root,
            None::<&semver::Version>,
            socket_bytes.clone(),
            graph.types_mut(),
        )
        .map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("wac: register socket package {}: {}", plan.root, e),
            )
        })?;
        let socket_id = graph.register_package(socket_pkg).map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("wac: register socket {}: {}", plan.root, e),
            )
        })?;

        // Register each provider as a plug. Order is preserved so
        // composition is deterministic. Skip the root if it appears
        // again (a degenerate plan shape). Track the PackageId keyed
        // by component id so the explicit-export pass can look up the
        // corresponding instantiation.
        let mut plug_packages: Vec<(String, PackageId)> = Vec::new();
        let mut staged: HashSet<String> = HashSet::new();
        for binding in &plan.bindings {
            if !staged.insert(binding.provider_id.clone()) || binding.provider_id == plan.root {
                continue;
            }
            let dep_bytes = component_map.get(&binding.provider_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!("provider component {} not found", binding.provider_id),
                )
            })?;
            let dep_pkg = Package::from_bytes(
                &binding.provider_id,
                None::<&semver::Version>,
                dep_bytes.clone(),
                graph.types_mut(),
            )
            .map_err(|e| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!("wac: register plug package {}: {}", binding.provider_id, e),
                )
            })?;
            let dep_id = graph.register_package(dep_pkg).map_err(|e| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!("wac: register plug {}: {}", binding.provider_id, e),
                )
            })?;
            plug_packages.push((binding.provider_id.clone(), dep_id));
        }

        // Some `explicit-exports` may name a component that has no
        // import binding in the plan (a "pure re-export" plug that
        // supplies nothing the root imports, but exports something the
        // outer world needs). Register those as plug packages too so
        // the explicit-export pass has instances to alias against.
        for ee in &plan.explicit_exports {
            if ee.source_instance == plan.root {
                // Root exports are auto-exported by the plug loop.
                continue;
            }
            if staged.insert(ee.source_instance.clone()) {
                let dep_bytes = component_map.get(&ee.source_instance).ok_or_else(|| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!(
                            "explicit-export source component {} not found",
                            ee.source_instance
                        ),
                    )
                })?;
                let dep_pkg = Package::from_bytes(
                    &ee.source_instance,
                    None::<&semver::Version>,
                    dep_bytes.clone(),
                    graph.types_mut(),
                )
                .map_err(|e| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!(
                            "wac: register explicit-export package {}: {}",
                            ee.source_instance, e
                        ),
                    )
                })?;
                let dep_id = graph.register_package(dep_pkg).map_err(|e| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!(
                            "wac: register explicit-export {}: {}",
                            ee.source_instance, e
                        ),
                    )
                })?;
                plug_packages.push((ee.source_instance.clone(), dep_id));
            }
        }

        // Inline the wac-graph `plug` helper so we can track the
        // per-plug instantiation NodeId — the higher-level `plug()` in
        // the crate hides it, but explicit-exports need it. Semantically
        // identical: instantiate the socket, then for each plug find
        // exports that satisfy socket imports (exact or semver-compat),
        // wire them, and finally auto-export the socket's exports.
        //
        // Tracks plug instantiations by component id. Only populated
        // for plugs whose exports the socket actually consumed — a plug
        // that connects nothing gets its NodeId only when an explicit
        // export needs it (populated lazily in the export loop below).
        let socket_instantiation = graph.instantiate(socket_id);
        let mut plug_instantiations: HashMap<String, NodeId> = HashMap::new();
        let mut any_plugged = false;

        for (plug_component_id, plug_pkg_id) in &plug_packages {
            let mut matches: Vec<(String, String)> = Vec::new();
            {
                let mut cache = Default::default();
                let mut checker = SubtypeChecker::new(&mut cache);
                let plug_ty = graph[*plug_pkg_id].ty();
                let socket_ty = graph[socket_id].ty();
                for (name, plug_ty_id) in &graph.types()[plug_ty].exports {
                    let matching_import = graph.types()[socket_ty]
                        .imports
                        .get(name)
                        .map(|ty| (name.clone(), ty))
                        .or_else(|| {
                            graph.types()[socket_ty]
                                .imports
                                .iter()
                                .find(|(import_name, _)| are_semver_compatible(name, import_name))
                                .map(|(import_name, ty)| (import_name.clone(), ty))
                        });
                    if let Some((socket_name, socket_ty_id)) = matching_import {
                        if checker
                            .is_subtype(*plug_ty_id, graph.types(), *socket_ty_id, graph.types())
                            .is_ok()
                        {
                            matches.push((name.clone(), socket_name));
                        }
                    }
                }
            }

            if matches.is_empty() {
                continue;
            }

            let plug_inst = graph.instantiate(*plug_pkg_id);
            plug_instantiations.insert(plug_component_id.clone(), plug_inst);
            any_plugged = true;
            for (plug_name, socket_name) in matches {
                let alias = graph
                    .alias_instance_export(plug_inst, &plug_name)
                    .map_err(|e| {
                        Error::new(
                            ErrorCode::EmitCompositionFailed,
                            format!(
                                "wac: alias {}::{} for wiring: {}",
                                plug_component_id, plug_name, e
                            ),
                        )
                    })?;
                graph
                    .set_instantiation_argument(socket_instantiation, &socket_name, alias)
                    .map_err(|e| {
                        Error::new(
                            ErrorCode::EmitCompositionFailed,
                            format!(
                                "wac: wire {} <- {}::{}: {}",
                                socket_name, plug_component_id, plug_name, e
                            ),
                        )
                    })?;
            }
        }

        if !any_plugged && plan.explicit_exports.is_empty() {
            // Same signal `wac_graph::plug` emits — nothing to compose.
            return Err(Error::new(
                ErrorCode::EmitCompositionFailed,
                "wac: no plug exports matched any socket imports",
            ));
        }

        // Auto-export the socket's exports at the outer world (same as
        // `plug()` does).
        let socket_export_names: Vec<String> = graph.types()[graph[socket_id].ty()]
            .exports
            .keys()
            .cloned()
            .collect();
        for name in socket_export_names {
            let alias = graph
                .alias_instance_export(socket_instantiation, &name)
                .map_err(|e| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!("wac: alias socket export {}: {}", name, e),
                    )
                })?;
            graph.export(alias, &name).map_err(|e| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!("wac: export socket {}: {}", name, e),
                )
            })?;
        }

        // Explicit re-exports: alias each named plug's specified
        // interface and add as a top-level export of the composed
        // component. Instantiate the plug lazily if it wasn't
        // instantiated during the wiring pass (a "pure re-export" that
        // supplies nothing the root imports).
        //
        // Duplicate names (whether a re-export collides with the
        // socket's own auto-export, or two explicit-exports name the
        // same interface) surface as an EmitCompositionFailed error
        // rather than silently overwriting.
        for ee in &plan.explicit_exports {
            if ee.source_instance == plan.root {
                // Root exports were already auto-exported. Nothing to do,
                // but validate the interface actually exists to catch
                // typos.
                let root_exports = &graph.types()[graph[socket_id].ty()].exports;
                if !root_exports.contains_key(ee.interface_name.as_str()) {
                    return Err(Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!(
                            "explicit-export {}::{} not found among root exports",
                            ee.source_instance, ee.interface_name
                        ),
                    ));
                }
                continue;
            }

            // Look up the plug PackageId. Must exist — we registered a
            // package for every explicit-export source above.
            let plug_pkg_id = plug_packages
                .iter()
                .find(|(id, _)| id == &ee.source_instance)
                .map(|(_, id)| *id)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!(
                            "explicit-export references unregistered component: {}",
                            ee.source_instance
                        ),
                    )
                })?;

            // Reuse an existing instantiation if the wiring pass
            // already created one for this plug; otherwise instantiate
            // it now.
            let plug_inst = *plug_instantiations
                .entry(ee.source_instance.clone())
                .or_insert_with(|| graph.instantiate(plug_pkg_id));

            let alias = graph
                .alias_instance_export(plug_inst, &ee.interface_name)
                .map_err(|e| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!(
                            "wac: alias explicit-export {}::{}: {}",
                            ee.source_instance, ee.interface_name, e
                        ),
                    )
                })?;
            graph.export(alias, &ee.interface_name).map_err(|e| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!(
                        "wac: export explicit-export {}::{}: {}",
                        ee.source_instance, ee.interface_name, e
                    ),
                )
            })?;
        }

        // Encode without wac's built-in validation — we validate the
        // output later in compose_internal so error paths funnel
        // through the same code.
        let composed = graph
            .encode(EncodeOptions {
                validate: false,
                ..Default::default()
            })
            .map_err(|e| {
                Error::new(
                    ErrorCode::EmitCompositionFailed,
                    format!("wac encode failed: {}", e),
                )
            })?;

        self.events.info(
            "wac-plug composition complete",
            Some(format!(
                "plugs: {}, explicit-exports: {}, output size: {} bytes",
                staged.len(),
                plan.explicit_exports.len(),
                composed.len()
            )),
        );

        Ok(composed)
    }

    /// Create a composed component by wiring the root's imports to provider
    /// components, honoring each binding.
    ///
    /// `BytesComponentComposer` only instantiates the root and never wires its
    /// dependencies (its own resolution is, per upstream, "tied to the file
    /// system"), so the import stays unsatisfied and the provider is left out of
    /// the output. We instead stage the components to a temporary directory and
    /// drive the file-based `ComponentComposer`, which performs full dependency
    /// resolution via `CompositionGraphBuilder`: each binding becomes an
    /// explicit instantiation mapping the consumer's import instance to a named
    /// provider component, whose type-compatible export is connected to the
    /// import. `compose()` errors (rather than silently returning the bare root)
    /// when no dependency resolves, so an unwired plan now fails loudly.
    fn compose_with_wrapper(
        &self,
        plan: &PlanV1,
        component_map: &HashMap<String, Vec<u8>>,
        root_bytes: &[u8],
    ) -> Result<Vec<u8>, Error> {
        // Stage components under a unique, self-cleaning directory beneath the
        // cache dir (a known-writable location). Removed on drop.
        let work = StagingDir::new(&self.cache_dir)?;

        let root_path = work.path.join("root.wasm");
        std::fs::write(&root_path, root_bytes).map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("failed to stage root component: {}", e),
            )
        })?;

        let mut config = Config {
            dir: work.path.clone(),
            ..Config::default()
        };

        // Stage each provider once (keyed by provider id) and register it as a
        // named dependency; map every import to its provider via an explicit
        // instantiation so resolution is deterministic rather than relying on
        // wasm-compose's name auto-matching.
        let mut staged: HashSet<String> = HashSet::new();
        for (i, binding) in plan.bindings.iter().enumerate() {
            if staged.insert(binding.provider_id.clone()) {
                let dep_bytes = component_map.get(&binding.provider_id).ok_or_else(|| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!("provider component {} not found", binding.provider_id),
                    )
                })?;

                let file_name = format!("dep_{}.wasm", i);
                std::fs::write(work.path.join(&file_name), dep_bytes).map_err(|e| {
                    Error::new(
                        ErrorCode::EmitCompositionFailed,
                        format!("failed to stage provider {}: {}", binding.provider_id, e),
                    )
                })?;

                config.dependencies.insert(
                    binding.provider_id.clone(),
                    Dependency {
                        path: file_name.into(),
                    },
                );

                self.events.trace(
                    "staged provider",
                    Some(format!(
                        "id: {}, size: {} bytes",
                        binding.provider_id,
                        dep_bytes.len()
                    )),
                );
            }

            // Route the consumer's import instance to the provider component.
            // The provider's type-compatible export is selected automatically,
            // satisfying `export_name`.
            config.instantiations.insert(
                binding.import_name.clone(),
                Instantiation {
                    dependency: Some(binding.provider_id.clone()),
                    ..Default::default()
                },
            );

            self.events.trace(
                "wiring binding",
                Some(format!(
                    "import: {} -> provider: {} export: {}",
                    binding.import_name, binding.provider_id, binding.export_name
                )),
            );
        }

        self.events.info(
            "performing static composition",
            Some(format!("dependencies: {}", staged.len())),
        );

        let composed = ComponentComposer::new(root_path.as_path(), &config)
            .compose()
            .map_err(|e| {
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

/// A staging directory for file-based composition, removed when dropped.
struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    /// Create a unique staging directory beneath `base`.
    fn new(base: &Path) -> Result<Self, Error> {
        let mut nonce = [0u8; 8];
        getrandom::fill(&mut nonce)
            .map_err(|e| Error::new(ErrorCode::InternalError, format!("entropy failure: {}", e)))?;
        let path = base.join("compose-tmp").join(hex::encode(nonce));
        std::fs::create_dir_all(&path).map_err(|e| {
            Error::new(
                ErrorCode::EmitCompositionFailed,
                format!("failed to create staging dir: {}", e),
            )
        })?;
        Ok(Self { path })
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        // Preserve the staging dir for post-mortem debugging when
        // COMPOSE_KEEP_STAGING is set in the environment. Useful when
        // wasm-compose fails parsing one of the dep_NN files and you
        // want to inspect the actual bytes.
        if std::env::var_os("COMPOSE_KEEP_STAGING").is_some() {
            eprintln!("compose-tmp preserved at: {}", self.path.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.path);
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
            linkage: Default::default(),
            explicit_exports: vec![],
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
            linkage: Default::default(),
            explicit_exports: vec![],
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
            linkage: Default::default(),
            explicit_exports: vec![],
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
            linkage: Default::default(),
            explicit_exports: vec![],
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
            linkage: Default::default(),
            explicit_exports: vec![],
        };

        // Should fail - provider doesn't exist
        let result = emit.compose(&bad_plan);
        assert!(result.is_err());

        // Test with existing provider - should succeed
        let good_plan = PlanV1 {
            version: "1".to_string(),
            root: "root".to_string(),
            components: vec![
                // Components must be in canonical (ascending id) order.
                ComponentSpec {
                    id: "dependency".to_string(),
                    digest: dep_digest,
                    source: None,
                },
                ComponentSpec {
                    id: "root".to_string(),
                    digest: root_digest.clone(),
                    source: None,
                },
            ],
            bindings: vec![], // No bindings to avoid cycle detection issues
            secrets: vec![],
            policy: Policy::default(),
            linkage: Default::default(),
            explicit_exports: vec![],
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
