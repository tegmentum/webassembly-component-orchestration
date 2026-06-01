/// Policy enforcement and capability filtering
use crate::types::{Capability, CapabilityLevel, DeterminismMode, Policy, ResourceLimits, TenantId};
use std::collections::HashSet;

/// Policy enforcement errors
#[derive(Debug, Clone)]
pub enum PolicyError {
    /// Required capability was denied
    RequiredCapabilityDenied(String),
    /// Resource limit exceeded
    ResourceLimitExceeded(String),
    /// Policy violation
    PolicyViolation(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::RequiredCapabilityDenied(cap) => {
                write!(f, "required capability denied: {}", cap)
            }
            PolicyError::ResourceLimitExceeded(resource) => {
                write!(f, "resource limit exceeded: {}", resource)
            }
            PolicyError::PolicyViolation(msg) => write!(f, "policy violation: {}", msg),
        }
    }
}

impl std::error::Error for PolicyError {}

/// Host policy configuration
/// This defines what the host allows, independent of plan requests
#[derive(Debug, Clone)]
pub struct HostPolicy {
    /// Capabilities that the host supports and allows
    pub allowed_capabilities: HashSet<String>,
    /// Maximum resource limits the host will enforce (overrides plan hints)
    pub max_limits: ResourceLimits,
    /// Determinism mode enforcement
    pub determinism_mode: DeterminismMode,
    /// Whether to allow tenant isolation
    pub tenant_isolation_enabled: bool,
}

impl Default for HostPolicy {
    fn default() -> Self {
        Self {
            allowed_capabilities: [
                "wasi:cli".to_string(),
                "wasi:filesystem".to_string(),
                "wasi:http".to_string(),
                // Runtime / dynamic linking verbs. A plan must still
                // declare these to use them; the host merely permits them.
                "dynlink:resolve".to_string(),
                "dynlink:invoke".to_string(),
            ]
            .iter()
            .cloned()
            .collect(),
            max_limits: ResourceLimits {
                cpu_ms: Some(60_000),    // 60 seconds
                memory_bytes: Some(512 * 1024 * 1024), // 512 MB
                io_ops: Some(10_000),    // 10k operations
            },
            determinism_mode: DeterminismMode::Relaxed,
            tenant_isolation_enabled: true,
        }
    }
}

/// Policy enforcer that filters capabilities and enforces limits
#[derive(Clone)]
pub struct PolicyEnforcer {
    host_policy: HostPolicy,
}

impl PolicyEnforcer {
    /// Create a new policy enforcer with host policy
    pub fn new(host_policy: HostPolicy) -> Self {
        Self { host_policy }
    }

    /// Create with default host policy
    pub fn with_defaults() -> Self {
        Self::new(HostPolicy::default())
    }

    /// Filter plan capabilities against host policy
    /// Returns (allowed_capabilities, denied_required, denied_optional)
    pub fn filter_capabilities(
        &self,
        plan_capabilities: &[Capability],
    ) -> Result<FilteredCapabilities, PolicyError> {
        let mut allowed = Vec::new();
        let mut denied_required = Vec::new();
        let mut denied_optional = Vec::new();

        for cap in plan_capabilities {
            if self.host_policy.allowed_capabilities.contains(&cap.name) {
                // Host allows this capability
                allowed.push(cap.clone());
            } else {
                // Host denies this capability
                match cap.level {
                    CapabilityLevel::Required => {
                        // Required capability denied - this is a hard failure
                        denied_required.push(cap.name.clone());
                    }
                    CapabilityLevel::Optional => {
                        // Optional capability denied - soft degradation
                        denied_optional.push(cap.name.clone());
                        tracing::warn!(
                            capability = %cap.name,
                            "optional capability denied by host policy"
                        );
                    }
                }
            }
        }

        if !denied_required.is_empty() {
            return Err(PolicyError::RequiredCapabilityDenied(
                denied_required.join(", "),
            ));
        }

        Ok(FilteredCapabilities {
            allowed,
            denied_optional,
        })
    }

    /// Enforce resource limits (plan hints must not exceed host maximums)
    pub fn enforce_limits(&self, plan_limits: &ResourceLimits) -> Result<ResourceLimits, PolicyError> {
        let mut enforced = plan_limits.clone();

        // CPU limit
        if let Some(plan_cpu) = plan_limits.cpu_ms {
            if let Some(max_cpu) = self.host_policy.max_limits.cpu_ms {
                if plan_cpu > max_cpu {
                    tracing::warn!(
                        plan_cpu = plan_cpu,
                        max_cpu = max_cpu,
                        "plan CPU limit exceeds host maximum, capping"
                    );
                    enforced.cpu_ms = Some(max_cpu);
                }
            }
        } else {
            // No plan limit, use host default
            enforced.cpu_ms = self.host_policy.max_limits.cpu_ms;
        }

        // Memory limit
        if let Some(plan_mem) = plan_limits.memory_bytes {
            if let Some(max_mem) = self.host_policy.max_limits.memory_bytes {
                if plan_mem > max_mem {
                    tracing::warn!(
                        plan_memory = plan_mem,
                        max_memory = max_mem,
                        "plan memory limit exceeds host maximum, capping"
                    );
                    enforced.memory_bytes = Some(max_mem);
                }
            }
        } else {
            enforced.memory_bytes = self.host_policy.max_limits.memory_bytes;
        }

        // IO limit
        if let Some(plan_io) = plan_limits.io_ops {
            if let Some(max_io) = self.host_policy.max_limits.io_ops {
                if plan_io > max_io {
                    tracing::warn!(
                        plan_io = plan_io,
                        max_io = max_io,
                        "plan IO limit exceeds host maximum, capping"
                    );
                    enforced.io_ops = Some(max_io);
                }
            }
        } else {
            enforced.io_ops = self.host_policy.max_limits.io_ops;
        }

        Ok(enforced)
    }

