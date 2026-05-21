/// Event collection and emission
use crate::types::{Event, EventLevel};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Event collector that captures all emitted events
#[derive(Debug, Clone)]
pub struct EventCollector {
    events: Arc<Mutex<Vec<Event>>>,
}

impl EventCollector {
    /// Create a new event collector
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Emit a structured event
    pub fn emit(&self, event: Event) {
        // Also log to tracing
        match event.level {
            EventLevel::Trace => tracing::trace!(
                message = %event.message,
                context = ?event.context,
                "event"
            ),
            EventLevel::Info => tracing::info!(
                message = %event.message,
                context = ?event.context,
                "event"
            ),
            EventLevel::Warn => tracing::warn!(
                message = %event.message,
                context = ?event.context,
                "event"
            ),
            EventLevel::Error => tracing::error!(
                message = %event.message,
                context = ?event.context,
                "event"
            ),
        }

        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }

    /// Emit a trace-level event
    pub fn trace(&self, message: impl Into<String>, context: Option<String>) {
        self.emit(Event {
            level: EventLevel::Trace,
            timestamp: current_timestamp(),
            message: message.into(),
            context,
        });
    }

    /// Emit an info-level event
    pub fn info(&self, message: impl Into<String>, context: Option<String>) {
        self.emit(Event {
            level: EventLevel::Info,
            timestamp: current_timestamp(),
            message: message.into(),
            context,
        });
    }

    /// Emit a warn-level event
    pub fn warn(&self, message: impl Into<String>, context: Option<String>) {
        self.emit(Event {
            level: EventLevel::Warn,
            timestamp: current_timestamp(),
            message: message.into(),
            context,
        });
    }

    /// Emit an error-level event
    pub fn error(&self, message: impl Into<String>, context: Option<String>) {
        self.emit(Event {
            level: EventLevel::Error,
            timestamp: current_timestamp(),
            message: message.into(),
            context,
        });
    }

    /// Get all collected events
    pub fn get_events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    /// Clear all collected events
    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }

    /// Get events filtered by level
    pub fn get_events_by_level(&self, level: EventLevel) -> Vec<Event> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.level == level)
            .cloned()
            .collect()
    }
}

impl Default for EventCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Get current timestamp in milliseconds since epoch
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_collection() {
        let collector = EventCollector::new();

        collector.info("test message", None);
        collector.error("error message", Some("context".to_string()));

        let events = collector.get_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].level, EventLevel::Info);
        assert_eq!(events[1].level, EventLevel::Error);
        assert_eq!(events[1].context, Some("context".to_string()));
    }

    #[test]
    fn test_filter_by_level() {
        let collector = EventCollector::new();

        collector.info("info1", None);
        collector.error("error1", None);
        collector.info("info2", None);

        let errors = collector.get_events_by_level(EventLevel::Error);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "error1");
    }
}
