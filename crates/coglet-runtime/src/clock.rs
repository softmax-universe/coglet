//! Runtime-epoch continuity, independent of transport connection lifetime.
//! Linux BOOTTIME includes suspension and process downtime. A reboot changes
//! the boot identity and fails closed; wall-clock guesses never renew retention.
use crate::{Result, RuntimeService};
use serde::{Deserialize, Serialize};

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
#[cfg(not(target_os = "linux"))]
fn sample() -> Result<(String, u64)> {
    Err("supervisor clock continuity is not implemented on this platform".into())
}
impl RuntimeService {
    /// Activate a durable native clock once, or recover the same epoch. Refuse
    /// to infer an epoch for already prepared dispatches from an older runtime.
    pub fn supervised_now(&mut self) -> Result<u64> {
        if !self.store.is_healthy() {
            return Err("runtime requires WAL recovery".into());
        }
        let (boot, micros) = sample()?;
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
