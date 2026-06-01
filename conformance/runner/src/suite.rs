/// Conformance test suite with phased tests
use crate::adapter::HostAdapter;
use anyhow::Result;
use compose_host_wasmtime::types::*;
use serde::{Deserialize, Serialize};

/// Test phase
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TestPhase {
    Plan,
    Emit,
    Exec,
}

/// Test case
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub name: String,
    pub description: String,
    pub phase: TestPhase,
    pub plan: PlanV1,
    pub expect_error: Option<String>,
    pub expect_success: bool,
}

/// Test result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub phase: TestPhase,
    pub passed: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub expected_error: Option<String>,
}

/// Conformance report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformanceReport {
    pub host_id: String,
    pub timestamp: u64,
    pub total_tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub results: Vec<TestResult>,
    pub metrics_count: usize,
    pub audit_count: usize,
    pub attestation: Option<String>,
}

impl ConformanceReport {
    /// Create a new conformance report
    pub fn new(host_id: String, results: Vec<TestResult>) -> Self {
        let total = results.len();
        let passed = results.iter().filter(|r| r.passed).count();
        let failed = total - passed;

        Self {
            host_id,
            timestamp: current_timestamp(),
            total_tests: total,
            passed,
            failed,
            results,
            metrics_count: 0,
            audit_count: 0,
            attestation: None,
        }
    }

    /// Add attestation to the report
    pub fn with_attestation(mut self, attestation: String) -> Self {
        self.attestation = Some(attestation);
        self
    }

    /// Add metrics info
    pub fn with_metrics(mut self, count: usize) -> Self {
        self.metrics_count = count;
        self
    }

    /// Add audit info
    pub fn with_audit(mut self, count: usize) -> Self {
        self.audit_count = count;
        self
    }

    /// Check if all tests passed
    pub fn all_passed(&self) -> bool {
        self.failed == 0
    }
}

/// Test suite runner
pub struct TestSuite {
    pub test_cases: Vec<TestCase>,
}

impl TestSuite {
    /// Create a new test suite
    pub fn new(test_cases: Vec<TestCase>) -> Self {
        Self { test_cases }
    }

    /// Run the test suite against a host adapter
    pub fn run<A: HostAdapter>(&self, adapter: &A, host_id: String) -> Result<ConformanceReport> {
        let mut results = Vec::new();

        for test in &self.test_cases {
            // Pre-populate test blobs for this test
            self.setup_test_blobs(adapter, test)?;

            let result = self.run_test(adapter, test)?;
            results.push(result);
        }

        let metrics_count = adapter.get_metrics().len();
        let audit_count = adapter.get_audit_count();

        let report = ConformanceReport::new(host_id, results)
            .with_metrics(metrics_count)
            .with_audit(audit_count);

        Ok(report)
    }

    /// Setup test blobs for a test case
    fn setup_test_blobs<A: HostAdapter>(&self, adapter: &A, test: &TestCase) -> Result<()> {
        // For each component in the test plan, add a dummy blob if needed
        for comp in &test.plan.components {
            // Create dummy component data (minimal valid wasm module)
            let dummy_wasm = vec![
                0x00, 0x61, 0x73, 0x6d, // magic "\0asm"
                0x01, 0x00, 0x00, 0x00, // version 1
            ];

            // Try to add the blob - it's okay if it already exists
            let _ = adapter.add_test_blob(&comp.digest, &dummy_wasm);
        }
        Ok(())
    }

