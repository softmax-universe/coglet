//! Bounded, non-private C-call timings. Durations share one per-link monotonic
//! epoch; they are not adapter clocks and contain no frame bytes or identities.
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default)]
pub struct CallTiming {
    pub sequence: u64,
    pub send: bool,
    pub entry_us: u64,
    pub clock_us: Option<u64>,
    pub tick_us: Option<u64>,
    pub ready_us: Option<u64>,
    pub ready_bytes: usize,
    pub pump_entry_us: Option<u64>,
    pub pump_exit_us: Option<u64>,
    pub return_us: u64,
    pub status: i32,
}

pub(crate) struct History {
    epoch: Instant,
    next: u64,
    recent: [Option<CallTiming>; 16],
    first_failure: Option<CallTiming>,
}
impl Default for History {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            next: 0,
            recent: [None; 16],
            first_failure: None,
        }
    }
}
impl History {
    pub fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(u64::MAX)
    }
    pub fn begin(&mut self, send: bool) -> CallTiming {
        self.next = self.next.saturating_add(1);
        CallTiming {
            sequence: self.next,
            send,
            entry_us: self.now(),
            ..CallTiming::default()
        }
    }
    pub fn finish(&mut self, mut call: CallTiming, status: i32) {
        call.return_us = self.now();
        call.status = status;
        self.recent[(call.sequence % 16) as usize] = Some(call);
        // A slow call is diagnostic, not a new protocol timeout. Keep the first
        // fatal or >=1s residence, including success delayed behind a pump.
        if self.first_failure.is_none()
            && (status < 0 || call.return_us.saturating_sub(call.entry_us) >= 1_000_000)
        {
            self.first_failure = Some(call);
        }
    }
    pub fn snapshot(&self) -> (Vec<CallTiming>, Option<CallTiming>) {
        let mut recent: Vec<_> = self.recent.iter().flatten().copied().collect();
        recent.sort_by_key(|c| c.sequence);
        (recent, self.first_failure)
    }
}

#[cfg(feature = "ffi-test-hooks")]
pub mod obstruction {
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    /// Per-link test hold at the optional recv pump boundary. A finite watchdog
    /// and independent release avoid using close/stop concurrently with a handle.
    #[derive(Default)]
    pub struct PumpHold {
        state: Mutex<(usize, bool)>,
        changed: Condvar,
    }
    impl PumpHold {
        pub fn entries(&self) -> usize {
            self.state.lock().unwrap().0
        }
        pub fn release(&self) {
            self.state.lock().unwrap().1 = true;
            self.changed.notify_all();
        }
        pub(crate) fn enter(&self) {
            let mut state = self.state.lock().unwrap();
            state.0 += 1;
            self.changed.notify_all();
            let (state, timeout) = self
                .changed
                .wait_timeout_while(state, Duration::from_secs(8), |state| !state.1)
                .unwrap();
            assert!(
                !timeout.timed_out() || state.1,
                "recv pump obstruction cleanup watchdog"
            );
        }
    }
}
