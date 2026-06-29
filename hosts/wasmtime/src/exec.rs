/// Execution and reflection APIs
use compose_core::audit::AuditLogger;
use compose_core::blobs::BlobStore;
use compose_core::emit::EmitHandler;
use compose_core::events::EventCollector;
use compose_core::metrics::{MetricLabel, MetricsCollector};
use compose_core::policy::PolicyEnforcer;
use compose_core::types::{
    Digest, Error, ErrorCode, ExecResult, ExportInfo, HttpRequest, HttpResponse, PlanV1,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use wasmtime::{
    component::{Component, Linker},
    Engine, Store,
};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{DirPerms, FilePerms, ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// A host-to-guest filesystem preopen for `run_cli_with_mounts`. Mirrors
/// `wasmtime run --dir HOST::GUEST` semantics: the host directory is opened
/// and made available to the wasm under the guest-side path. Permissions
/// default to all but can be tightened (e.g. for read-only mounts).
#[derive(Debug, Clone)]
pub struct Mount {
    /// Host filesystem path that backs the preopen.
    pub host_path: std::path::PathBuf,
    /// Guest-visible directory name (e.g. "/", "/site-packages").
    pub guest_path: String,
    /// Directory operations permitted on the mount.
    pub dir_perms: DirPerms,
    /// File operations permitted within the mount.
    pub file_perms: FilePerms,
}

impl Mount {
    /// Convenience: a read/write mount at the given guest path.
    pub fn rw(host_path: impl Into<std::path::PathBuf>, guest_path: impl Into<String>) -> Self {
        Self {
            host_path: host_path.into(),
            guest_path: guest_path.into(),
            dir_perms: DirPerms::all(),
            file_perms: FilePerms::all(),
        }
    }
}

/// Fold the set of providers linked at runtime into an exec-key hasher.
///
/// No-op when empty, so static-composition exec-keys are unchanged from
/// before runtime linking existed. The `BTreeSet` is iterated in sorted
/// order for determinism, and a domain separator guards against collision
/// with the static portion of the key.
fn fold_resolved_providers(hasher: &mut sha2::Sha256, resolved: &BTreeSet<Digest>) {
    use sha2::Digest as _;
    if resolved.is_empty() {
        return;
    }
    hasher.update(b"dynlink-resolved:v1");
    for digest in resolved {
        hasher.update(digest);
    }
}

/// Host state for WASI execution
struct HostState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

/// Exec handler for execution and reflection
pub struct ExecHandler {
    engine: Engine,
    blobs: BlobStore,
    emit: EmitHandler,
    events: EventCollector,
    cache_dir: PathBuf,
    policy_enforcer: PolicyEnforcer,
    audit_logger: AuditLogger,
    metrics: MetricsCollector,
    trust: compose_core::trust::TrustStore,
}

impl ExecHandler {
    /// Create a new exec handler
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        engine: Engine,
        blobs: BlobStore,
        emit: EmitHandler,
        events: EventCollector,
        cache_dir: PathBuf,
        policy_enforcer: PolicyEnforcer,
        audit_logger: AuditLogger,
        metrics: MetricsCollector,
        trust: compose_core::trust::TrustStore,
    ) -> Self {
        Self {
            engine,
            blobs,
            emit,
            events,
            cache_dir,
            policy_enforcer,
            audit_logger,
            metrics,
            trust,
        }
    }

    /// Execute plan as a CLI application
    pub fn run_cli(
        &self,
        plan: &PlanV1,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<ExecResult, Error> {
        self.run_cli_with_mounts(plan, args, env, Vec::new())
    }

    /// Execute plan as a CLI application, with host filesystem preopens.
    ///
    /// Mirrors `wasmtime run --dir` — each Mount maps a host path to a
    /// guest-visible directory. Required for guests like pylon's python.wasm
    /// that need a real stdlib + sysconfig + site-packages tree under `/`.
    /// `Linkage::Runtime` plans currently ignore mounts (the dynlink exec
    /// path doesn't thread them through yet — separate change).
    pub fn run_cli_with_mounts(
        &self,
        plan: &PlanV1,
        _args: Vec<String>,
        _env: Vec<(String, String)>,
        mounts: Vec<Mount>,
    ) -> Result<ExecResult, Error> {
        // Runtime-linked plans (flavor A) take a separate path: the root
        // consumer's endpoint import is bound to a provider at exec time
        // rather than statically composed.
        if plan.linkage == compose_core::types::Linkage::Runtime {
            if !mounts.is_empty() {
                self.events.warn(
                    "mounts ignored for runtime-linked plan",
                    Some("Linkage::Runtime exec path does not thread mounts yet".to_string()),
                );
            }
            return self.run_cli_runtime_linked(plan, _args, _env);
        }

        let start_time = std::time::Instant::now();

        self.events
            .info("executing plan as CLI", Some(format!("args: {:?}", _args)));

        // Enforce policy before execution
        let enforced_policy = self
            .policy_enforcer
            .enforce_policy(&plan.policy)
            .map_err(|e| {
                self.events
                    .error("policy enforcement failed", Some(e.to_string()));
                Error::new(
                    ErrorCode::PolicyViolation,
                    format!("policy enforcement failed: {}", e),
                )
            })?;

        if !enforced_policy.denied_optional.is_empty() {
            self.events.warn(
                "optional capabilities denied",
                Some(format!(
                    "denied: {}",
                    enforced_policy.denied_optional.join(", ")
                )),
            );
        }

        // Compose the plan first
        let composition = self.emit.compose(plan)?;

        // Compute exec key. The static composition path resolves no
        // providers at runtime, so the resolved set is empty here and the
        // key is identical to the pre-dynlink computation. The runtime-
        // linking exec path (flavor A) passes the plan's bound provider
        // digests instead.
        let exec_key = self.compute_exec_key(plan, &composition.digest, &BTreeSet::new())?;
        let plan_digest = self.compute_plan_digest(plan)?;

        // Check cache
        if let Some(cached_result) = self.check_cache(&exec_key) {
            self.events.info("execution cache hit", None);
            // Log cache hit
            let _ = self.audit_logger.log_exec(
                &plan_digest,
                &exec_key,
                enforced_policy.tenant_id(),
                "success (cached)",
                Some(cached_result.exit_code),
            );
            return Ok(cached_result);
        }

        // Load the composed component
        let component_bytes = self.blobs.get(&composition.digest)?;
        let component = Component::new(&self.engine, &component_bytes).map_err(|e| {
            let err = Error::new(
                ErrorCode::InternalError,
                format!("failed to load component: {}", e),
            );
            // Log failure
            let _ = self.audit_logger.log_exec(
                &plan_digest,
                &exec_key,
                enforced_policy.tenant_id(),
                "failure: load component",
                None,
            );
            err
        })?;

        // Execute the component with WASI support
        self.events.info("executing component with WASI", None);

        let result = self
            .execute_wasi_command(&component, _args, _env, mounts, &exec_key)
            .map_err(|e| {
                let err = Error::new(ErrorCode::ExecTrap, format!("execution failed: {}", e));
                // Log failure
                let _ = self.audit_logger.log_exec(
                    &plan_digest,
                    &exec_key,
                    enforced_policy.tenant_id(),
                    &format!("failure: {}", e),
                    None,
                );
                err
            })?;

        // Update cache
        self.update_cache(&exec_key, &result)?;

        // Log successful execution
        let _ = self.audit_logger.log_exec(
            &plan_digest,
            &exec_key,
            enforced_policy.tenant_id(),
            "success",
            Some(result.exit_code),
        );

        self.events.info(
            "execution complete",
            Some(format!("exit_code: {}", result.exit_code)),
        );

        // Record execution metrics
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let labels = vec![
            MetricLabel {
                key: "operation".to_string(),
                value: "exec".to_string(),
            },
            MetricLabel {
                key: "tenant".to_string(),
                value: enforced_policy.tenant_id().unwrap_or("default").to_string(),
            },
        ];

        self.metrics
            .timer("exec.duration_ms", duration_ms, labels.clone());
        self.metrics.counter("exec.total", 1, labels.clone());

        if result.exit_code == 0 {
            self.metrics.counter("exec.success", 1, labels);
        } else {
            self.metrics.counter("exec.failure", 1, labels);
        }

        Ok(result)
    }

    /// Execute a runtime-linked plan (flavor A): run the root consumer as
    /// a CLI command with its single `compose:dynlink/endpoint` import
    /// bound to a provider resolved at exec time. The provider is trust-
    /// gated and its digest is folded into the exec-key so the cache only
    /// hits when the same provider is linked.
    fn run_cli_runtime_linked(
        &self,
        plan: &PlanV1,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<ExecResult, Error> {
        let start_time = std::time::Instant::now();
        self.events
            .info("executing runtime-linked plan as CLI", None);

        let enforced_policy = self
            .policy_enforcer
            .enforce_policy(&plan.policy)
            .map_err(|e| {
                Error::new(
                    ErrorCode::PolicyViolation,
                    format!("policy enforcement failed: {}", e),
                )
            })?;

        // Capability gate: runtime linking requires both dynlink verbs to be
        // granted (declared by the plan and permitted by the host).
        if !enforced_policy.has_capability(crate::dynlink::CAP_RESOLVE)
            || !enforced_policy.has_capability(crate::dynlink::CAP_INVOKE)
        {
            return Err(Error::new(
                ErrorCode::ExecCapabilityDenied,
                format!(
                    "runtime linking requires the '{}' and '{}' capabilities",
                    crate::dynlink::CAP_RESOLVE,
                    crate::dynlink::CAP_INVOKE
                ),
            ));
        }

        // Locate the root component.
        let consumer_digest = plan
            .components
            .iter()
            .find(|c| c.id == plan.root)
            .map(|c| c.digest.clone())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::PlanInvalidGraph,
                    "root component not found in plan",
                )
            })?;

        // Flavor B: if the root imports `compose:dynlink/linker`, it drives
        // linking itself (dlopen by id/digest) rather than via a plan binding.
        let root_bytes = self.blobs.get(&consumer_digest)?;
        let root_component = Component::new(&self.engine, &root_bytes).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to load root component: {}", e),
            )
        })?;
        if crate::dynlink::imports_linker(&self.engine, &root_component) {
            return self.run_cli_dlopen(plan, &root_bytes, &enforced_policy, args, env);
        }

        let binding = match plan.bindings.as_slice() {
            [b] => b,
            [] => {
                return Err(Error::new(
                    ErrorCode::PlanInvalidGraph,
                    "runtime linkage requires exactly one endpoint binding",
                ))
            }
            _ => {
                // A consumer imports `compose:dynlink/endpoint` exactly once,
                // so a single plan binding satisfies it. Multiple providers
                // are served by flavor B: the guest resolves them on demand
                // via `compose:dynlink/linker` (resolve-by-id/digest).
                return Err(Error::new(
                    ErrorCode::PlanInvalidGraph,
                    "runtime linkage binds one endpoint provider per plan; \
                     use guest-driven linking (compose:dynlink/linker) for multiple providers",
                ));
            }
        };

        let provider_digest = plan
            .components
            .iter()
            .find(|c| c.id == binding.provider_id)
            .map(|c| c.digest.clone())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::PlanInvalidGraph,
                    format!("provider '{}' not found in plan", binding.provider_id),
                )
            })?;

        // Trust-gate the provider before it is loaded or instantiated.
        self.trust
            .verify_digest(&provider_digest)
            .map_err(|e| Error::new(ErrorCode::TrustUntrustedSource, e.to_string()))?;

        let consumer_bytes = self.blobs.get(&consumer_digest)?;
        let provider_bytes = self.blobs.get(&provider_digest)?;

        // Fold the resolved provider into the exec-key; the consumer digest
        // stands in for the (non-existent) composed artifact.
        let mut resolved = BTreeSet::new();
        resolved.insert(provider_digest.clone());
        let exec_key = self.compute_exec_key(plan, &consumer_digest, &resolved)?;
        let plan_digest = self.compute_plan_digest(plan)?;

        if let Some(cached) = self.check_cache(&exec_key) {
            self.events
                .info("execution cache hit (runtime-linked)", None);
            let _ = self.audit_logger.log_exec(
                &plan_digest,
                &exec_key,
                enforced_policy.tenant_id(),
                "success (cached, runtime-linked)",
                Some(cached.exit_code),
            );
            return Ok(cached);
        }

        let out = crate::dynlink::run_cli_with_endpoint(
            &self.engine,
            &consumer_bytes,
            &provider_bytes,
            &args,
            &env,
        )?;

        let result = ExecResult {
            exit_code: out.exit_code,
            stdout: out.stdout,
            stderr: out.stderr,
            exec_key: exec_key.clone(),
        };

        self.update_cache(&exec_key, &result)?;
        // Audit the run, recording the resolved provider so the dynamic
        // linking decision is captured in the tamper-evident log.
        let _ = self.audit_logger.log_exec(
            &plan_digest,
            &exec_key,
            enforced_policy.tenant_id(),
            &format!(
                "success (runtime-linked, provider={})",
                hex::encode(&provider_digest)
            ),
            Some(result.exit_code),
        );

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let labels = vec![
            MetricLabel {
                key: "operation".to_string(),
                value: "exec.runtime_linked".to_string(),
            },
            MetricLabel {
                key: "tenant".to_string(),
                value: enforced_policy.tenant_id().unwrap_or("default").to_string(),
            },
        ];
        self.metrics
            .timer("exec.duration_ms", duration_ms, labels.clone());
        self.metrics.counter("exec.total", 1, labels);

        Ok(result)
    }

    /// Execute a guest-driven runtime-linked plan (flavor B): the root
    /// component imports `compose:dynlink/linker` and resolves providers by
    /// id/digest at run time. The plan's components form the id→digest
    /// registry; the host's trust store and the plan's granted capabilities
    /// gate each resolution. Not cached — the resolved set is only known
    /// after the run — but it is recorded in the audit log.
    fn run_cli_dlopen(
        &self,
        plan: &PlanV1,
        root_bytes: &[u8],
        enforced_policy: &compose_core::policy::EnforcedPolicy,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<ExecResult, Error> {
        let start_time = std::time::Instant::now();
        self.events
            .info("executing runtime-linked plan (guest-driven)", None);

        // Hybrid path: when the plan has both `compose:dynlink/linker`-
        // driven loading AND static cap bindings (e.g. pylon's
        // python.wasm imports the linker for crc32c-style dynamic
        // packages, while ALSO importing tegmentum:aead-multiplexer,
        // openssl:component/*, sqlite:wasm/*, etc. that need static
        // composition), compose the static portion first so those caps
        // are satisfied, then feed the composed bytes through the
        // dlopen path which adds the linker. Without this, dlopen
        // traps at instantiation: "matching implementation was not
        // found in the linker" for every non-dynlink cap import.
        let guest_bytes_owned;
        let guest_bytes: &[u8] = if plan.bindings.is_empty() {
            root_bytes
        } else {
            self.events.info(
                "hybrid dlopen: composing static bindings first",
                Some(format!("bindings: {}", plan.bindings.len())),
            );
            let composition = self.emit.compose(plan)?;
            guest_bytes_owned = self.blobs.get(&composition.digest)?;
            guest_bytes_owned.as_slice()
        };

        let registry: Vec<(String, Digest)> = plan
            .components
            .iter()
            .map(|c| (c.id.clone(), c.digest.clone()))
            .collect();
        let granted: BTreeSet<String> = enforced_policy
            .capabilities
            .iter()
            .map(|c| c.name.clone())
            .collect();

        let (out, resolved) = crate::dynlink::run_cli_dlopen(
            &self.engine,
            guest_bytes,
            &registry,
            self.blobs.clone(),
            self.trust.clone(),
            plan.policy.determinism,
            granted,
            &args,
            &env,
        )?;

        // The exec-key folds the providers the guest actually resolved (for
        // traceability/audit). There is no composed artifact, so the
        // artifact slot is empty.
        let exec_key = self.compute_exec_key(plan, &Vec::new(), &resolved)?;
        let plan_digest = self.compute_plan_digest(plan)?;
        let resolved_hex = resolved
            .iter()
            .map(hex::encode)
            .collect::<Vec<_>>()
            .join(",");
        let _ = self.audit_logger.log_exec(
            &plan_digest,
            &exec_key,
            enforced_policy.tenant_id(),
            &format!("success (runtime-linked, guest-driven, resolved=[{resolved_hex}])"),
            Some(out.exit_code),
        );

        let result = ExecResult {
            exit_code: out.exit_code,
            stdout: out.stdout,
            stderr: out.stderr,
            exec_key,
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let labels = vec![
            MetricLabel {
                key: "operation".to_string(),
                value: "exec.dlopen".to_string(),
            },
            MetricLabel {
                key: "tenant".to_string(),
                value: enforced_policy.tenant_id().unwrap_or("default").to_string(),
            },
        ];
        self.metrics
            .timer("exec.duration_ms", duration_ms, labels.clone());
        self.metrics.counter("exec.total", 1, labels);

        Ok(result)
    }

    /// Invoke a specific exported function by name
    pub fn invoke(&self, plan: &PlanV1, export_name: &str, args: &[u8]) -> Result<Vec<u8>, Error> {
        self.events.info(
            "invoking function",
            Some(format!("export: {}", export_name)),
        );

        // Compose the plan first
        let composition = self.emit.compose(plan)?;

        // Load the composed component
        let component_bytes = self.blobs.get(&composition.digest)?;
        let component = Component::new(&self.engine, &component_bytes).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to load component: {}", e),
            )
        })?;

        // Create a linker with WASI support
        let mut linker = Linker::<HostState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to add WASI to linker: {}", e),
            )
        })?;

        // Build minimal WASI context
        let wasi_ctx = WasiCtxBuilder::new().build();
        let host_state = HostState {
            wasi_ctx,
            wasi_table: ResourceTable::new(),
        };

        // Create a store
        let mut store = Store::new(&self.engine, host_state);

        // Instantiate the component
        let instance = linker.instantiate(&mut store, &component).map_err(|e| {
            Error::new(
                ErrorCode::ExecTrap,
                format!("failed to instantiate component: {}", e),
            )
        })?;

        // Get the export
        let func = instance.get_func(&mut store, export_name).ok_or_else(|| {
            Error::new(
                ErrorCode::ExecMissingExport,
                format!("export '{}' not found", export_name),
            )
        })?;

        // Prepare parameters (deserialize from args bytes if needed)
        let params = if args.is_empty() {
            vec![]
        } else {
            // For now, we assume no parameters or handle bytes directly
            // Full implementation would deserialize based on function signature
            vec![]
        };

        // Prepare results buffer
        let mut results =
            vec![wasmtime::component::Val::Bool(false); func.ty(&store).results().len()];

        // Call the function
        func.call(&mut store, &params, &mut results).map_err(|e| {
            Error::new(
                ErrorCode::ExecTrap,
                format!("function invocation failed: {}", e),
            )
        })?;

        // Serialize results back to bytes
        // For now, return empty for void functions or serialize results
        let result_bytes = if results.is_empty() {
            vec![]
        } else {
            // Simple serialization - full implementation would use proper WIT types
            serde_json::to_vec(&results.len()).unwrap_or_default()
        };

        self.events.info(
            "function invoked successfully",
            Some(format!("result size: {} bytes", result_bytes.len())),
        );

        Ok(result_bytes)
    }

    /// Serve the plan's `wasi:http` component over HTTP on `port`.
    /// Requires the `http-server` feature; long-running (blocks).
    #[cfg(feature = "http-server")]
    pub fn serve_http(&self, plan: &PlanV1, port: u16) -> Result<(), Error> {
        let _ = self
            .policy_enforcer
            .enforce_policy(&plan.policy)
            .map_err(|e| Error::new(ErrorCode::PolicyViolation, format!("policy: {e}")))?;
        let composition = self.emit.compose(plan)?;
        let component_bytes = self.blobs.get(&composition.digest)?;
        self.events
            .info("serving HTTP", Some(format!("port: {port}")));
        crate::http::serve(&component_bytes, port)
    }

    /// Without the `http-server` feature, serve-http is unavailable.
    #[cfg(not(feature = "http-server"))]
    pub fn serve_http(&self, _plan: &PlanV1, _port: u16) -> Result<(), Error> {
        Err(Error::new(
            ErrorCode::NotImplemented,
            "serve-http requires the `http-server` feature",
        ))
    }

    /// Handle a single HTTP request with the plan's `wasi:http` component.
    /// Requires the `http-server` feature.
    #[cfg(feature = "http-server")]
    pub fn handle_http(&self, plan: &PlanV1, request: HttpRequest) -> Result<HttpResponse, Error> {
        let _ = self
            .policy_enforcer
            .enforce_policy(&plan.policy)
            .map_err(|e| Error::new(ErrorCode::PolicyViolation, format!("policy: {e}")))?;
        let composition = self.emit.compose(plan)?;
        let component_bytes = self.blobs.get(&composition.digest)?;
        self.events.info(
            "handling HTTP request",
            Some(format!(
                "method: {}, path: {}",
                request.method, request.path
            )),
        );
        crate::http::handle(&component_bytes, &request)
    }

    /// Without the `http-server` feature, handle-http is unavailable.
    #[cfg(not(feature = "http-server"))]
    pub fn handle_http(
        &self,
        _plan: &PlanV1,
        _request: HttpRequest,
    ) -> Result<HttpResponse, Error> {
        Err(Error::new(
            ErrorCode::NotImplemented,
            "handle-http requires the `http-server` feature",
        ))
    }

    /// List all exports from the root component
    pub fn list_exports(&self, plan: &PlanV1) -> Result<Vec<String>, Error> {
        self.events.info("listing exports", None);

        // Compose the plan first
        let _composition = self.emit.compose(plan)?;

        // For M3, return basic exports
        // Full introspection would require parsing the component or instantiating it
        // and enumerating its exports, which is complex with the component model
        let exports = vec!["wasi:cli/run@0.2.0".to_string()];

        self.events
            .info("exports listed", Some(format!("count: {}", exports.len())));

        Ok(exports)
    }

    /// Describe a specific export (get type signature)
    pub fn describe_export(&self, _plan: &PlanV1, export_name: &str) -> Result<ExportInfo, Error> {
        self.events.info(
            "describing export",
            Some(format!("export: {}", export_name)),
        );

        // For M3, return a basic type signature
        // Full implementation would parse WIT types
        Ok(ExportInfo {
            name: export_name.to_string(),
            type_sig: "func() -> ()".to_string(),
        })
    }

    /// Execute a WASI command component
    fn execute_wasi_command(
        &self,
        component: &Component,
        args: Vec<String>,
        env: Vec<(String, String)>,
        mounts: Vec<Mount>,
        exec_key: &Digest,
    ) -> Result<ExecResult, Error> {
        // Create a linker with WASI support
        let mut linker = Linker::<HostState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to add WASI to linker: {}", e),
            )
        })?;

        // Capture stdout and stderr
        let stdout = MemoryOutputPipe::new(4096);
        let stderr = MemoryOutputPipe::new(4096);
        let stdout_clone = stdout.clone();
        let stderr_clone = stderr.clone();

        // Build WASI context with args, env, mounts, and captured I/O
        let mut builder = WasiCtxBuilder::new();
        builder.args(&args);

        for (key, value) in env {
            builder.env(&key, &value);
        }

        for mount in &mounts {
            builder
                .preopened_dir(
                    &mount.host_path,
                    &mount.guest_path,
                    mount.dir_perms,
                    mount.file_perms,
                )
                .map_err(|e| {
                    Error::new(
                        ErrorCode::InternalError,
                        format!(
                            "failed to preopen {} as {}: {}",
                            mount.host_path.display(),
                            mount.guest_path,
                            e
                        ),
                    )
                })?;
        }

        builder
            .stdin(MemoryInputPipe::new(Vec::new()))
            .stdout(stdout)
            .stderr(stderr);

        let wasi_ctx = builder.build();
        let host_state = HostState {
            wasi_ctx,
            wasi_table: ResourceTable::new(),
        };

        // Create a store with the host state
        let mut store = Store::new(&self.engine, host_state);

        // Instantiate the command
        let command = Command::instantiate(&mut store, component, &linker).map_err(|e| {
            Error::new(
                ErrorCode::ExecTrap,
                format!("failed to instantiate command: {}", e),
            )
        })?;

        // Execute the command - returns Result<(), ()>
        let exit_code = match command.wasi_cli_run().call_run(&mut store).map_err(|e| {
            Error::new(
                ErrorCode::ExecTrap,
                format!("command execution failed: {}", e),
            )
        })? {
            Ok(()) => 0u32,
            Err(()) => 1u32,
        };

        // Drop store to release references to pipes
        drop(store);

        // Get captured output using content() method
        let stdout_bytes = stdout_clone.contents().to_vec();
        let stderr_bytes = stderr_clone.contents().to_vec();

        Ok(ExecResult {
            exit_code,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
            exec_key: exec_key.clone(),
        })
    }

    /// Check if an execution is cached by exec-key
    pub fn check_cache(&self, exec_key: &Digest) -> Option<ExecResult> {
        let cache_path = self.cache_key_path(exec_key);
        std::fs::read(&cache_path)
            .ok()
            .and_then(|bytes| ciborium::from_reader(&bytes[..]).ok())
    }

    /// Compute exec key for caching with tenant isolation
    /// exec_key = H(plan + digests + "exec:v1" + host_abi + caps + policy + tenant + limits + features + resolved)
    ///
    /// `resolved_providers` is the set of provider digests linked at
    /// runtime. It is empty for static composition (keeping those keys
    /// stable) and non-empty for runtime linking, where folding it in
    /// ensures a cache hit only when the exact same providers are linked.
    fn compute_exec_key(
        &self,
        plan: &PlanV1,
        artifact_digest: &Digest,
        resolved_providers: &BTreeSet<Digest>,
    ) -> Result<Digest, Error> {
        use sha2::{Digest as Sha2Digest, Sha256};

        let plan_validator = compose_core::plan::PlanValidator::new(self.blobs.clone());
        let plan_bytes = plan_validator.serialize(plan)?;

        let mut hasher = Sha256::new();

        // Plan and artifact
        hasher.update(&plan_bytes);
        hasher.update(artifact_digest);
        hasher.update(b"exec:v1");

        // Policy fingerprint
        let policy_bytes = self.serialize_policy_fingerprint(&plan.policy);
        hasher.update(&policy_bytes);

        // Tenant ID for isolation
        if let Some(tenant_id) = &plan.policy.tenant {
            hasher.update(b"tenant:");
            hasher.update(tenant_id.as_bytes());
        }

        // Resource limits
        if let Some(cpu_ms) = plan.policy.limits.cpu_ms {
            hasher.update(cpu_ms.to_le_bytes());
        }
        if let Some(memory_bytes) = plan.policy.limits.memory_bytes {
            hasher.update(memory_bytes.to_le_bytes());
        }
        if let Some(io_ops) = plan.policy.limits.io_ops {
            hasher.update(io_ops.to_le_bytes());
        }

        // Providers linked at runtime (empty for static composition).
        fold_resolved_providers(&mut hasher, resolved_providers);

        Ok(hasher.finalize().to_vec())
    }

    /// Serialize policy to bytes for fingerprinting
    fn serialize_policy_fingerprint(&self, policy: &compose_core::types::Policy) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Determinism mode
        bytes.push(match policy.determinism {
            compose_core::types::DeterminismMode::Strict => 0,
            compose_core::types::DeterminismMode::Audit => 1,
            compose_core::types::DeterminismMode::Relaxed => 2,
        });

        // Capabilities (sorted for determinism)
        let mut cap_names: Vec<_> = policy
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        cap_names.sort();
        for name in cap_names {
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0); // separator
        }

        bytes
    }

    /// Compute plan digest for audit logging
    fn compute_plan_digest(&self, plan: &PlanV1) -> Result<Digest, Error> {
        let plan_validator = compose_core::plan::PlanValidator::new(self.blobs.clone());
        plan_validator.compute_digest(plan)
    }

    /// Update cache with exec-key -> result mapping
    fn update_cache(&self, exec_key: &Digest, result: &ExecResult) -> Result<(), Error> {
        let cache_path = self.cache_key_path(exec_key);
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("failed to create cache directory: {}", e),
                )
            })?;
        }

        let mut bytes = Vec::new();
        ciborium::into_writer(result, &mut bytes).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to serialize exec result: {}", e),
            )
        })?;

        std::fs::write(&cache_path, bytes).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to write cache: {}", e),
            )
        })
    }

    /// Get cache file path for exec key
    fn cache_key_path(&self, exec_key: &Digest) -> PathBuf {
        let hex_key = hex::encode(exec_key);
        self.cache_dir
            .join("exec")
            .join(&hex_key[..2])
            .join(&hex_key[2..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};

    fn hash_with(resolved: &BTreeSet<Digest>) -> Digest {
        let mut h = Sha256::new();
        h.update(b"base");
        fold_resolved_providers(&mut h, resolved);
        h.finalize().to_vec()
    }

    #[test]
    fn empty_resolved_set_does_not_change_key() {
        // Static composition (empty set) must hash identically to not
        // folding at all, so existing exec-keys stay stable.
        let mut baseline = Sha256::new();
        baseline.update(b"base");
        assert_eq!(hash_with(&BTreeSet::new()), baseline.finalize().to_vec());
    }

    #[test]
    fn resolved_providers_change_the_key() {
        let empty = hash_with(&BTreeSet::new());
        let one: BTreeSet<Digest> = [vec![1u8; 32]].into_iter().collect();
        let two: BTreeSet<Digest> = [vec![1u8; 32], vec![2u8; 32]].into_iter().collect();

        assert_ne!(hash_with(&one), empty);
        assert_ne!(hash_with(&two), hash_with(&one));
    }

    #[test]
    fn resolved_set_order_is_deterministic() {
        // BTreeSet ordering means insertion order can't affect the key.
        let a: BTreeSet<Digest> = [vec![1u8; 32], vec![2u8; 32]].into_iter().collect();
        let b: BTreeSet<Digest> = [vec![2u8; 32], vec![1u8; 32]].into_iter().collect();
        assert_eq!(hash_with(&a), hash_with(&b));
    }
}
