//! Trusted application embedding API. Handles are owned/serialized by the caller.
//! Unwinding Rust panics fence the entire embedding, not just the current link.
//! This does not contain aborts, signals, OOM or invalid caller pointers.
use crate::{SURFACE_MAX_FRAME, SurfaceBinding, SurfaceSession};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

#[cfg(feature = "ffi-test-hooks")]
#[path = "ffi_test_hooks.rs"]
mod test_hooks;
#[path = "ffi_timing.rs"]
pub mod timing;
#[cfg(feature = "ffi-test-hooks")]
pub use test_hooks::*;

static BINDING: OnceLock<SurfaceBinding> = OnceLock::new();
static RUNTIME: Mutex<Option<Arc<Mutex<crate::RuntimeService>>>> = Mutex::new(None);
static FENCED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static FAILED_AT_START: AtomicU64 = AtomicU64::new(0);

/// Supervisor status: 0 running, 1 stopped, -2 new guest failure or caught panic.
/// Historical failed batches are never replayed by Restart.
#[unsafe(no_mangle)]
pub extern "C" fn coglet_runtime_status() -> i32 {
    guarded(|| {
        if FENCED.load(Ordering::SeqCst) {
            return -2;
        }
        let Ok(runtime) = RUNTIME.lock() else {
            return -2;
        };
        let Some(service) = runtime.as_ref() else {
            return 1;
        };
        let Ok(service) = service.lock() else {
            return -2;
        };
        if service.failed_attempts() as u64 > FAILED_AT_START.load(Ordering::SeqCst) {
            -2
        } else {
            0
        }
    })
}

fn guarded(call: impl FnOnce() -> i32) -> i32 {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(status) => status,
        Err(payload) => {
            // A user-defined panic payload may itself panic in Drop. Never run
            // it across the C boundary; fault-path leak until shutdown.
            std::mem::forget(payload);
            // Do not resume uncertain execution, clear service poison or dispatch
            // already queued output. Close all links, then stop and reopen the WAL.
            // The durable attempt coordinator decides whether recovery can run.
            FENCED.store(true, Ordering::SeqCst);
            -2
        }
    }
}

