//! Deterministic 1,000-step coglet-shaped guest.
//!
//! ABI (plain core Wasm, no imports in the conforming build):
//!   alloc_events(len) -> ptr        scratch buffer for the event batch
//!   handle(ptr, len, step, logical_time, seed) -> u64   runs one step, returns FNV-1a hash of state
//!   state_ptr() / state_len()       the state blob, so the host can hash it independently
//!
//! Event record (16 bytes, little endian): kind u8, 3 pad, a f32 bits, b f64 bits.
//! State blob: 32 x f32 (bits) then 32 x f64 (bits) then 4 x u64 counters. All hashed as raw bits,
//! so NaN payloads and signed zeros are visible to the hash.
#![allow(static_mut_refs)]

const NF32: usize = 32;
const NF64: usize = 32;
const STATE_BYTES: usize = NF32 * 4 + NF64 * 8 + 32;
const EV_BYTES: usize = 16;
const EV_BUF: usize = 4096;

static mut EVENTS: [u8; EV_BUF] = [0; EV_BUF];
static mut STATE: [u8; STATE_BYTES] = [0; STATE_BYTES];
static mut X: [f32; NF32] = [0.0; NF32];
static mut Y: [f64; NF64] = [0.0; NF64];
static mut CTR: [u64; 4] = [0; 4];
static mut INIT: bool = false;

#[cfg(any(feature = "wasi-clock", feature = "wasi-random"))]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" {
    #[cfg(feature = "wasi-clock")]
    fn clock_time_get(id: u32, precision: u64, out: *mut u64) -> u32;
    #[cfg(feature = "wasi-random")]
    fn random_get(buf: *mut u8, len: u32) -> u32;
}

fn splitmix(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[no_mangle]
pub extern "C" fn alloc_events(len: u32) -> u32 {
    if len as usize > EV_BUF {
        return 0;
    }
    unsafe { EVENTS.as_mut_ptr() as u32 }
}

#[no_mangle]
pub extern "C" fn state_ptr() -> u32 {
    unsafe { STATE.as_ptr() as u32 }
}

#[no_mangle]
pub extern "C" fn state_len() -> u32 {
    STATE_BYTES as u32
}

#[inline(never)]
fn apply_event(kind: u8, a: f32, b: f64, i: usize, rng: &mut u64) {
    let (x, y) = unsafe { (&mut X, &mut Y) };
    let i = i % NF32;
    let j = (i * 7 + 3) % NF64;
    match kind % 12 {
        0 => x[i] = x[i] * 0.5 + a,
        1 => y[j] = y[j] + b * 1.0e-3 + x[i] as f64,
        2 => x[i] = (x[i].abs() + a.abs()).sqrt(), // NaN if an input is NaN; sqrt instruction
        3 => y[j] = (y[j] - b).sqrt(),             // NaN-producing: sqrt of a negative
        4 => x[i] = (x[i] - x[i]) / (a - a),       // 0/0 -> NaN
        5 => x[i] = a * f32::INFINITY - a * f32::INFINITY, // inf - inf -> NaN
        6 => y[j] = (y[j] + b).sin() * (b * 0.25).cos() + y[j].mul_add(0.5, b),
        7 => x[i] = x[i].floor() + a.ceil() - a.trunc() + a.round_ties_even(), // payload through rounding ops
        8 => y[j] = (x[i] as f64) * y[j] + b, // f32 -> f64 promote, NaN payload
        9 => x[i] = (y[j] as f32) + a,        // f64 -> f32 demote
        10 => {
            x[i] = -x[i].copysign(a)
                + f32::from_bits(a.to_bits() ^ (splitmix(rng) as u32 & 0x007f_ffff))
        }
        _ => y[j] = y[j].abs().ln_1p() + (b.abs() + 1.0).powf(0.37) + b.exp().min(1.0e300),
    }
    // Keep finite lanes from running away, but leave NaN lanes alone: they carry payloads
    // across steps, which is what makes the state hash sensitive to NaN bit patterns.
    if x[i] > 1.0e30 {
        x[i] = 1.0e30;
    }
    if y[j] > 1.0e300 {
        y[j] = 1.0e300;
    }
}

#[cfg(feature = "relaxed-simd")]
fn relaxed(seed: u64, step: u32) {
    use core::arch::wasm32::*;
    unsafe {
        let a = f32x4(1.0000001, 3.3333333e7, 1.0e-8, -2.5);
        let b = f32x4(1.0e7 + step as f32, 3.0000001, 1.0e8, 4.0e-1);
        let c = f32x4(-1.0e7, -1.0e8 * (seed & 3) as f32, 1.0, f32::NAN);
        // fused vs unfused multiply-add is implementation-defined under relaxed-simd.
        let m = f32x4_relaxed_madd(a, b, c);
        // out-of-range / NaN lanes are implementation-defined too.
        let big = f32x4(3.0e9, f32::NAN, -3.0e9, 1.0);
        let t = i32x4_relaxed_trunc_f32x4(big);
        X[0] = f32x4_extract_lane::<0>(m);
        X[1] = f32x4_extract_lane::<1>(m);
        X[2] = f32x4_extract_lane::<2>(m);
        X[3] = f32x4_extract_lane::<3>(m);
        CTR[3] ^= (i32x4_extract_lane::<0>(t) as u32 as u64)
            | ((i32x4_extract_lane::<1>(t) as u32 as u64) << 32);
        CTR[3] ^= (i32x4_extract_lane::<2>(t) as u32 as u64).rotate_left(7);
    }
}

#[cfg(feature = "threads")]
fn atomics(step: u32) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static A: AtomicU32 = AtomicU32::new(0);
    let v = A.fetch_add(step, Ordering::SeqCst);
    unsafe { CTR[3] ^= v as u64 };
}

