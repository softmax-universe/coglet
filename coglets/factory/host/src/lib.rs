//! Host-side Factory dispatcher and replaceable Hermes adapter.
//!
//! Guest Wasm only enqueues intents. This crate claims fenced outbox rows,
//! talks to Hermes through the public Kanban HTTP API (never internal tables),
//! and records later outcomes as durable, replayable events.
mod adapter;
mod contract_fake;
mod dispatcher;
mod http;

pub use adapter::{
    AdapterError, FollowUp, HermesAdapter, Lease, MockAdapter, Outcome, RemoteTask, TaskSpec,
    WorkerReport,
};
pub use contract_fake::ContractKanbanServer;
pub use dispatcher::{DispatchBatch, Dispatcher, FactoryHost};
pub use http::HttpKanban;
