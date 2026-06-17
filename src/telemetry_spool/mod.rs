//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Telemetry spool module.
//
// Provides durable OTLP telemetry delivery across OpenObserve downtime:
//
//   writer.rs       — atomic file writer: pending/<signal>-<micros>-<seq>.otlp
//   spool_client.rs — `HttpClient` impl that intercepts OTLP export calls and
//                     spools request bodies instead of posting directly
//   shipper.rs      — background tokio task: drains pending/ → OO when online
//   cli.rs          — `meridian telemetry status|export|import` subcommands

pub mod cli;
pub mod shipper;
pub mod spool_client;
pub mod writer;
