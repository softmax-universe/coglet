//! Runtime-epoch continuity, independent of transport connection lifetime.
//! Linux BOOTTIME and Apple continuous time include suspension and downtime.
//! Kernel boot identities change across reboot and fail closed; civil-clock
//! guesses never renew retention.
use crate::{Result, RuntimeService};
use serde::{Deserialize, Serialize};

#[cfg(test)]
pub(super) type TestClockSample = std::sync::Arc<dyn Fn() -> Result<(String, u64)> + Send + Sync>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClockAnchor {
    boot: String,
    boot_micros: u64,
    runtime_micros: u64,
}
impl ClockAnchor {
    fn at(&self, boot: &str, micros: u64) -> Result<u64> {
        if boot != self.boot {
            return Err("runtime clock continuity unavailable after reboot".into());
        }
        self.runtime_micros
            .checked_add(
                micros
                    .checked_sub(self.boot_micros)
                    .ok_or("boot clock rollback")?,
            )
            .ok_or_else(|| "runtime clock overflow".into())
    }
}
#[cfg(target_os = "linux")]
fn sample() -> Result<(String, u64)> {
    let boot =
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|e| e.to_string())?;
    let boot = boot.trim().to_owned();
    if boot.is_empty() {
        return Err("missing boot identity".into());
    }
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: clock_gettime initializes the complete timespec on success.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, time.as_mut_ptr()) } != 0 {
        return Err("BOOTTIME unavailable".into());
    }
    let time = unsafe { time.assume_init() };
    let seconds = u64::try_from(time.tv_sec).map_err(|_| "negative boot clock")?;
    let nanos = u64::try_from(time.tv_nsec).map_err(|_| "negative boot clock")?;
    let micros = seconds
        .checked_mul(1_000_000)
        .and_then(|s| s.checked_add(nanos / 1000))
        .ok_or("boot clock overflow")?;
    Ok((boot, micros))
}
// ADR-0039: iOS cannot read the kernel boot UUID in an application sandbox.
// Cache failures too: no caller may substitute a new epoch after entropy failure.
#[cfg(any(target_os = "ios", test))]
fn process_epoch(
    epoch: &std::sync::OnceLock<Result<String>>,
    entropy: impl FnOnce(&mut [u8; 32]) -> Result<()>,
) -> Result<String> {
    epoch
        .get_or_init(|| {
            let mut bytes = [0; 32];
            entropy(&mut bytes)?;
            Ok(format!("ios-process:{}", hex::encode(bytes)))
        })
        .clone()
}
#[cfg(any(target_os = "ios", test))]
fn os_entropy(bytes: &mut [u8; 32]) -> Result<()> {
    // SAFETY: getentropy receives a writable buffer below its 256-byte limit.
    if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err("process epoch entropy unavailable".into());
    }
    Ok(())
}
#[cfg(any(target_os = "ios", test))]
fn ios_process_epoch() -> Result<String> {
    static EPOCH: std::sync::OnceLock<Result<String>> = std::sync::OnceLock::new();
    process_epoch(&EPOCH, os_entropy)
}
#[cfg(target_os = "ios")]
fn sample() -> Result<(String, u64)> {
    Ok((ios_process_epoch()?, apple_continuous_micros()?))
}
#[cfg(target_os = "macos")]
fn sample() -> Result<(String, u64)> {
    // Boot-session UUID is stable across process restarts and wall-clock edits.
    // Do not fall back to kern.boottime, which can move when civil time changes.
    let mut boot = [0u8; 128];
    let mut len = boot.len();
    // SAFETY: sysctl writes no more than len bytes into the supplied buffer.
    if unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            boot.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || len != 37
        || boot[36] != 0
    {
        return Err("kernel boot-session UUID unavailable".into());
    }
    let boot = std::str::from_utf8(&boot[..36]).map_err(|_| "invalid boot-session UUID")?;
    if !boot.bytes().enumerate().all(|(i, b)| {
        if [8, 13, 18, 23].contains(&i) {
            b == b'-'
        } else {
            b.is_ascii_hexdigit()
        }
    }) {
        return Err("invalid boot-session UUID".into());
    }
    Ok((format!("darwin:{boot}"), apple_continuous_micros()?))
}
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn apple_continuous_micros() -> Result<u64> {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_continuous_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut scale = Timebase { numer: 0, denom: 0 };
    // SAFETY: timebase_info receives a valid writable struct; the continuous
    // clock has no preconditions and includes sleep (macOS 10.12 / iOS 10+).
    let ticks = unsafe {
        if mach_timebase_info(&mut scale) != 0 || scale.denom == 0 || scale.numer == 0 {
            return Err("continuous clock timebase unavailable".into());
        }
        mach_continuous_time()
    };
    let micros =
        u64::try_from(u128::from(ticks) * u128::from(scale.numer) / u128::from(scale.denom) / 1000)
            .map_err(|_| "boot clock overflow")?;
    Ok(micros)
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn sample() -> Result<(String, u64)> {
    Err("supervisor clock continuity is not implemented on this platform".into())
}
impl RuntimeService {
    /// Resample an established native epoch after persistence. Logical callers
    /// own their clock; never create an anchor or write the WAL at publication.
    pub(crate) fn read_publication_now(&self, entry: u64) -> Result<u64> {
        let Some(anchor) = &self.state.clock_anchor else {
            return Ok(entry);
        };
        if !self.store.is_healthy() {
            return Err("runtime requires WAL recovery".into());
        }
        let (boot, micros) = self.clock_sample()?;
        let now = anchor.at(&boot, micros)?;
        if now < entry || now < self.state.runtime.time {
            return Err("read publication clock rollback".into());
        }
        Ok(now)
    }

    fn clock_sample(&self) -> Result<(String, u64)> {
        #[cfg(test)]
        if let Some(sample) = &self.test_clock_sample {
            return sample();
        }
        sample()
    }

    /// Activate a durable native clock once, or recover the same epoch. Refuse
    /// to infer an epoch for already prepared dispatches from an older runtime.
    pub fn supervised_now(&mut self) -> Result<u64> {
        if !self.store.is_healthy() {
            return Err("runtime requires WAL recovery".into());
        }
        let (boot, micros) = self.clock_sample()?;
        self.supervised_at(boot, micros)
    }
    // Keep the recovery policy identical across OS samplers. Tests inject only
    // the kernel sample, never bypass WAL reopen/dispatch validation.
    fn supervised_at(&mut self, boot: String, micros: u64) -> Result<u64> {
        if self.state.clock_anchor.is_none() {
            if !self.state.dispatches.is_empty() {
                return Err("existing dispatches have no supervised clock anchor".into());
            }
            let mut next = self.state.clone();
            next.clock_anchor = Some(ClockAnchor {
                boot: boot.clone(),
                boot_micros: micros,
                runtime_micros: next.runtime.time,
            });
            self.store.commit(
                &serde_json::to_value(&next).map_err(|e| e.to_string())?,
                None,
            )?;
            self.state = next;
        }
        let now = self
            .state
            .clock_anchor
            .as_ref()
            .unwrap()
            .at(&boot, micros)?;
        if now < self.state.runtime.time {
            return Err("supervised clock is behind durable runtime".into());
        }
        Ok(now)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn fixture(name: &str) -> (std::path::PathBuf, Value) {
        let cases: Value =
            serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
        let mut setup = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "no-effects")
            .unwrap()["steps"][0]["send"]
            .clone();
        let action = json!({"coglet":setup["manifest"]["id"],"id":"4".repeat(32),"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":50_000_000,"correlation_id":"5".repeat(32)});
        setup["initial_actions"] = json!([action]);
        setup["initial_effect_ids"]["actions"] = json!(["4".repeat(32)]);
        let dir = std::env::temp_dir().join(format!("clock-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        (dir, setup)
    }
    include!("clock_process_tests.rs");
    #[test]
    fn same_boot_wal_recovery_preserves_epoch_and_elapsed_time() {
        let (dir, setup) = fixture("same-boot");
        let path = dir.join("wal");
        let mut service = RuntimeService::open(&path, &setup, 128).unwrap();
        let first = service.supervised_at("boot-a".into(), 100).unwrap();
        drop(service);
        let mut recovered = RuntimeService::open(&path, &setup, 128).unwrap();
        assert_eq!(
            recovered.supervised_at("boot-a".into(), 1_000_100).unwrap(),
            first + 1_000_000
        );
        drop(recovered);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn prepared_dispatch_recovery_refuses_new_boot_and_missing_anchor() {
        let (dir, setup) = fixture("prepared");
        let path = dir.join("wal");
        let mut service = RuntimeService::open(&path, &setup, 128).unwrap();
        service.supervised_at("boot-a".into(), 100).unwrap();
        let surface = format!("surface:{}", "b".repeat(64));
        let coglet = setup["manifest"]["id"].as_str().unwrap();
        let mut session = crate::SurfaceSession::new(
            crate::SurfaceBinding::new(surface.clone(), vec![coglet.into()]).unwrap(),
        );
        for frame in [
            json!({"type":"hello","body":{"surface":surface,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}}),
            json!({"type":"capabilities","body":{"revision":1,"clock":1000,"entries":[{"name":"expression.control","events":[],"reads":[],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}}],"correlation_id":"2".repeat(32)}}),
            json!({"type":"grants","body":{"epoch":1,"entries":[{"coglet":coglet,"operations":["expression.set_expression"],"resources":["display.face"],"expires":50_000_000}],"correlation_id":"3".repeat(32)}}),
        ] {
            assert_eq!(session.send(&crate::cbor(&frame).unwrap(), 100), 0);
        }
        let raw = service
            .prepare_dispatch(&session, &"4".repeat(32), 100)
            .unwrap();
        drop(service);
        let mut recovered = RuntimeService::open(&path, &setup, 128).unwrap();
        assert!(
            recovered
                .supervised_at("boot-b".into(), 200)
                .unwrap_err()
                .contains("reboot")
        );
        assert_eq!(recovered.supervised_at("boot-a".into(), 200).unwrap(), 100);
        assert_eq!(
            recovered
                .prepare_dispatch(&session, &"4".repeat(32), 200)
                .unwrap(),
            raw
        );
        // Persist an older-runtime WAL with prepared bytes but no anchor.
        recovered.state.clock_anchor = None;
        recovered
            .store
            .commit(&serde_json::to_value(&recovered.state).unwrap(), None)
            .unwrap();
        drop(recovered);
        let mut legacy = RuntimeService::open(&path, &setup, 128).unwrap();
        assert!(
            legacy
                .supervised_at("boot-a".into(), 300)
                .unwrap_err()
                .contains("no supervised clock anchor")
        );
        drop(legacy);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn native_boot_sample_is_stable_and_monotonic() {
        let (boot, first) = sample().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (again, next) = sample().unwrap();
        assert_eq!(boot, again);
        assert!(next > first);
    }
    #[test]
    fn clock_accounts_for_downtime_and_refuses_reboot_rollback_overflow() {
        let a = ClockAnchor {
            boot: "boot-a".into(),
            boot_micros: 100,
            runtime_micros: 40,
        };
        assert_eq!(a.at("boot-a", 110).unwrap(), 50);
        assert_eq!(a.at("boot-a", 1_000_100).unwrap(), 1_000_040);
        assert!(a.at("boot-b", 110).is_err());
        assert!(a.at("boot-a", 99).is_err());
        let a = ClockAnchor {
            runtime_micros: u64::MAX,
            ..a
        };
        assert!(a.at("boot-a", 101).is_err());
    }
}
