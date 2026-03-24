use crate::outgoing_message::OutgoingEnvelope;
use crate::outgoing_message::OutgoingMessage;
use codex_app_server_protocol::LogEntryLevel;
use codex_app_server_protocol::LogEntryNotification;
use codex_app_server_protocol::LogSpanContext;
use codex_app_server_protocol::ServerNotification;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::mpsc;
use tracing::Event;
use tracing::Id;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing::span::Attributes;
use tracing::span::Record;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

#[derive(Clone)]
pub(crate) struct LogNotificationLayer {
    sender: mpsc::Sender<OutgoingEnvelope>,
    enabled: bool,
}

#[derive(Clone, Debug, Default)]
struct StoredSpanFields {
    values: BTreeMap<String, JsonValue>,
}

#[derive(Default)]
struct JsonValueVisitor {
    values: BTreeMap<String, JsonValue>,
    message: Option<String>,
}

impl LogNotificationLayer {
    pub(crate) fn new(sender: mpsc::Sender<OutgoingEnvelope>, enabled: bool) -> Self {
        Self { sender, enabled }
    }
}

impl<S> Layer<S> for LogNotificationLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !self.enabled {
            return;
        }

        let mut visitor = JsonValueVisitor::default();
        event.record(&mut visitor);
        let notification = LogEntryNotification {
            timestamp: unix_timestamp_seconds(),
            level: log_entry_level(*event.metadata().level()),
            target: event.metadata().target().to_string(),
            message: visitor
                .message
                .unwrap_or_else(|| event.metadata().name().to_string()),
            fields: visitor.values,
            span: current_span_context(&ctx, event),
        };

        let _ = self.sender.try_send(OutgoingEnvelope::Broadcast {
            message: OutgoingMessage::AppServerNotification(ServerNotification::LogEntry(
                notification,
            )),
        });
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = JsonValueVisitor::default();
        attrs.record(&mut visitor);
        if visitor.values.is_empty() {
            return;
        }

        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(StoredSpanFields {
                values: visitor.values,
            });
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };

        let mut visitor = JsonValueVisitor::default();
        values.record(&mut visitor);
        if visitor.values.is_empty() {
            return;
        }

        let mut extensions = span.extensions_mut();
        if let Some(stored) = extensions.get_mut::<StoredSpanFields>() {
            stored.values.extend(visitor.values);
        } else {
            extensions.insert(StoredSpanFields {
                values: visitor.values,
            });
        }
    }
}

impl Visit for JsonValueVisitor {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_value(field, JsonValue::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_value(field, JsonValue::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_value(field, JsonValue::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(field, JsonValue::from(value));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.record_value(field, JsonValue::from(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_value(field, JsonValue::from(format!("{value:?}")));
    }
}

impl JsonValueVisitor {
    fn record_value(&mut self, field: &Field, value: JsonValue) {
        if field.name() == "message" {
            self.message = Some(match value {
                JsonValue::String(text) => text,
                other => other.to_string(),
            });
            return;
        }
        self.values.insert(field.name().to_string(), value);
    }
}

fn current_span_context<S>(ctx: &Context<'_, S>, event: &Event<'_>) -> Option<LogSpanContext>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    let scope = ctx.event_scope(event)?;
    let span = scope.from_root().last()?;
    let fields = span
        .extensions()
        .get::<StoredSpanFields>()
        .map(|stored| stored.values.clone())
        .unwrap_or_default();
    Some(LogSpanContext {
        name: span.metadata().name().to_string(),
        fields,
    })
}

fn unix_timestamp_seconds() -> i64 {
    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return 0;
    };
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

fn log_entry_level(level: tracing::Level) -> LogEntryLevel {
    match level {
        tracing::Level::TRACE => LogEntryLevel::Trace,
        tracing::Level::DEBUG => LogEntryLevel::Debug,
        tracing::Level::INFO => LogEntryLevel::Info,
        tracing::Level::WARN => LogEntryLevel::Warn,
        tracing::Level::ERROR => LogEntryLevel::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn emits_log_entry_notification_with_structured_fields() {
        let (sender, mut receiver) = mpsc::channel(4);
        let subscriber =
            tracing_subscriber::registry().with(LogNotificationLayer::new(sender, true));

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "app_server.request",
                rpc.method = "thread/start",
                app_server.connection_id = 7_u64
            );
            let _guard = span.enter();
            tracing::warn!(attempt = 2_u64, retryable = true, "listener queue is full");
        });

        let notification = next_log_entry(&mut receiver);
        assert_eq!(notification.level, LogEntryLevel::Warn);
        assert_eq!(notification.target, module_path!());
        assert_eq!(notification.message, "listener queue is full");
        assert_eq!(notification.fields.get("attempt"), Some(&json!(2)));
        assert_eq!(notification.fields.get("retryable"), Some(&json!(true)));
        assert!(notification.timestamp > 0);
        assert_eq!(
            notification.span,
            Some(LogSpanContext {
                name: "app_server.request".to_string(),
                fields: BTreeMap::from([
                    ("app_server.connection_id".to_string(), json!(7)),
                    ("rpc.method".to_string(), json!("thread/start")),
                ]),
            })
        );
    }

    #[test]
    fn skips_notifications_when_disabled() {
        let (sender, mut receiver) = mpsc::channel(1);
        let subscriber =
            tracing_subscriber::registry().with(LogNotificationLayer::new(sender, false));

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("this should not be delivered");
        });

        assert!(receiver.try_recv().is_err());
    }

    fn next_log_entry(receiver: &mut mpsc::Receiver<OutgoingEnvelope>) -> LogEntryNotification {
        let envelope = receiver.try_recv().expect("missing log notification");
        let OutgoingEnvelope::Broadcast { message } = envelope else {
            panic!("expected broadcast envelope");
        };
        let OutgoingMessage::AppServerNotification(ServerNotification::LogEntry(notification)) =
            message
        else {
            panic!("expected log/entry notification");
        };
        notification
    }
}
