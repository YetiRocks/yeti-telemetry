//! Yeti Telemetry Extension
//!
//! Provides telemetry event persistence, file rotation, and real-time SSE.
//! Implements `EventSubscriber` to receive tracing events as JSON from core's
//! DispatchLayer and processes them into tables, files, and PubSub notifications.
//!
//! Real-time viewing uses native table SSE (GET /yeti-telemetry/Log?stream=sse).

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;
use yeti_core::prelude::*;

// ============================================================================
// Extension (auto-detected by compiler via struct name)
// ============================================================================

pub struct TelemetryExtension;

impl TelemetryExtension {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TelemetryExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl Extension for TelemetryExtension {
    fn name(&self) -> &str {
        "telemetry"
    }

    fn initialize(&self) -> Result<()> {
        eprintln!("[yeti-telemetry] Telemetry extension initialized");
        Ok(())
    }

    fn on_ready(&self, ctx: &ExtensionContext) -> Result<()> {
        eprintln!("[yeti-telemetry] Setting up event subscriber...");

        let log_table = ctx.table("log");
        let span_table = ctx.table("span");
        let metric_table = ctx.table("metric");

        if let Some(ref log_t) = log_table {
            let mut writer = TelemetryWriter::new(
                log_t.storage().clone(),
                span_table.as_ref().map(|t| t.storage().clone()),
                metric_table.as_ref().map(|t| t.storage().clone()),
                log_t.pubsub().cloned(),
            );

            // Add file output for JSON Lines rotation
            let logs_dir = PathBuf::from(ctx.root_dir()).join("logs");
            writer = writer.add_output(Box::new(FileProvider::new(logs_dir)));

            // Add OTLP output if configured in yeti-config.yaml
            match OtlpOutput::from_config(ctx.root_dir()) {
                Some(otlp) => {
                    writer = writer.add_output(Box::new(otlp));
                    eprintln!("[yeti-telemetry] OTLP output configured");
                }
                None => {
                    eprintln!("[yeti-telemetry] OTLP disabled (no otlpEndpoint configured)");
                }
            }

            ctx.set_event_subscriber(Box::new(writer));
            eprintln!("[yeti-telemetry] Event subscriber configured");
        } else {
            eprintln!("[yeti-telemetry] WARNING: Log table not found, no event subscriber");
        }

        Ok(())
    }
}

// ============================================================================
// Status Resource
// ============================================================================

/// Type alias required by compiler (filename → type name mapping)
pub type Telemetry = TelemetryResource;

#[derive(Default)]
pub struct TelemetryResource;

impl Resource for TelemetryResource {
    fn name(&self) -> &str {
        "telemetry"
    }

    /// GET /yeti-telemetry/telemetry — extension status + app registry
    ///
    /// Note: Cannot check host-side statics from dylib (TLS isolation).
    /// The writer is always started alongside the extension by app_loader.
    get!(_req, ctx, {
        let apps: Vec<serde_json::Value> = ctx.app_registry().iter().map(|a| {
            json!({
                "id": a.id,
                "name": a.name,
                "is_extension": a.is_extension,
            })
        }).collect();
        ok(json!({
            "writer": true,
            "status": "active",
            "apps": apps,
        }))
    });
}

// ============================================================================
// Telemetry Writer — event processing and persistence
// ============================================================================

/// Trait for pluggable telemetry output providers.
///
/// Implementations receive serialized telemetry records (as JSON values)
/// and can write them to files, forward to OTLP endpoints, etc.
trait TelemetryOutput: Send {
    fn write_log(&mut self, record: &serde_json::Value);
    fn write_span(&mut self, record: &serde_json::Value);
    fn write_metric(&mut self, record: &serde_json::Value) {
        let _ = record;
    }
}

/// Background writer that receives tracing events as JSON and persists them
/// to tables, files, and PubSub for SSE.
struct TelemetryWriter {
    log_storage: Arc<dyn KvBackend>,
    span_storage: Option<Arc<dyn KvBackend>>,
    metric_storage: Option<Arc<dyn KvBackend>>,
    pubsub: Option<Arc<PubSubManager>>,
    outputs: Vec<Box<dyn TelemetryOutput>>,
}

impl TelemetryWriter {
    fn new(
        log_storage: Arc<dyn KvBackend>,
        span_storage: Option<Arc<dyn KvBackend>>,
        metric_storage: Option<Arc<dyn KvBackend>>,
        pubsub: Option<Arc<PubSubManager>>,
    ) -> Self {
        Self {
            log_storage,
            span_storage,
            metric_storage,
            pubsub,
            outputs: Vec::new(),
        }
    }

