/// Host adapter API for conformance testing
use anyhow::Result;
use compose_host_wasmtime::{
    types::*, CompositorHost, HostConfig,
};

/// Host adapter trait for conformance testing
pub trait HostAdapter {
    /// Validate a plan
    fn validate_plan(&self, plan: &PlanV1) -> Result<(), Error>;

    /// Emit/compose an artifact from a plan
    fn emit_plan(&self, plan: &PlanV1) -> Result<Vec<u8>, Error>;

    /// Execute a plan as a CLI
    fn exec_plan(&self, plan: &PlanV1, args: Vec<String>) -> Result<ExecResult, Error>;

    /// Get metrics from the host
    fn get_metrics(&self) -> Vec<String>;

    /// Get audit records count
    fn get_audit_count(&self) -> usize;

    /// Add a test blob to the store (for conformance testing)
    fn add_test_blob(&self, digest: &[u8], data: &[u8]) -> Result<(), Error>;
}

/// Wasmtime host adapter implementation
pub struct WasmtimeAdapter {
    host: CompositorHost,
}

impl WasmtimeAdapter {
    /// Create a new wasmtime adapter
    pub fn new() -> Result<Self> {
        let config = HostConfig::default();
        let host = CompositorHost::new(config)?;
        Ok(Self { host })
    }

    /// Get the underlying host
    pub fn host(&self) -> &CompositorHost {
        &self.host
    }

    /// Add a test blob to the store (for conformance testing)
    pub fn add_test_blob(&self, _digest: &[u8], data: &[u8]) -> Result<(), Error> {
        self.host.blobs.put(data)?;
        Ok(())
    }
}

impl HostAdapter for WasmtimeAdapter {
    fn validate_plan(&self, plan: &PlanV1) -> Result<(), Error> {
        let validator = self.host.plan_validator();
        validator.validate(plan)
    }

    fn emit_plan(&self, plan: &PlanV1) -> Result<Vec<u8>, Error> {
        let emit_handler = compose_host_wasmtime::emit::EmitHandler::new(
            self.host.blobs.clone(),
            self.host.events.clone(),
            self.host.config.cache_dir.clone(),
        );

        let result = emit_handler.compose(plan)?;
        self.host.blobs.get(&result.digest)
    }

    fn exec_plan(&self, plan: &PlanV1, args: Vec<String>) -> Result<ExecResult, Error> {
        let exec_handler = self.host.exec_handler();
        exec_handler.run_cli(plan, args, vec![])
    }

    fn get_metrics(&self) -> Vec<String> {
        let metrics = self.host.metrics.list(None, None, None);
        metrics.iter().map(|m| m.name.clone()).collect()
    }

    fn get_audit_count(&self) -> usize {
        // For conformance, we just report 0 as audit is file-based
        0
    }

    fn add_test_blob(&self, _digest: &[u8], data: &[u8]) -> Result<(), Error> {
        self.host.blobs.put(data)?;
        Ok(())
    }
}

impl Default for WasmtimeAdapter {
    fn default() -> Self {
        Self::new().expect("Failed to create wasmtime adapter")
    }
}
