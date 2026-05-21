/// Execution and reflection APIs
use crate::audit::AuditLogger;
use crate::blobs::BlobStore;
use crate::emit::EmitHandler;
use crate::events::EventCollector;
use crate::metrics::{MetricLabel, MetricsCollector};
use crate::policy::PolicyEnforcer;
use crate::types::{Digest, Error, ErrorCode, ExecResult, ExportInfo, HttpRequest, HttpResponse, PlanV1};
use std::path::PathBuf;
use wasmtime::{
    component::{Component, Linker},
    Engine, Store,
};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiView, WasiCtxView};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi_nn::wit::{WasiNnCtx, WasiNnView};
use wasmtime_wasi_nn::InMemoryRegistry;

/// Host state for WASI execution
struct HostState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
    wasi_nn_ctx: WasiNnCtx,
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
}

impl ExecHandler {
    /// Create a new exec handler
    pub fn new(
        engine: Engine,
        blobs: BlobStore,
        emit: EmitHandler,
        events: EventCollector,
        cache_dir: PathBuf,
        policy_enforcer: PolicyEnforcer,
        audit_logger: AuditLogger,
        metrics: MetricsCollector,
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
        }
    }

    /// Execute plan as a CLI application
    pub fn run_cli(
        &self,
        plan: &PlanV1,
        _args: Vec<String>,
        _env: Vec<(String, String)>,
    ) -> Result<ExecResult, Error> {
        let start_time = std::time::Instant::now();

        self.events.info(
            "executing plan as CLI",
            Some(format!("args: {:?}", _args)),
        );

        // Enforce policy before execution
        let enforced_policy = self.policy_enforcer.enforce_policy(&plan.policy).map_err(|e| {
            self.events.error(
                "policy enforcement failed",
                Some(e.to_string()),
            );
            Error::new(
                ErrorCode::PolicyViolation,
                format!("policy enforcement failed: {}", e),
            )
        })?;

        if !enforced_policy.denied_optional.is_empty() {
            self.events.warn(
                "optional capabilities denied",
                Some(format!("denied: {}", enforced_policy.denied_optional.join(", "))),
            );
        }

        // Compose the plan first
        let composition = self.emit.compose(plan)?;

        // Compute exec key
        let exec_key = self.compute_exec_key(plan, &composition.digest)?;
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

        let result = self.execute_wasi_command(&component, _args, _env, &exec_key).map_err(|e| {
            let err = Error::new(
                ErrorCode::ExecTrap,
                format!("execution failed: {}", e),
            );
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

        self.events.info("execution complete", Some(format!("exit_code: {}", result.exit_code)));

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

        self.metrics.timer("exec.duration_ms", duration_ms, labels.clone());
        self.metrics.counter("exec.total", 1, labels.clone());

        if result.exit_code == 0 {
            self.metrics.counter("exec.success", 1, labels);
        } else {
            self.metrics.counter("exec.failure", 1, labels);
        }

        Ok(result)
    }

    /// Invoke a specific exported function by name
    pub fn invoke(
        &self,
        plan: &PlanV1,
        export_name: &str,
        args: &[u8],
    ) -> Result<Vec<u8>, Error> {
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

        // Add wasi-nn support to linker
        wasmtime_wasi_nn::wit::add_to_linker(&mut linker, |state: &mut HostState| {
            WasiNnView::new(&mut state.wasi_table, &mut state.wasi_nn_ctx)
        })
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to add wasi-nn to linker: {}", e),
            )
        })?;

        // Build minimal WASI context
        let wasi_ctx = WasiCtxBuilder::new().build();
        let host_state = HostState {
            wasi_ctx,
            wasi_table: ResourceTable::new(),
            wasi_nn_ctx: WasiNnCtx::new([], InMemoryRegistry::new().into()),
        };

        // Create a store
        let mut store = Store::new(&self.engine, host_state);

        // Instantiate the component
        let instance = linker
            .instantiate(&mut store, &component)
            .map_err(|e| {
                Error::new(
                    ErrorCode::ExecTrap,
                    format!("failed to instantiate component: {}", e),
                )
            })?;

        // Get the export
        let func = instance
            .get_func(&mut store, export_name)
            .ok_or_else(|| {
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
        let mut results = vec![wasmtime::component::Val::Bool(false); func.ty(&store).results().len()];

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

    /// Serve HTTP requests (stub for M3)
    pub fn serve_http(&self, _plan: &PlanV1, port: u16) -> Result<(), Error> {
        self.events.warn(
            "HTTP server not implemented in M3",
            Some(format!("port: {}", port)),
        );
        Err(Error::new(
            ErrorCode::NotImplemented,
            "serve-http requires http-server feature",
        ))
    }

    /// Handle a single HTTP request
    pub fn handle_http(&self, _plan: &PlanV1, request: HttpRequest) -> Result<HttpResponse, Error> {
        self.events.info(
            "handling HTTP request",
            Some(format!("method: {}, path: {}", request.method, request.path)),
        );

        // For M3, return a basic response
        Ok(HttpResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
            body: b"Hello from sys:compose\n".to_vec(),
        })
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

        self.events.info(
            "exports listed",
            Some(format!("count: {}", exports.len())),
        );

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

        // Add wasi-nn support to linker
        wasmtime_wasi_nn::wit::add_to_linker(&mut linker, |state: &mut HostState| {
            WasiNnView::new(&mut state.wasi_table, &mut state.wasi_nn_ctx)
        })
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to add wasi-nn to linker: {}", e),
            )
        })?;

        // Capture stdout and stderr
        let stdout = MemoryOutputPipe::new(4096);
        let stderr = MemoryOutputPipe::new(4096);
        let stdout_clone = stdout.clone();
        let stderr_clone = stderr.clone();

        // Build WASI context with args, env, and captured I/O
        let mut builder = WasiCtxBuilder::new();
        builder.args(&args);

        for (key, value) in env {
            builder.env(&key, &value);
        }

        builder
            .stdin(MemoryInputPipe::new(Vec::new()))
            .stdout(stdout)
            .stderr(stderr);

        let wasi_ctx = builder.build();
        let host_state = HostState {
            wasi_ctx,
            wasi_table: ResourceTable::new(),
            wasi_nn_ctx: WasiNnCtx::new([], InMemoryRegistry::new().into()),
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
        let exit_code = match command
            .wasi_cli_run()
            .call_run(&mut store)
            .map_err(|e| {
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
    /// exec_key = H(plan + digests + "exec:v1" + host_abi + caps + policy + tenant + limits + features)
    fn compute_exec_key(&self, plan: &PlanV1, artifact_digest: &Digest) -> Result<Digest, Error> {
        use sha2::{Digest as Sha2Digest, Sha256};

        let plan_validator = crate::plan::PlanValidator::new(self.blobs.clone());
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

        Ok(hasher.finalize().to_vec())
    }

    /// Serialize policy to bytes for fingerprinting
    fn serialize_policy_fingerprint(&self, policy: &crate::types::Policy) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Determinism mode
        bytes.push(match policy.determinism {
            crate::types::DeterminismMode::Strict => 0,
            crate::types::DeterminismMode::Audit => 1,
            crate::types::DeterminismMode::Relaxed => 2,
        });

        // Capabilities (sorted for determinism)
        let mut cap_names: Vec<_> = policy.capabilities.iter().map(|c| c.name.as_str()).collect();
        cap_names.sort();
        for name in cap_names {
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0); // separator
        }

        bytes
    }

    /// Compute plan digest for audit logging
    fn compute_plan_digest(&self, plan: &PlanV1) -> Result<Digest, Error> {
        let plan_validator = crate::plan::PlanValidator::new(self.blobs.clone());
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
        self.cache_dir.join("exec").join(&hex_key[..2]).join(&hex_key[2..])
    }
}
