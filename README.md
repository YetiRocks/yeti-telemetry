<p align="center">
  <img src="https://cdn.prod.website-files.com/68e09cef90d613c94c3671c0/697e805a9246c7e090054706_logo_horizontal_grey.png" alt="Yeti" width="200" />
</p>

---

# Yeti Telemetry

[![Yeti](https://img.shields.io/badge/Yeti-Extension-blue)](https://yetirocks.com)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![OpenTelemetry](https://img.shields.io/badge/OTLP-Export-purple)](https://opentelemetry.io)

Unified telemetry collector, viewer, and dashboard for Yeti. Captures logs, spans, and metrics from the tracing pipeline, persists them to RocksDB tables, streams them via SSE, exports to OTLP endpoints, and provides a real-time dashboard UI.

## Features

- **Log Collection** - Captures all tracing log events with level, target, and message
- **Span Tracking** - Records timed operations with trace IDs and parent-child relationships
- **Metric Recording** - Stores metric data points with attributes
- **Real-Time Dashboard** - React UI with live log streaming at `/yeti-telemetry/`
- **SSE Streaming** - Real-time event streams for Log, Span, and Metric tables
- **OTLP Export** - Push metrics to any OpenTelemetry-compatible collector (Grafana, Datadog, etc.)
- **File Rotation** - JSONL daily log files with 100MB max size and 7-day retention
- **REST API** - Query persisted telemetry records with FIQL filters and pagination

## Installation

```bash
# Clone into your Yeti applications folder
cd ~/yeti/applications
git clone https://github.com/yetirocks/yeti-telemetry.git

# Restart Yeti to load the extension
# The dashboard will be available at /yeti-telemetry/
```

yeti-telemetry is loaded before other extensions so it captures their startup events.

## Dashboard

Open your browser to:
```
https://localhost:9996/yeti-telemetry/
```

The dashboard displays:
- **Real-time log stream** with level filtering and auto-scroll
- **Telemetry status** (collector active, writer active, OTLP endpoint)

## API Endpoints

### Telemetry Status

```bash
curl -sk https://localhost:9996/yeti-telemetry/telemetry
# Response: {"collector": true, "writer": true, "otlpEndpoint": null}
```

### Log Records

```bash
# List all log records
curl -sk https://localhost:9996/yeti-telemetry/Log

# Filter by level
curl -sk "https://localhost:9996/yeti-telemetry/Log?filter=level==ERROR"

# Filter by target
curl -sk "https://localhost:9996/yeti-telemetry/Log?filter=target==yeti_core::routing"

# Paginate
curl -sk "https://localhost:9996/yeti-telemetry/Log?limit=50&offset=100"

# Real-time SSE stream
curl -sk -N "https://localhost:9996/yeti-telemetry/Log?stream=sse"
```

### Span Records

```bash
# List all span records
curl -sk https://localhost:9996/yeti-telemetry/Span

# Filter by name
curl -sk "https://localhost:9996/yeti-telemetry/Span?filter=name==handle_request"

# Filter by trace ID
curl -sk "https://localhost:9996/yeti-telemetry/Span?filter=traceId==abc123"

# Real-time SSE stream
curl -sk -N "https://localhost:9996/yeti-telemetry/Span?stream=sse"
```

### Metric Records

```bash
# List all metric records
curl -sk https://localhost:9996/yeti-telemetry/Metric

# Filter by metric name
curl -sk "https://localhost:9996/yeti-telemetry/Metric?filter=name==http.server.requests"

# Real-time SSE stream
curl -sk -N "https://localhost:9996/yeti-telemetry/Metric?stream=sse"
```

## Schema

```graphql
type Log @table(database: "yeti-telemetry") @export(sse: true) {
  id: ID! @primaryKey
  timestamp: String! @indexed
  level: String! @indexed
  target: String! @indexed
  message: String!
  fields: String
  __createdAt__: String
}

type Span @table(database: "yeti-telemetry") @export(sse: true) {
  id: ID! @primaryKey
  traceId: String @indexed
  parentSpanId: String
  name: String! @indexed
  target: String!
  level: String!
  startTime: String!
  endTime: String
  durationMs: Float
  fields: String
  __createdAt__: String
}

type Metric @table(database: "yeti-telemetry") @export(sse: true) {
  id: ID! @primaryKey
  name: String! @indexed
  value: Float!
  attributes: String
  timestamp: String!
  __createdAt__: String
}
```

## OTLP Export

Configure an OpenTelemetry endpoint in `yeti-config.yaml` (the server-level config):

```yaml
telemetry:
  otlpEndpoint: "http://localhost:4317"
```

When configured, yeti-telemetry exports these metrics:
- `http.server.requests` - Counter of HTTP requests
- `http.server.request.duration` - Histogram of request durations
- `http.server.errors` - Counter of error responses

## File Logging

yeti-telemetry writes JSONL log files to `~/yeti/logs/`:
- **Daily rotation** with date-stamped filenames
- **100MB max** file size before rotation
- **7-day retention** with automatic cleanup

## Architecture

```
tracing events → DispatchLayer → JSON channel → TelemetryWriter
                                                    ├── Log/Span/Metric tables (RocksDB)
                                                    ├── PubSub → SSE streams
                                                    ├── FileProvider (JSONL rotation)
                                                    └── OtlpOutput (metrics export)
```

The core DispatchLayer captures tracing events and sends them as JSON values through an unbounded channel. The TelemetryWriter (running as an EventSubscriber) processes each event and routes it to the appropriate output.

## Replacing yeti-telemetry

To use a custom telemetry pipeline, create your own extension implementing the `EventSubscriber` trait and delete yeti-telemetry. The core DispatchLayer will route events to your extension instead.

## Project Structure

```
yeti-telemetry/
├── config.yaml          # Extension configuration
├── schema.graphql       # Log, Span, Metric tables with SSE
├── resources/
│   └── telemetry.rs     # TelemetryExtension, TelemetryWriter,
│                        # FileProvider, OtlpOutput
├── source/              # React/Vite dashboard source
│   └── src/
│       ├── App.tsx      # Dashboard component
│       ├── components/  # UI components
│       ├── hooks/       # Custom React hooks
│       └── types.ts     # TypeScript types
└── web/                 # Built dashboard UI
    └── index.html
```

## Learn More

- [Yeti Documentation](https://yetirocks.com/docs)
- [Telemetry Guide](https://yetirocks.com/docs/guides/telemetry)
- [Event Subscribers](https://yetirocks.com/docs/guides/event-subscribers)
- [OpenTelemetry](https://opentelemetry.io)

---

Built with [Yeti](https://yetirocks.com) - The fast, declarative database platform.
