use serde_json::Value;
use std::sync::Arc;

pub type EventSink = Arc<dyn Fn(Value) + Send + Sync>;

pub fn silent() -> EventSink {
    Arc::new(|_| {})
}