    fn add_output(mut self, output: Box<dyn TelemetryOutput>) -> Self {
        self.outputs.push(output);
        self
    }

    /// Main event loop — receives JSON events and dispatches by kind.
    async fn run_loop(mut self, mut rx: tokio::sync::mpsc::Receiver<serde_json::Value>) {
        eprintln!("[telemetry-writer] Started");
        let mut log_count: u64 = 0;
        let mut span_count: u64 = 0;
        let mut metric_count: u64 = 0;

        while let Some(event) = rx.recv().await {
            let kind = event
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            match kind {
                "log" => {
                    log_count += 1;
                    self.write_log(&event).await;
                }
                "span" => {
                    span_count += 1;
                    self.write_span(&event).await;
                }
                "metric" => {
                    metric_count += 1;
                    self.write_metric(&event).await;
                }
                _ => {}
            }

            // Periodic status (every 1000 events)
            let total = log_count + span_count + metric_count;
            if total % 1000 == 0 && total > 0 {
                eprintln!(
                    "[telemetry-writer] Processed {} events (logs={}, spans={}, metrics={})",
                    total, log_count, span_count, metric_count
                );
            }
        }

        eprintln!(
            "[telemetry-writer] Shutting down (logs={}, spans={}, metrics={})",
            log_count, span_count, metric_count
        );
    }

    async fn write_log(&mut self, event: &serde_json::Value) {
        let id = generate_id_v7();
        let timestamp = event.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let record = json!({
            "id": id,
            "timestamp": format_epoch_ms(timestamp),
            "level": event.get("level").and_then(|v| v.as_str()).unwrap_or("INFO"),
            "target": event.get("target").and_then(|v| v.as_str()).unwrap_or(""),
            "message": event.get("message").and_then(|v| v.as_str()).unwrap_or(""),
            "fields": serde_json::to_string(
                event.get("fields").unwrap_or(&json!({}))
            ).unwrap_or_default(),
        });

        if let Ok(bytes) = to_storage_bytes(&record) {
            let _ = self.log_storage.put(id.as_bytes(), &bytes).await;
        }

        if let Some(ref ps) = self.pubsub {
            ps.notify_update("Log", &id, &record).await;
        }

        for output in &mut self.outputs {
            output.write_log(&record);
        }
    }

    async fn write_span(&mut self, event: &serde_json::Value) {
        let storage = match &self.span_storage {
            Some(s) => s,
            None => return,
        };

        let id = generate_id_v7();
        let start_ms = event.get("startTime").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end_ms = event.get("endTime").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let duration_ms = end_ms - start_ms;

        let record = json!({
            "id": id,
            "name": event.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            "target": event.get("target").and_then(|v| v.as_str()).unwrap_or(""),
            "level": event.get("level").and_then(|v| v.as_str()).unwrap_or("INFO"),
            "startTime": format_epoch_ms(start_ms),
            "endTime": format_epoch_ms(end_ms),
            "durationMs": duration_ms,
            "fields": serde_json::to_string(
                event.get("fields").unwrap_or(&json!({}))
            ).unwrap_or_default(),
        });

        if let Ok(bytes) = to_storage_bytes(&record) {
            let _ = storage.put(id.as_bytes(), &bytes).await;
        }

        if let Some(ref ps) = self.pubsub {
            ps.notify_update("Span", &id, &record).await;
        }

        for output in &mut self.outputs {
            output.write_span(&record);
        }
    }

    async fn write_metric(&mut self, event: &serde_json::Value) {
        let storage = match &self.metric_storage {
            Some(s) => s,
            None => return,
        };

        let id = generate_id_v7();
        let timestamp = event.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let record = json!({
            "id": id,
            "name": event.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            "value": event.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0),
            "attributes": serde_json::to_string(
                event.get("attributes").unwrap_or(&json!({}))
            ).unwrap_or_default(),
            "timestamp": format_epoch_ms(timestamp),
        });

        if let Ok(bytes) = to_storage_bytes(&record) {
            let _ = storage.put(id.as_bytes(), &bytes).await;
        }

        if let Some(ref ps) = self.pubsub {
            ps.notify_update("Metric", &id, &record).await;
        }

        for output in &mut self.outputs {
            output.write_metric(&record);
        }
    }
}

impl EventSubscriber for TelemetryWriter {
    fn run(
        self: Box<Self>,
        rx: tokio::sync::mpsc::Receiver<serde_json::Value>,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(self.run_loop(rx))
    }
}

