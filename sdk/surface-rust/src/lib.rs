//! Coglet surface SDK core for spec-v0 (surface protocol 0.1).
//!
//! See `README.md` for scope, guarantees and the list of open qualification gates.

pub mod backend;
pub mod catalog;
pub mod codec;
pub mod config;
pub mod error;
#[doc(hidden)]
pub mod fixture_clock;
pub mod ids;
pub mod reference;
pub mod speech;
pub mod store;
pub mod surface;
#[doc(hidden)]
pub mod testing;
pub mod transport;

pub use backend::{ActionRequest, Backend, BackendHealth, Native, ReadSnapshot, Sink};
pub use config::{AdapterFixture, Config, ExpressionState, GrantSpec, Limits, SpeechConfig};
pub use error::{ApiError, Code};
pub use reference::SemanticBackend;
pub use store::{Store, StoreError};
pub use surface::{Advertisement, KillSwitch, Surface};
pub use transport::ControlPeer;