    /// Run a single test
    fn run_test<A: HostAdapter>(&self, adapter: &A, test: &TestCase) -> Result<TestResult> {
        let start = std::time::Instant::now();

        let (passed, error) = match test.phase {
            TestPhase::Plan => self.run_plan_test(adapter, test),
            TestPhase::Emit => self.run_emit_test(adapter, test),
            TestPhase::Exec => self.run_exec_test(adapter, test),
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(TestResult {
            name: test.name.clone(),
            phase: test.phase.clone(),
            passed,
            duration_ms,
            error,
            expected_error: test.expect_error.clone(),
        })
    }

    /// Run a plan phase test
    fn run_plan_test<A: HostAdapter>(&self, adapter: &A, test: &TestCase) -> (bool, Option<String>) {
        match adapter.validate_plan(&test.plan) {
            Ok(_) => {
                if test.expect_success {
                    (true, None)
                } else {
                    (false, Some("Expected failure but got success".to_string()))
                }
            }
            Err(e) => {
                let error_msg = format!("code={:?}, message={}", e.code, e.message);
                if test.expect_success {
                    (false, Some(format!("Unexpected error: {}", error_msg)))
                } else {
                    // Check if error matches expected
                    if let Some(expected) = &test.expect_error {
                        let error_str = format!("{:?}", e.code);
                        if error_str.contains(expected) {
                            (true, None)
                        } else {
                            (false, Some(format!("Expected error '{}' but got {}", expected, error_msg)))
                        }
                    } else {
                        (true, None)
                    }
                }
            }
        }
    }

    /// Run an emit phase test
    fn run_emit_test<A: HostAdapter>(&self, adapter: &A, test: &TestCase) -> (bool, Option<String>) {
        match adapter.emit_plan(&test.plan) {
            Ok(_bytes) => {
                if test.expect_success {
                    (true, None)
                } else {
                    (false, Some("Expected failure but got success".to_string()))
                }
            }
            Err(e) => {
                if test.expect_success {
                    (false, Some(format!("Unexpected error: {}", e)))
                } else {
                    (true, None)
                }
            }
        }
    }

    /// Run an exec phase test
    fn run_exec_test<A: HostAdapter>(&self, adapter: &A, test: &TestCase) -> (bool, Option<String>) {
        match adapter.exec_plan(&test.plan, vec![]) {
            Ok(result) => {
                if test.expect_success {
                    if result.exit_code == 0 {
                        (true, None)
                    } else {
                        (false, Some(format!("Non-zero exit code: {}", result.exit_code)))
                    }
                } else {
                    (false, Some("Expected failure but got success".to_string()))
                }
            }
            Err(e) => {
                if test.expect_success {
                    (false, Some(format!("Unexpected error: {}", e)))
                } else {
                    (true, None)
                }
            }
        }
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Get the test blob digest (computed from dummy wasm data)
fn test_blob_digest() -> Vec<u8> {
    // Minimal valid wasm module
    let dummy_wasm = vec![
        0x00, 0x61, 0x73, 0x6d, // magic "\0asm"
        0x01, 0x00, 0x00, 0x00, // version 1
    ];

    // Compute SHA-256 digest
    use sha2::{Digest as ShaDigest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&dummy_wasm);
    hasher.finalize().to_vec()
}

/// Create default test suite
pub fn create_default_tests() -> Vec<TestCase> {
    let test_digest = test_blob_digest();

    vec![
        TestCase {
            name: "valid-minimal-plan".to_string(),
            description: "Test plan validation with minimal valid plan".to_string(),
            phase: TestPhase::Plan,
            plan: PlanV1 {
                version: "1".to_string(),
                root: "test".to_string(),
                components: vec![ComponentSpec {
                    id: "test".to_string(),
                    digest: test_digest.clone(),
                    source: None,
                }],
                bindings: vec![],
                secrets: vec![],
                linkage: Default::default(),
                policy: Policy::default(),
            },
            expect_error: None,
            expect_success: true,
        },
        TestCase {
            name: "invalid-empty-root".to_string(),
            description: "Test plan validation fails with empty root".to_string(),
            phase: TestPhase::Plan,
            plan: PlanV1 {
                version: "1".to_string(),
                root: "".to_string(),
                components: vec![ComponentSpec {
                    id: "test".to_string(),
                    digest: test_digest.clone(),
                    source: None,
                }],
                bindings: vec![],
                secrets: vec![],
                linkage: Default::default(),
                policy: Policy::default(),
            },
            expect_error: Some("PlanMissingField".to_string()),
            expect_success: false,
        },
        TestCase {
            name: "invalid-no-components".to_string(),
            description: "Test plan validation fails with no components".to_string(),
            phase: TestPhase::Plan,
            plan: PlanV1 {
                version: "1".to_string(),
                root: "test".to_string(),
                components: vec![],
                bindings: vec![],
                secrets: vec![],
                linkage: Default::default(),
                policy: Policy::default(),
            },
            expect_error: Some("PlanMissingField".to_string()),
            expect_success: false,
        },
        TestCase {
            name: "invalid-version".to_string(),
            description: "Test plan validation fails with unsupported version".to_string(),
            phase: TestPhase::Plan,
            plan: PlanV1 {
                version: "2".to_string(),
                root: "test".to_string(),
                components: vec![ComponentSpec {
                    id: "test".to_string(),
                    digest: test_digest,
                    source: None,
                }],
                bindings: vec![],
                secrets: vec![],
                linkage: Default::default(),
                policy: Policy::default(),
            },
            expect_error: Some("PlanInvalidSchema".to_string()),
            expect_success: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conformance_report_creation() {
        let results = vec![
            TestResult {
                name: "test1".to_string(),
                phase: TestPhase::Plan,
                passed: true,
                duration_ms: 10,
                error: None,
                expected_error: None,
            },
            TestResult {
                name: "test2".to_string(),
                phase: TestPhase::Emit,
                passed: false,
                duration_ms: 20,
                error: Some("Error".to_string()),
                expected_error: None,
            },
        ];

        let report = ConformanceReport::new("test-host".to_string(), results);
        assert_eq!(report.total_tests, 2);
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 1);
        assert!(!report.all_passed());
    }
}