/// Start/recover from trusted JSON {wal, setup, native_schedule?, trusted_extension_catalog?}.
/// The optional closed catalog record is {json, sha256}; pins cover exact UTF-8 bytes.
/// A caught panic requires closing every link and calling stop before restart.
/// # Safety
/// `config` references `n` readable immutable bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_runtime_start(config: *const u8, n: usize) -> i32 {
    guarded(|| {
        const MAX_START_CONFIG: usize = 16 * 1024 * 1024;
        if FENCED.load(Ordering::SeqCst) {
            return -2;
        }
        if config.is_null() || n == 0 || n > MAX_START_CONFIG || BINDING.get().is_none() {
            return -1;
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Config {
            wal: std::path::PathBuf,
            setup: serde_json::Value,
            #[serde(default)]
            native_schedule: Option<serde_json::Value>,
            #[serde(default)]
            trusted_extension_catalog: Option<serde_json::Map<String, serde_json::Value>>,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct CatalogConfig {
            json: String,
            sha256: String,
        }
        let Ok(config) =
            serde_json::from_slice::<Config>(unsafe { slice::from_raw_parts(config, n) })
        else {
            return -1;
        };
        if !config.wal.is_absolute() {
            return -1;
        }
        let catalog = match config.trusted_extension_catalog {
            Some(fields) => {
                match serde_json::from_value::<CatalogConfig>(serde_json::Value::Object(fields)) {
                    Ok(catalog) => Some(catalog),
                    Err(_) => return -1,
                }
            }
            None => None,
        };
        let Ok(mut runtime) = RUNTIME.lock() else {
            return -2;
        };
        if runtime.is_some() {
            return -1;
        }
        let opened = match catalog {
            Some(catalog) => crate::TrustedExtensionCatalog::from_pinned_bytes(
                catalog.json.as_bytes(),
                &catalog.sha256,
            )
            .and_then(|catalog| {
                crate::RuntimeService::open_with_catalog(&config.wal, &config.setup, 128, catalog)
            }),
            None => crate::RuntimeService::open(&config.wal, &config.setup, 128),
        };
        let result = opened.and_then(|mut service| {
            service.supervised_now()?;
            if let Some(schedule) = config.native_schedule {
                #[cfg(feature = "native-engine")]
                service
                    .enable_native(serde_json::from_value(schedule).map_err(|e| e.to_string())?)?;
                #[cfg(not(feature = "native-engine"))]
                {
                    let _ = schedule;
                    return Err("native-engine feature unavailable".into());
                }
            }
            Ok(service)
        });
        match result {
            Ok(service) => {
                FAILED_AT_START.store(service.failed_attempts() as u64, Ordering::SeqCst);
                *runtime = Some(Arc::new(Mutex::new(service)));
                0
            }
            Err(error) => {
                eprintln!("coglet runtime startup refused: {error}");
                -2
            }
        }
    })
}

/// Stop only after all links close. Returns 1 while links hold the runtime.
/// Poisoned service state is discarded, NEVER unpoisoned for continued execution.
#[unsafe(no_mangle)]
pub extern "C" fn coglet_runtime_stop() -> i32 {
    guarded(|| {
        // Only stop may recover the outer ownership slot: it never executes the
        // possibly poisoned RuntimeService and drops it before clearing poison.
        let mut runtime = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
        if runtime.as_ref().is_some_and(|r| Arc::strong_count(r) != 1) {
            return 1;
        }
        *runtime = None;
        RUNTIME.clear_poison();
        GENERATION.fetch_add(1, Ordering::SeqCst);
        FENCED.store(false, Ordering::SeqCst);
        0
    })
}

#[repr(C)]
pub struct CogletSurfaceLink {
    session: SurfaceSession,
    clock: Instant,
    generation: u64,
    timings: timing::History,
    #[cfg(feature = "ffi-test-hooks")]
    recv_pump_hold: Option<Arc<timing::obstruction::PumpHold>>,
}
impl CogletSurfaceLink {
    /// Native diagnostic snapshot, with the same exclusive custody as recv.
    /// Contains no private payloads. Production builds retain these timings too.
    pub fn timings(&self) -> (Vec<timing::CallTiming>, Option<timing::CallTiming>) {
        self.timings.snapshot()
    }
    #[cfg(feature = "ffi-test-hooks")]
    pub fn hold_recv_pump(&mut self, hold: Arc<timing::obstruction::PumpHold>) {
        self.recv_pump_hold = Some(hold);
    }
    fn timed(
        &mut self,
        send: bool,
        call: impl FnOnce(&mut Self, &mut timing::CallTiming) -> i32,
    ) -> i32 {
        let mut observation = self.timings.begin(send);
        let result = catch_unwind(AssertUnwindSafe(|| call(self, &mut observation)));
        self.timings
            .finish(observation, result.as_ref().copied().unwrap_or(-2));
        match result {
            Ok(status) => status,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
    fn pump(&mut self, now: u64, call: &mut timing::CallTiming) -> crate::Result<()> {
        call.pump_entry_us = Some(self.timings.now());
        #[cfg(feature = "ffi-test-hooks")]
        if !call.send
            && let Some(hold) = &self.recv_pump_hold
        {
            hold.enter();
        }
        let result = self.session.pump_dispatch(now);
        call.pump_exit_us = Some(self.timings.now());
        result
    }
    fn fenced(&self) -> bool {
        FENCED.load(Ordering::SeqCst) || self.generation != GENERATION.load(Ordering::SeqCst)
    }
}

/// Configure the process's immutable supervisor binding from UTF-8 JSON.
/// Repeating identical configuration is idempotent; replacing identity requires
/// a new runtime process. This is NOT a surface wire message.
/// # Safety
/// `config` must reference `n` readable bytes for this call, with no concurrent mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_runtime_configure(config: *const u8, n: usize) -> i32 {
    guarded(|| {
        if FENCED.load(Ordering::SeqCst) {
            return -2;
        }
        if config.is_null() || n == 0 || n > SURFACE_MAX_FRAME {
            return -1;
        }
        let raw = unsafe { slice::from_raw_parts(config, n) };
        let Ok(binding) = SurfaceBinding::from_config(raw) else {
            return -1;
        };
        match BINDING.set(binding.clone()) {
            Ok(()) => 0,
            Err(_) if BINDING.get() == Some(&binding) => 0,
            Err(_) => -1,
        }
    })
}

/// Open an unnegotiated link in the already-configured runtime.
/// # Safety
/// `out` must be null or point to writable, aligned storage for one handle pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_open(
    abi_version: u32,
    out: *mut *mut CogletSurfaceLink,
) -> i32 {
    guarded(|| {
        if out.is_null() {
            return -1;
        }
        unsafe {
            out.write(ptr::null_mut());
        }
        if FENCED.load(Ordering::SeqCst) {
            return -2;
        }
        if abi_version != 1 {
            return -1;
        }
        let Some(binding) = BINDING.get() else {
            return -1;
        };
        let Ok(runtime) = RUNTIME.lock() else {
            return -2;
        };
        let session = if let Some(service) = &*runtime {
            match SurfaceSession::with_service(binding.clone(), service.clone()) {
                Ok(session) => session,
                Err(_) => return -2,
            }
        } else {
            SurfaceSession::new(binding.clone())
        };
        let link = Box::new(CogletSurfaceLink {
            session,
            clock: Instant::now(),
            generation: GENERATION.load(Ordering::SeqCst),
            timings: timing::History::default(),
            #[cfg(feature = "ffi-test-hooks")]
            recv_pump_hold: None,
        });
        unsafe {
            out.write(Box::into_raw(link));
        }
        0
    })
}

/// Copy and validate one deterministic CBOR message. No caller pointer is retained.
/// # Safety
/// `link` must be a live handle, exclusively accessed for the call; `cbor` must
/// reference `n` readable bytes without concurrent mutation. No calls after close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_send(
    link: *mut CogletSurfaceLink,
    cbor: *const u8,
    n: usize,
) -> i32 {
    guarded(|| {
        if link.is_null() || cbor.is_null() || n == 0 || n > SURFACE_MAX_FRAME {
            return -1;
        }
        let link = unsafe { &mut *link };
        link.timed(true, |link, call| {
            if link.fenced() {
                return -2;
            }
            let Ok(now) = link.session.transport_now(link.clock) else {
                link.session.disconnect();
                return -2;
            };
            call.clock_us = Some(link.timings.now());
            let status = link
                .session
                .send(unsafe { slice::from_raw_parts(cbor, n) }, now);
            if status == 0 && link.pump(now, call).is_err() {
                link.session.disconnect();
                return -2;
            }
            status
        })
    })
}