    /// Create an enforced policy from a plan policy
    pub fn enforce_policy(&self, plan_policy: &Policy) -> Result<EnforcedPolicy, PolicyError> {
        // Filter capabilities
        let filtered = self.filter_capabilities(&plan_policy.capabilities)?;

        // Enforce resource limits
        let limits = self.enforce_limits(&plan_policy.limits)?;

        // Validate tenant ID if tenant isolation is enabled
        let tenant = if self.host_policy.tenant_isolation_enabled {
            plan_policy.tenant.clone()
        } else {
            None
        };

        Ok(EnforcedPolicy {
            determinism: plan_policy.determinism,
            capabilities: filtered.allowed,
            denied_optional: filtered.denied_optional,
            tenant,
            limits,
        })
    }

    /// Get the host policy
    pub fn host_policy(&self) -> &HostPolicy {
        &self.host_policy
    }
}

/// Result of capability filtering
#[derive(Debug, Clone)]
pub struct FilteredCapabilities {
    /// Capabilities that are allowed
    pub allowed: Vec<Capability>,
    /// Optional capabilities that were denied (for logging)
    pub denied_optional: Vec<String>,
}

/// Policy after enforcement by host
#[derive(Debug, Clone)]
pub struct EnforcedPolicy {
    pub determinism: DeterminismMode,
    pub capabilities: Vec<Capability>,
    pub denied_optional: Vec<String>,
    pub tenant: Option<TenantId>,
    pub limits: ResourceLimits,
}

impl EnforcedPolicy {
    /// Check if a capability is allowed
    pub fn has_capability(&self, name: &str) -> bool {
        self.capabilities.iter().any(|c| c.name == name)
    }

    /// Get the tenant ID if any
    pub fn tenant_id(&self) -> Option<&str> {
        self.tenant.as_deref()
    }

    /// Get effective CPU limit
    pub fn cpu_limit_ms(&self) -> Option<u64> {
        self.limits.cpu_ms
    }

    /// Get effective memory limit
    pub fn memory_limit_bytes(&self) -> Option<u64> {
        self.limits.memory_bytes
    }

    /// Get effective IO operations limit
    pub fn io_limit_ops(&self) -> Option<u64> {
        self.limits.io_ops
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_capability(name: &str, level: CapabilityLevel) -> Capability {
        Capability {
            name: name.to_string(),
            level,
        }
    }

    #[test]
    fn test_required_capability_denied() {
        let enforcer = PolicyEnforcer::with_defaults();
        let caps = vec![
            create_test_capability("wasi:cli", CapabilityLevel::Required),
            create_test_capability("unknown:cap", CapabilityLevel::Required),
        ];

        let result = enforcer.filter_capabilities(&caps);
        assert!(result.is_err());
        match result {
            Err(PolicyError::RequiredCapabilityDenied(cap)) => {
                assert!(cap.contains("unknown:cap"));
            }
            _ => panic!("Expected RequiredCapabilityDenied error"),
        }
    }

    #[test]
    fn test_optional_capability_denied() {
        let enforcer = PolicyEnforcer::with_defaults();
        let caps = vec![
            create_test_capability("wasi:cli", CapabilityLevel::Required),
            create_test_capability("unknown:cap", CapabilityLevel::Optional),
        ];

        let result = enforcer.filter_capabilities(&caps).unwrap();
        assert_eq!(result.allowed.len(), 1);
        assert_eq!(result.denied_optional.len(), 1);
        assert_eq!(result.denied_optional[0], "unknown:cap");
    }

    #[test]
    fn test_resource_limit_enforcement() {
        let enforcer = PolicyEnforcer::with_defaults();
        let limits = ResourceLimits {
            cpu_ms: Some(100_000), // Exceeds max
            memory_bytes: Some(128 * 1024 * 1024), // Within max
            io_ops: None,
        };

        let enforced = enforcer.enforce_limits(&limits).unwrap();
        assert_eq!(enforced.cpu_ms, Some(60_000)); // Capped to host max
        assert_eq!(enforced.memory_bytes, Some(128 * 1024 * 1024)); // Unchanged
        assert_eq!(enforced.io_ops, Some(10_000)); // Host default applied
    }

    #[test]
    fn test_full_policy_enforcement() {
        let enforcer = PolicyEnforcer::with_defaults();
        let policy = Policy {
            determinism: DeterminismMode::Strict,
            capabilities: vec![
                create_test_capability("wasi:cli", CapabilityLevel::Required),
                create_test_capability("wasi:http", CapabilityLevel::Optional),
                create_test_capability("unknown:cap", CapabilityLevel::Optional),
            ],
            tenant: Some("tenant-123".to_string()),
            limits: ResourceLimits {
                cpu_ms: Some(30_000),
                memory_bytes: Some(256 * 1024 * 1024),
                io_ops: Some(5_000),
            },
        };

        let enforced = enforcer.enforce_policy(&policy).unwrap();
        assert_eq!(enforced.capabilities.len(), 2);
        assert_eq!(enforced.denied_optional.len(), 1);
        assert_eq!(enforced.tenant, Some("tenant-123".to_string()));
        assert_eq!(enforced.limits.cpu_ms, Some(30_000));
    }
}
