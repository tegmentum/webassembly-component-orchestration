/// Metrics collection and aggregation
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Metric type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Timer,
}

/// Metric value
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MetricValue {
    Counter { value: u64 },
    Gauge { value: f64 },
    Histogram { values: Vec<f64> },
    DurationMs { value: u64 },
}

/// Metric label
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MetricLabel {
    pub key: String,
    pub value: String,
}

/// Metric record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub metric_type: MetricType,
    pub value: MetricValue,
    pub labels: Vec<MetricLabel>,
    pub timestamp: u64,
}

/// Aggregation period
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AggregationPeriod {
    Minute,
    Hour,
    Day,
}

/// Metric summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSummary {
    pub name: String,
    pub count: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub period: AggregationPeriod,
}

/// Metrics collector
#[derive(Clone)]
pub struct MetricsCollector {
    metrics: Arc<Mutex<Vec<Metric>>>,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Record a metric
    pub fn record(&self, metric: Metric) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.push(metric);
    }

    /// Record a counter metric
    pub fn counter(&self, name: impl Into<String>, value: u64, labels: Vec<MetricLabel>) {
        self.record(Metric {
            name: name.into(),
            metric_type: MetricType::Counter,
            value: MetricValue::Counter { value },
            labels,
            timestamp: current_timestamp(),
        });
    }

    /// Record a gauge metric
    pub fn gauge(&self, name: impl Into<String>, value: f64, labels: Vec<MetricLabel>) {
        self.record(Metric {
            name: name.into(),
            metric_type: MetricType::Gauge,
            value: MetricValue::Gauge { value },
            labels,
            timestamp: current_timestamp(),
        });
    }

    /// Record a timer metric (duration in milliseconds)
    pub fn timer(&self, name: impl Into<String>, duration_ms: u64, labels: Vec<MetricLabel>) {
        self.record(Metric {
            name: name.into(),
            metric_type: MetricType::Timer,
            value: MetricValue::DurationMs { value: duration_ms },
            labels,
            timestamp: current_timestamp(),
        });
    }

    /// List metrics with optional filtering
    pub fn list(
        &self,
        name_filter: Option<&str>,
        labels_filter: Option<&[MetricLabel]>,
        since: Option<u64>,
    ) -> Vec<Metric> {
        let metrics = self.metrics.lock().unwrap();
        metrics
            .iter()
            .filter(|m| {
                // Filter by name
                if let Some(name) = name_filter {
                    if !m.name.contains(name) {
                        return false;
                    }
                }

                // Filter by timestamp
                if let Some(since_ts) = since {
                    if m.timestamp < since_ts {
                        return false;
                    }
                }

                // Filter by labels
                if let Some(labels) = labels_filter {
                    for label in labels {
                        if !m.labels.contains(label) {
                            return false;
                        }
                    }
                }

                true
            })
            .cloned()
            .collect()
    }

    /// Get metric summary
    pub fn summary(&self, name: &str, _period: AggregationPeriod, since: Option<u64>) -> Option<MetricSummary> {
        let metrics = self.list(Some(name), None, since);

        if metrics.is_empty() {
            return None;
        }

        let mut count = 0u64;
        let mut sum = 0.0;
        let mut min = f64::MAX;
        let mut max = f64::MIN;

        for metric in &metrics {
            count += 1;
            let value = match &metric.value {
                MetricValue::Counter { value } => *value as f64,
                MetricValue::Gauge { value } => *value,
                MetricValue::DurationMs { value } => *value as f64,
                MetricValue::Histogram { values } => values.iter().sum::<f64>() / values.len() as f64,
            };

            sum += value;
            min = min.min(value);
            max = max.max(value);
        }

        Some(MetricSummary {
            name: name.to_string(),
            count,
            sum,
            min,
            max,
            avg: sum / count as f64,
            period: _period,
        })
    }

    /// Clear old metrics
    pub fn clear_old(&self, before: u64) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.retain(|m| m.timestamp >= before);
    }

    /// Get total metric count
    pub fn count(&self) -> usize {
        let metrics = self.metrics.lock().unwrap();
        metrics.len()
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_metric() {
        let collector = MetricsCollector::new();

        collector.counter("test.counter", 42, vec![]);

        let metrics = collector.list(Some("test.counter"), None, None);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "test.counter");
        match &metrics[0].value {
            MetricValue::Counter { value } => assert_eq!(*value, 42),
            _ => panic!("Expected counter value"),
        }
    }

    #[test]
    fn test_gauge_metric() {
        let collector = MetricsCollector::new();

        collector.gauge("test.gauge", 3.14, vec![]);

        let metrics = collector.list(Some("test.gauge"), None, None);
        assert_eq!(metrics.len(), 1);
        match &metrics[0].value {
            MetricValue::Gauge { value } => assert!((value - 3.14).abs() < 0.001),
            _ => panic!("Expected gauge value"),
        }
    }

    #[test]
    fn test_timer_metric() {
        let collector = MetricsCollector::new();

        collector.timer("test.timer", 1500, vec![]);

        let metrics = collector.list(Some("test.timer"), None, None);
        assert_eq!(metrics.len(), 1);
        match &metrics[0].value {
            MetricValue::DurationMs { value } => assert_eq!(*value, 1500),
            _ => panic!("Expected timer value"),
        }
    }

    #[test]
    fn test_metric_filtering() {
        let collector = MetricsCollector::new();

        collector.counter("foo.count", 1, vec![]);
        collector.counter("bar.count", 2, vec![]);
        collector.gauge("foo.gauge", 1.0, vec![]);

        let foo_metrics = collector.list(Some("foo"), None, None);
        assert_eq!(foo_metrics.len(), 2);

        let bar_metrics = collector.list(Some("bar"), None, None);
        assert_eq!(bar_metrics.len(), 1);
    }

    #[test]
    fn test_metric_summary() {
        let collector = MetricsCollector::new();

        collector.counter("test.stat", 10, vec![]);
        collector.counter("test.stat", 20, vec![]);
        collector.counter("test.stat", 30, vec![]);

        let summary = collector.summary("test.stat", AggregationPeriod::Minute, None).unwrap();
        assert_eq!(summary.count, 3);
        assert_eq!(summary.sum, 60.0);
        assert_eq!(summary.min, 10.0);
        assert_eq!(summary.max, 30.0);
        assert_eq!(summary.avg, 20.0);
    }

    #[test]
    fn test_clear_old_metrics() {
        let collector = MetricsCollector::new();

        collector.counter("test", 1, vec![]);
        assert_eq!(collector.count(), 1);

        // Clear all metrics before now + 1 second
        let future = current_timestamp() + 1000;
        collector.clear_old(future);
        assert_eq!(collector.count(), 0);
    }
}
