//! Guest ABI 0.1. Successful imports stage effects, not external completion.
//! No ambient time or randomness is used; contexts receive recorded runtime inputs.
#[cfg(feature = "database")]
pub mod database;

pub use coglet_types::{Action, Event, LlmRequest, LogEntry, TimerRequest};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    InvalidArgument,
    PermissionDenied,
    ResourceExhausted,
    Conflict,
    Reserved(i32),
    Encoding(String),
    FileSystem(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Timer,
    Emit,
    Llm,
    Log,
}

/// Host boundary. Implementations copy the argument before reentry.
pub trait Host {
    fn call(&mut self, operation: Operation, bytes: &[u8]) -> i32;
    /// Native engines supply a transactional overlay, never an ambient host path.
    fn read_file(&mut self, _path: &str) -> Result<Option<Vec<u8>>, HostError> {
        Err(HostError::PermissionDenied)
    }
    fn write_file(&mut self, _path: &str, _bytes: &[u8]) -> Result<(), HostError> {
        Err(HostError::PermissionDenied)
    }
}

pub use coglet_guest_macros::coglet;

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod __private {
    use super::*;
    use std::cell::RefCell;

    // wasi-libc's futex timeout slow path otherwise retains a clock import,
    // even for a single-threaded guest that never asks for time. Bind that
    // symbol to a deterministic trap: a dependency requesting ambient time
    // must abort, never observe or synthesize a timestamp.
    #[unsafe(no_mangle)]
    pub extern "C" fn __imported_wasi_snapshot_preview1_clock_time_get(
        _clock: u32,
        _precision: u64,
        _result: u32,
    ) -> u32 {
        std::arch::wasm32::unreachable()
    }
    thread_local! { static BUFFERS: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) }; }

    #[link(wasm_import_module = "coglet_v0")]
    unsafe extern "C" {
        fn timer(ptr: u32, len: u32) -> i32;
        fn emit(ptr: u32, len: u32) -> i32;
        fn llm(ptr: u32, len: u32) -> i32;
        fn log(ptr: u32, len: u32) -> i32;
    }
    pub struct Imports;
    impl Host for Imports {
        fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>, HostError> {
            match std::fs::read(path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(HostError::FileSystem(error.to_string())),
            }
        }
        fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), HostError> {
            std::fs::write(path, bytes).map_err(|error| HostError::FileSystem(error.to_string()))
        }
        fn call(&mut self, operation: Operation, bytes: &[u8]) -> i32 {
            let Ok(len) = u32::try_from(bytes.len()) else {
                return 1;
            };
            let ptr = bytes.as_ptr() as u32;
            // SAFETY: each import is synchronous and must copy these live bytes.
            unsafe {
                match operation {
                    Operation::Timer => timer(ptr, len),
                    Operation::Emit => emit(ptr, len),
                    Operation::Llm => llm(ptr, len),
                    Operation::Log => log(ptr, len),
                }
            }
        }
    }
    pub fn allocate(len: u32) -> u32 {
        let bytes = vec![0; len as usize];
        let ptr = bytes.as_ptr() as u32;
        BUFFERS.with(|buffers| buffers.borrow_mut().push(bytes));
        ptr
    }
    pub fn invoke<F>(ptr: u32, len: u32, step: u64, time: u64, seed: u64, function: F) -> i32
    where
        F: FnOnce(&mut Context<'_, Imports>, &[Event]) -> Result<(), HostError>,
    {
        let bytes = BUFFERS.with(|cell| {
            let buffers = cell.borrow();
            buffers.iter().find_map(|b| {
                let base = b.as_ptr() as u32 as u64;
                let offset = (ptr as u64).checked_sub(base)?;
                let end = offset.checked_add(len as u64)?;
                if end > b.len() as u64 {
                    return None;
                }
                Some(b[offset as usize..end as usize].to_vec())
            })
        });
        let result = bytes
            .ok_or(HostError::InvalidArgument)
            .and_then(|b| decode_events(&b))
            .and_then(|events| {
                function(&mut Context::new(&mut Imports, step, time, seed), &events)
            });
        BUFFERS.with(|buffers| buffers.borrow_mut().clear());
        if result.is_ok() { 0 } else { 1 }
    }
}

pub struct Context<'a, H: Host> {
    host: &'a mut H,
    pub step: u64,
    pub logical_time: u64,
    pub seed: u64,
    index: u64,
}
impl<'a, H: Host> Context<'a, H> {
    pub fn new(host: &'a mut H, step: u64, logical_time: u64, seed: u64) -> Self {
        Self {
            host,
            step,
            logical_time,
            seed,
            index: 0,
        }
    }
    /// Deterministic 128-bit effect ID; the host still enforces lifetime uniqueness.
    /// Trace-local steps/seeds can repeat: use a persistent generation in the seed
    /// policy or a persistent guest ID ledger across trace resets.
    pub fn next_id(&mut self) -> Result<String, HostError> {
        let index = self.index;
        self.index = index.checked_add(1).ok_or(HostError::ResourceExhausted)?;
        let mut hash = Sha256::new();
        hash.update(b"coglet-guest-id-v0");
        hash.update(self.seed.to_be_bytes());
        hash.update(self.step.to_be_bytes());
        hash.update(index.to_be_bytes());
        Ok(hash.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
    fn send<T: Serialize>(
        &mut self,
        op: Operation,
        rule: &str,
        value: &T,
    ) -> Result<(), HostError> {
        let bytes = coglet_types::encode_typed(rule, value).map_err(HostError::Encoding)?;
        match self.host.call(op, &bytes) {
            0 => Ok(()),
            1 => Err(HostError::InvalidArgument),
            2 => Err(HostError::PermissionDenied),
            3 => Err(HostError::ResourceExhausted),
            4 => Err(HostError::Conflict),
            n => Err(HostError::Reserved(n)),
        }
    }
    pub fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>, HostError> {
        self.host.read_file(path)
    }
    pub fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), HostError> {
        self.host.write_file(path, bytes)
    }
    pub fn timer(&mut self, id: &str, at: u64) -> Result<(), HostError> {
        self.send(
            Operation::Timer,
            "abi::timer-request",
            &TimerRequest {
                id: id.to_owned(),
                at,
            },
        )
    }
    pub fn emit(&mut self, action: &Action) -> Result<(), HostError> {
        self.send(Operation::Emit, "common::action", action)
    }
    pub fn llm(&mut self, request: &LlmRequest) -> Result<(), HostError> {
        if request.capability != "llm.complete" {
            return Err(HostError::InvalidArgument);
        }
        self.send(Operation::Llm, "abi::llm-request", request)
    }
    pub fn log(&mut self, entry: &LogEntry) -> Result<(), HostError> {
        self.send(Operation::Log, "abi::log-entry", entry)
    }
}

/// Decode and validate a nonempty batch; preserve unsigned 64-bit inputs exactly.
pub fn decode_events(bytes: &[u8]) -> Result<Vec<Event>, HostError> {
    let value = coglet_types::decode_profile(bytes).map_err(HostError::Encoding)?;
    let coglet_types::Value::Array(events) = value else {
        return Err(HostError::InvalidArgument);
    };
    if events.is_empty() {
        return Err(HostError::InvalidArgument);
    }
    events
        .into_iter()
        .map(|event| {
            coglet_types::validate("common::event", &event).map_err(HostError::Encoding)?;
            event
                .deserialized()
                .map_err(|e| HostError::Encoding(e.to_string()))
        })
        .collect()
}