#[no_mangle]
pub extern "C" fn handle(ptr: u32, len: u32, step: u32, logical_time: u64, seed: u64) -> u64 {
    unsafe {
        if !INIT {
            for k in 0..NF32 {
                X[k] = (k as f32 + 1.0) * 0.125;
            }
            for k in 0..NF64 {
                Y[k] = (k as f64 + 1.0) * 0.0625;
            }
            INIT = true;
        }
        let mut rng = seed ^ logical_time.rotate_left(17) ^ ((step as u64) << 40);
        let buf = core::slice::from_raw_parts(ptr as *const u8, len as usize);
        let n = buf.len() / EV_BYTES;
        for e in 0..n {
            let r = &buf[e * EV_BYTES..(e + 1) * EV_BYTES];
            let a = f32::from_bits(u32::from_le_bytes([r[4], r[5], r[6], r[7]]));
            let b = f64::from_bits(u64::from_le_bytes([
                r[8], r[9], r[10], r[11], r[12], r[13], r[14], r[15],
            ]));
            let idx = (splitmix(&mut rng) % 1024) as usize;
            apply_event(r[0], a, b, idx, &mut rng);
        }
        CTR[0] = step as u64;
        CTR[1] = logical_time;
        CTR[2] = seed;
        CTR[3] = CTR[3].wrapping_add(n as u64);

        #[cfg(feature = "wasi-clock")]
        {
            let mut t: u64 = 0;
            clock_time_get(1, 1, &mut t); // CLOCK_MONOTONIC
            CTR[3] ^= t;
        }
        #[cfg(feature = "wasi-random")]
        {
            let mut r = [0u8; 8];
            random_get(r.as_mut_ptr(), 8);
            CTR[3] ^= u64::from_le_bytes(r);
        }
        #[cfg(feature = "relaxed-simd")]
        relaxed(seed, step);
        #[cfg(feature = "threads")]
        atomics(step);

        let mut o = 0;
        for k in 0..NF32 {
            STATE[o..o + 4].copy_from_slice(&X[k].to_bits().to_le_bytes());
            o += 4;
        }
        for k in 0..NF64 {
            STATE[o..o + 8].copy_from_slice(&Y[k].to_bits().to_le_bytes());
            o += 8;
        }
        for k in 0..4 {
            STATE[o..o + 8].copy_from_slice(&CTR[k].to_le_bytes());
            o += 8;
        }
        fnv1a(&STATE)
    }
}