/// Format epoch milliseconds as "seconds.millis" string for table storage.
fn format_epoch_ms(ms: f64) -> String {
    let secs = (ms / 1000.0) as u64;
    let millis = (ms % 1000.0) as u32;
    format!("{}.{:03}", secs, millis)
}

// ============================================================================
// File Provider — JSON Lines file rotation
// ============================================================================

/// File-based telemetry writer with daily rotation.
struct FileProvider {
    log_dir: PathBuf,
    current_date: String,
    writer: Option<BufWriter<File>>,
    current_size: u64,
    max_file_size: u64,
    retention_days: u32,
    write_count: u64,
}

impl FileProvider {
    fn new(log_dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&log_dir);
        let current_date = today_string();

        let mut provider = Self {
            log_dir,
            current_date,
            writer: None,
            current_size: 0,
            max_file_size: 100 * 1024 * 1024, // 100MB
            retention_days: 7,
            write_count: 0,
        };
        provider.open_file();
        provider
    }

    fn write_event(&mut self, event_type: &str, record: &serde_json::Value) {
        self.maybe_rotate();

        let line = json!({
            "type": event_type,
            "data": record,
        });

        if let Some(ref mut w) = self.writer {
            if let Ok(bytes) = serde_json::to_vec(&line) {
                let line_len = bytes.len() as u64 + 1;
                if w.write_all(&bytes).is_ok() && w.write_all(b"\n").is_ok() {
                    self.current_size += line_len;
                    self.write_count += 1;

                    if self.write_count % 100 == 0 {
                        let _ = w.flush();
                    }
                }
            }
        }
    }

    fn maybe_rotate(&mut self) {
        let today = today_string();
        let size_exceeded = self.current_size >= self.max_file_size;

        if today != self.current_date || size_exceeded {
            if let Some(ref mut w) = self.writer {
                let _ = w.flush();
            }
            self.writer = None;
            self.current_date = today;
            self.current_size = 0;
            self.open_file();
            self.cleanup_old_files();
        }
    }

    fn open_file(&mut self) {
        let filename = format!("telemetry-{}.jsonl", self.current_date);
        let path = self.log_dir.join(&filename);

        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                self.current_size = file.metadata().map(|m| m.len()).unwrap_or(0);
                self.writer = Some(BufWriter::new(file));
            }
            Err(e) => {
                eprintln!("[file-provider] Failed to open {}: {}", path.display(), e);
            }
        }
    }

    fn cleanup_old_files(&self) {
        let cutoff = SystemTime::now()
            - std::time::Duration::from_secs(u64::from(self.retention_days) * 86400);

        if let Ok(entries) = fs::read_dir(&self.log_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    if let Ok(meta) = path.metadata() {
                        if let Ok(modified) = meta.modified() {
                            if modified < cutoff {
                                let _ = fs::remove_file(&path);
                                eprintln!(
                                    "[file-provider] Cleaned up old file: {}",
                                    path.display()
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

fn today_string() -> String {
    let now = SystemTime::now();
    let d = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let days = secs / 86400;
    let (year, month, day) = days_to_date(days);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn days_to_date(days_since_epoch: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days_since_epoch + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

impl TelemetryOutput for FileProvider {
    fn write_log(&mut self, record: &serde_json::Value) {
        self.write_event("log", record);
    }

    fn write_span(&mut self, record: &serde_json::Value) {
        self.write_event("span", record);
    }

    fn write_metric(&mut self, record: &serde_json::Value) {
        self.write_event("metric", record);
    }
}

// ============================================================================
// OTLP Provider — OpenTelemetry metrics export
// ============================================================================

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, MeterProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::SdkMeterProvider;

/// OTLP metrics config parsed from yeti-config.yaml
struct OtlpConfig {
    endpoint: String,
    service_name: String,
    metrics_enabled: bool,
}

/// OTLP output provider — exports HTTP metrics to an OTLP collector.
///
/// Lazily initializes the meter provider on first use (inside run_loop on the
/// host's tokio runtime) to avoid dylib tokio spawn issues during on_ready().
struct OtlpOutput {
    config: OtlpConfig,
    provider: Option<SdkMeterProvider>,
    requests_total: Option<Counter<u64>>,
    requests_duration: Option<Histogram<f64>>,
    errors_total: Option<Counter<u64>>,
}

impl OtlpOutput {
    /// Parse OTLP config from yeti-config.yaml. Returns None if no endpoint configured.
    fn from_config(root_dir: &str) -> Option<Self> {
        let config_path = PathBuf::from(root_dir).join("yeti-config.yaml");
        let contents = fs::read_to_string(&config_path).ok()?;
        let yaml: serde_json::Value = serde_yaml::from_str(&contents).ok()?;

        let telemetry = yaml.get("telemetry")?;
        let endpoint = telemetry
            .get("otlpEndpoint")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())?
            .to_string();

        let service_name = telemetry
            .get("serviceName")
            .and_then(|v| v.as_str())
            .unwrap_or("yeti")
            .to_string();

        let metrics_enabled = telemetry
            .get("metrics")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        eprintln!(
            "[yeti-telemetry] OTLP config: endpoint={}, service={}, metrics={}",
            endpoint, service_name, metrics_enabled
        );

        Some(Self {
            config: OtlpConfig {
                endpoint,
                service_name,
                metrics_enabled,
            },
            provider: None,
            requests_total: None,
            requests_duration: None,
            errors_total: None,
        })
    }

    /// Lazily initialize the OTLP meter provider and instruments.
    /// Called on first write_span() inside the host's tokio runtime context.
    fn ensure_initialized(&mut self) {
        if self.provider.is_some() {
            return;
        }

        if !self.config.metrics_enabled {
            return;
        }

        let exporter = match opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(&self.config.endpoint)
            .with_timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[otlp-output] Failed to create metric exporter: {}", e);
                return;
            }
        };

        let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter)
            .with_interval(std::time::Duration::from_secs(15))
            .build();

        let resource = opentelemetry_sdk::Resource::builder()
            .with_attribute(KeyValue::new(
                "service.name",
                self.config.service_name.clone(),
            ))
            .with_attribute(KeyValue::new(
                "deployment.environment",
                std::env::var("YETI_ENV").unwrap_or_else(|_| "development".to_string()),
            ))
            .build();

        let provider = SdkMeterProvider::builder()
            .with_reader(reader)
            .with_resource(resource)
            .build();

        let meter = provider.meter("yeti-telemetry");

        self.requests_total = Some(
            meter
                .u64_counter("http.server.requests")
                .with_description("Total number of HTTP requests")
                .build(),
        );
        self.requests_duration = Some(
            meter
                .f64_histogram("http.server.request.duration")
                .with_description("HTTP request duration in seconds")
                .with_unit("s")
                .build(),
        );
        self.errors_total = Some(
            meter
                .u64_counter("http.server.errors")
                .with_description("Total number of HTTP errors")
                .build(),
        );

        self.provider = Some(provider);
        eprintln!(
            "[otlp-output] Meter provider initialized (endpoint: {})",
            self.config.endpoint
        );
    }
}

impl Drop for OtlpOutput {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            eprintln!("[otlp-output] Shutting down meter provider");
            if let Err(e) = provider.shutdown() {
                eprintln!("[otlp-output] Shutdown error: {:?}", e);
            }
        }
    }
}

impl TelemetryOutput for OtlpOutput {
    fn write_log(&mut self, _record: &serde_json::Value) {
        // Logs are persisted to tables; OTLP export focuses on metrics from spans.
    }

    fn write_span(&mut self, record: &serde_json::Value) {
        self.ensure_initialized();

        // Only record HTTP request spans as OTLP metrics
        let target = record.get("target").and_then(|v| v.as_str()).unwrap_or("");
        if target != "http.request" {
            return;
        }

        let fields_str = record.get("fields").and_then(|v| v.as_str()).unwrap_or("{}");
        let fields: serde_json::Value =
            serde_json::from_str(fields_str).unwrap_or(json!({}));

        let method = fields
            .get("http.method")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN");
        let route = fields
            .get("http.route")
            .and_then(|v| v.as_str())
            .unwrap_or("/");
        let status = fields
            .get("http.status_code")
            .and_then(|v| v.as_str())
            .unwrap_or("0");
        let is_error = fields
            .get("status")
            .and_then(|v| v.as_str())
            == Some("ERROR");

        let attributes = [
            KeyValue::new("http.method", method.to_string()),
            KeyValue::new("http.route", route.to_string()),
            KeyValue::new("http.status_code", status.to_string()),
        ];

        if let Some(ref counter) = self.requests_total {
            counter.add(1, &attributes);
        }

        // Calculate duration from durationMs field (stored in milliseconds)
        let duration_ms = record
            .get("durationMs")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if let Some(ref histogram) = self.requests_duration {
            histogram.record(duration_ms / 1000.0, &attributes);
        }

        if is_error {
            if let Some(ref counter) = self.errors_total {
                counter.add(1, &attributes);
            }
        }
    }

    fn write_metric(&mut self, _record: &serde_json::Value) {
        // Custom metrics could be forwarded to OTLP here if needed.
    }
}
