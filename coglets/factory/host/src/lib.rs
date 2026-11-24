//! Host-side Factory dispatcher and replaceable Hermes adapter.
//!
//! Guest Wasm only enqueues intents. This crate claims fenced outbox rows,
//! talks to Hermes through the public Kanban API shape (never internal
//! tables), and records later outcomes. Production adapters are swappable;
//! CI uses [`MockAdapter`].
mod adapter;
mod dispatcher;

pub use adapter::{
    AdapterError, FollowUp, HermesAdapter, Lease, MockAdapter, Outcome, RemoteTask, TaskSpec,
};
pub use dispatcher::{DispatchBatch, Dispatcher};
