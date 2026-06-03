/// Resource limits and DOS protection
use crate::types::{Error, ErrorCode};

/// System-wide limits to prevent DOS attacks
#[derive(Debug, Clone)]
pub struct SystemLimits {
    /// Maximum plan file size in bytes
    pub max_plan_size: usize,
    /// Maximum number of components in a plan
    pub max_components: usize,
    /// Maximum number of bindings
    pub max_bindings: usize,
    /// Maximum graph depth (to prevent deep nesting attacks)
    pub max_graph_depth: usize,
    /// Maximum blob size
    pub max_blob_size: u64,
    /// Maximum total memory for execution
    pub max_total_memory: u64,
}

impl Default for SystemLimits {
    fn default() -> Self {
        Self {
            max_plan_size: 1024 * 1024,           // 1MB
            max_components: 1000,                 // 1000 components
            max_bindings: 10_000,                 // 10K bindings
            max_graph_depth: 100,                 // 100 levels deep
            max_blob_size: 100 * 1024 * 1024,     // 100MB
            max_total_memory: 1024 * 1024 * 1024, // 1GB
        }
    }
}

impl SystemLimits {
    /// Validate plan size
    pub fn check_plan_size(&self, size: usize) -> Result<(), Error> {
        if size > self.max_plan_size {
            return Err(Error::new(
                ErrorCode::PlanInvalidSchema,
                format!("Plan size {} exceeds maximum {}", size, self.max_plan_size),
            ));
        }
        Ok(())
    }

    /// Validate component count
    pub fn check_component_count(&self, count: usize) -> Result<(), Error> {
        if count > self.max_components {
            return Err(Error::new(
                ErrorCode::PlanInvalidSchema,
                format!(
                    "Component count {} exceeds maximum {}",
                    count, self.max_components
                ),
            ));
        }
        Ok(())
    }

    /// Validate binding count
    pub fn check_binding_count(&self, count: usize) -> Result<(), Error> {
        if count > self.max_bindings {
            return Err(Error::new(
                ErrorCode::PlanInvalidSchema,
                format!(
                    "Binding count {} exceeds maximum {}",
                    count, self.max_bindings
                ),
            ));
        }
        Ok(())
    }

    /// Validate graph depth
    pub fn check_graph_depth(&self, depth: usize) -> Result<(), Error> {
        if depth > self.max_graph_depth {
            return Err(Error::new(
                ErrorCode::PlanInvalidGraph,
                format!(
                    "Graph depth {} exceeds maximum {}",
                    depth, self.max_graph_depth
                ),
            ));
        }
        Ok(())
    }

    /// Validate blob size
    pub fn check_blob_size(&self, size: u64) -> Result<(), Error> {
        if size > self.max_blob_size {
            return Err(Error::new(
                ErrorCode::BlobIoError,
                format!("Blob size {} exceeds maximum {}", size, self.max_blob_size),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = SystemLimits::default();
        assert_eq!(limits.max_plan_size, 1024 * 1024);
        assert_eq!(limits.max_components, 1000);
    }

    #[test]
    fn test_check_plan_size() {
        let limits = SystemLimits::default();
        assert!(limits.check_plan_size(1024).is_ok());
        assert!(limits.check_plan_size(2 * 1024 * 1024).is_err());
    }

    #[test]
    fn test_check_component_count() {
        let limits = SystemLimits::default();
        assert!(limits.check_component_count(100).is_ok());
        assert!(limits.check_component_count(2000).is_err());
    }
}
