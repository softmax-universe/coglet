//! Trusted application embedding API. Handles are owned/serialized by the caller.
use crate::{SURFACE_MAX_FRAME, SurfaceBinding, SurfaceSession};
use std::{
    ptr, slice,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

static BINDING: OnceLock<SurfaceBinding> = OnceLock::new();
static RUNTIME: Mutex<Option<Arc<Mutex<crate::RuntimeService>>>> = Mutex::new(None);

/// Start/recover the durable coordinator from trusted JSON {wal, setup}.
/// Call before opening links. Non-Linux clocks currently fail closed.
/// # Safety
/// `config` references `n` readable immutable bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_runtime_start(config: *const u8, n: usize) -> i32 {
    if config.is_null() || n == 0 || n > SURFACE_MAX_FRAME || BINDING.get().is_none() {
        return -1;
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        wal: std::path::PathBuf,
        setup: serde_json::Value,
        #[serde(default)]
        native_schedule: Option<serde_json::Value>,
    }
    let Ok(config) = serde_json::from_slice::<Config>(unsafe { slice::from_raw_parts(config, n) })
    else {
        return -1;
    };
    if !config.wal.is_absolute() {
        return -1;
    }
    let Ok(mut runtime) = RUNTIME.lock() else {
        return -2;
    };
    if runtime.is_some() {
        return -1;
    }
    let result =
        crate::RuntimeService::open(&config.wal, &config.setup, 128).and_then(|mut service| {
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
            *runtime = Some(Arc::new(Mutex::new(service)));
            0
        }
        Err(_) => -2,
    }
}
/// Stop only after all links close. Returns 1 while links hold the runtime.
#[unsafe(no_mangle)]
pub extern "C" fn coglet_runtime_stop() -> i32 {
    let Ok(mut runtime) = RUNTIME.lock() else {
        return -2;
    };
    if runtime.as_ref().is_some_and(|r| Arc::strong_count(r) != 1) {
        return 1;
    }
    *runtime = None;
    0
}

#[repr(C)]
pub struct CogletSurfaceLink {
    session: SurfaceSession,
    clock: Instant,
}

/// Configure the process's immutable supervisor binding from UTF-8 JSON.
/// Repeating identical configuration is idempotent; replacing identity requires
/// a new runtime process. This is NOT a surface wire message.
/// # Safety
/// `config` must reference `n` readable bytes for this call, with no concurrent mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_runtime_configure(config: *const u8, n: usize) -> i32 {
    if config.is_null() || n == 0 || n > SURFACE_MAX_FRAME {
        return -1;
    }
    // SAFETY: caller supplies the readable buffer per the function contract.
    let raw = unsafe { slice::from_raw_parts(config, n) };
    let Ok(binding) = SurfaceBinding::from_config(raw) else {
        return -1;
    };
    match BINDING.set(binding.clone()) {
        Ok(()) => 0,
        Err(_) if BINDING.get() == Some(&binding) => 0,
        Err(_) => -1,
    }
}

/// Open an unnegotiated link in the already-configured runtime.
/// # Safety
/// `out` must be null or point to writable, aligned storage for one handle pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_open(
    abi_version: u32,
    out: *mut *mut CogletSurfaceLink,
) -> i32 {
    if out.is_null() {
        return -1;
    }
    // SAFETY: caller supplies writable pointer storage.
    unsafe {
        out.write(ptr::null_mut());
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
    });
    // SAFETY: caller supplies writable pointer storage; ownership transfers.
    unsafe {
        out.write(Box::into_raw(link));
    }
    0
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
    if link.is_null() || cbor.is_null() || n == 0 || n > SURFACE_MAX_FRAME {
        return -1;
    }
    // SAFETY: handle and readable input are guaranteed by caller.
    let link = unsafe { &mut *link };
    let Ok(now) = link.session.transport_now(link.clock) else {
        link.session.disconnect();
        return -2;
    };
    let status = link
        .session
        .send(unsafe { slice::from_raw_parts(cbor, n) }, now);
    if status == 0 && link.session.pump_dispatch(now).is_err() {
        link.session.disconnect();
        return -2;
    }
    status
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
    if link.is_null() || n.is_null() || (out.is_null() && cap != 0) {
        return -1;
    }
    // SAFETY: writable n and exclusive handle required by caller.
    unsafe {
        n.write(0);
    }
    let link = unsafe { &mut *link };
    let Ok(now) = link.session.transport_now(link.clock) else {
        link.session.disconnect();
        return -2;
    };
    link.session.tick(now);
    if link.session.pump_dispatch(now).is_err() {
        link.session.disconnect();
        return -2;
    }
    let Some(bytes) = link.session.front() else {
        return if link.session.is_closed() { -2 } else { 1 };
    };
    unsafe {
        n.write(bytes.len());
    }
    if cap < bytes.len() {
        return 2;
    }
    // SAFETY: cap is sufficient and caller promises nonoverlapping output.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
    }
    link.session.pop();
    0
}

/// Apply disconnect and free the handle. Null is a no-op.
/// # Safety
/// Non-null `link` must be a live handle from open, with no concurrent access.
/// The handle must never be used again, including a second close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_close(link: *mut CogletSurfaceLink) {
    if !link.is_null() {
        // SAFETY: unique ownership of an open handle is required by caller.
        let mut link = unsafe { Box::from_raw(link) };
        link.session.disconnect();
    }
}