/// Poll one response; short buffers and size queries do not dequeue it.
/// # Safety
/// `link` must be live and exclusively accessed. `n` points to writable usize.
/// `out` points to `cap` writable bytes, or is null with cap=0 for size query.
/// Output buffers must not alias the handle or each other.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_recv(
    link: *mut CogletSurfaceLink,
    out: *mut u8,
    cap: usize,
    n: *mut usize,
) -> i32 {
    guarded(|| {
        if link.is_null() || n.is_null() || (out.is_null() && cap != 0) {
            return -1;
        }
        unsafe {
            n.write(0);
        }
        let link = unsafe { &mut *link };
        link.timed(false, |link, call| {
            if link.fenced() {
                return -2;
            }
            let Ok(now) = link.session.transport_now(link.clock) else {
                link.session.disconnect();
                return -2;
            };
            call.clock_us = Some(link.timings.now());
            link.session.tick(now);
            call.tick_us = Some(link.timings.now());
            call.output_depth = link.session.output_depth();
            if let Some(bytes) = link.session.front() {
                call.ready_us = Some(link.timings.now());
                call.ready_bytes = bytes.len();
                call.output_kind = timing::OutputKind::of_valid_frame(bytes);
            }
            // ADR-0034: tick above is mandatory (expiry/revocation/inactivity).
            // Once its front is eligible, do not put optional guest/fsync work
            // ahead of either a size query, short retry or the exact copy.
            if link.session.front().is_none() && link.pump(now, call).is_err() {
                link.session.disconnect();
                return -2;
            }
            let Some(bytes) = link.session.front() else {
                return if link.session.is_closed() { -2 } else { 1 };
            };
            if call.ready_us.is_none() {
                call.ready_us = Some(link.timings.now());
                call.ready_bytes = bytes.len();
                call.output_depth = link.session.output_depth();
                call.output_kind = timing::OutputKind::of_valid_frame(bytes);
            }
            unsafe {
                n.write(bytes.len());
            }
            if cap < bytes.len() {
                return 2;
            }
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
            }
            link.session.pop();
            0
        })
    })
}

/// Apply disconnect and free the handle. Null is a no-op. Fenced links are
/// discarded without calling into uncertain service state.
/// # Safety
/// Non-null `link` must be a live handle from open, with no concurrent access.
/// The handle must never be used again, including a second close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_close(link: *mut CogletSurfaceLink) {
    guarded(|| {
        if !link.is_null() {
            let mut link = unsafe { Box::from_raw(link) };
            if !link.fenced() {
                link.session.disconnect();
            }
        }
        0
    });
}

/// TEST BUILDS ONLY: inject an unwind while holding both runtime ownership and
/// service locks, exercising the poisoned-state path through an actual C entry.
/// Not present in ordinary release artifacts; never exposed as guest authority.
#[cfg(feature = "ffi-test-hooks")]
#[unsafe(no_mangle)]
pub extern "C" fn coglet_runtime_test_panic() -> i32 {
    guarded(|| {
        let runtime = RUNTIME.lock().unwrap();
        let _service = runtime.as_ref().map(|s| s.lock().unwrap());
        panic!("injected recoverable FFI panic");
    })
}

#[cfg(all(test, feature = "ffi-test-hooks"))]
mod tests {
    use super::*;
    #[test]
    fn caught_panic_fences_all_links_and_stop_discards_poison() {
        let raw = br#"{"surface":"surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","permitted_coglets":[]}"#;
        assert_eq!(
            unsafe { coglet_runtime_configure(raw.as_ptr(), raw.len()) },
            0
        );
        let mut link = ptr::null_mut();
        assert_eq!(unsafe { coglet_surface_open(1, &mut link) }, 0);
        // A real queued hello reply must not bypass inactivity preflight.
        let hello = crate::cbor(&serde_json::json!({"type":"hello","body":{
            "surface":"surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "clock":0,"offer":{"versions":["0.1"]},"correlation_id":"1".repeat(32)
        }}))
        .unwrap();
        assert_eq!(
            unsafe { coglet_surface_send(link, hello.as_ptr(), hello.len()) },
            0
        );
        assert!(unsafe { &*link }.session.front().is_some());
        unsafe { &mut *link }.clock = Instant::now() - std::time::Duration::from_secs(4);
        let mut expired_n = 123;
        assert_eq!(
            unsafe { coglet_surface_recv(link, ptr::null_mut(), 0, &mut expired_n) },
            -2
        );
        assert_eq!(expired_n, 0);
        assert!(unsafe { &*link }.session.front().is_none());
        unsafe { coglet_surface_close(link) };
        assert_eq!(unsafe { coglet_surface_open(1, &mut link) }, 0);
        assert_eq!(
            unsafe { coglet_surface_send(link, hello.as_ptr(), hello.len()) },
            0
        );
        assert!(unsafe { &*link }.session.front().is_some());
        // Keep ready output queued across the panic and stop-generation checks.
        assert_eq!(coglet_runtime_test_panic(), -2);
        assert!(RUNTIME.is_poisoned());
        let mut n = 123;
        assert_eq!(
            unsafe { coglet_surface_recv(link, ptr::null_mut(), 0, &mut n) },
            -2
        );
        assert_eq!(n, 0);
        let mut refused = ptr::null_mut();
        assert_eq!(unsafe { coglet_surface_open(1, &mut refused) }, -2);
        assert!(refused.is_null());
        assert_eq!(coglet_runtime_stop(), 0);
        // Even a negotiation-only handle cannot cross a stop/restart generation.
        assert_eq!(
            unsafe { coglet_surface_recv(link, ptr::null_mut(), 0, &mut n) },
            -2
        );
        unsafe {
            coglet_surface_close(link);
        }
        assert!(!RUNTIME.is_poisoned());
        assert_eq!(unsafe { coglet_surface_open(1, &mut link) }, 0);
        unsafe {
            coglet_surface_close(link);
        }
        assert_eq!(coglet_runtime_stop(), 0);

        // Also poison an actual durable service, not just the global slot.
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
        let setup = &cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "no-effects")
            .unwrap()["steps"][0]["send"];
        let dir = std::env::temp_dir().join(format!(
            "ffi-panic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let config =
            serde_json::to_vec(&serde_json::json!({"wal":dir.join("state.wal"),"setup":setup}))
                .unwrap();
        assert_eq!(
            unsafe { coglet_runtime_start(config.as_ptr(), config.len()) },
            0
        );
        assert_eq!(unsafe { coglet_surface_open(1, &mut link) }, 0);
        assert_eq!(coglet_runtime_test_panic(), -2);
        {
            let runtime = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
            assert!(runtime.as_ref().unwrap().is_poisoned());
        }
        assert_eq!(coglet_runtime_stop(), 1);
        assert_eq!(
            unsafe { coglet_runtime_start(config.as_ptr(), config.len()) },
            -2
        );
        unsafe {
            coglet_surface_close(link);
        }
        assert_eq!(coglet_runtime_stop(), 0);
        assert_eq!(
            unsafe { coglet_runtime_start(config.as_ptr(), config.len()) },
            0
        );
        assert_eq!(coglet_runtime_stop(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
