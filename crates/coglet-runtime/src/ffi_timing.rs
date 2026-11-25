//! Bounded, non-private C-call timings. Durations share one per-link monotonic
//! epoch; they are not adapter clocks and contain no frame bytes or identities.
use std::time::Instant;

/// Diagnostic labels only; not a wire parser or an authority decision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum OutputKind {
    #[default]
    None,
    Health,
    Read,
    Act,
    HelloReply,
    Error,
    Other,
}
impl OutputKind {
    pub(crate) fn of_valid_frame(bytes: &[u8]) -> Self {
        // Internally generated core-deterministic CBOR has exactly body/type;
        // the shorter body key sorts first, so the type field is the suffix.
        // Inspect a constant number of bytes, never decode/copy private bodies.
        for (suffix, kind) in [
            (b"\x64type\x66health".as_slice(), Self::Health),
            (b"\x64type\x64read".as_slice(), Self::Read),
            (b"\x64type\x63act".as_slice(), Self::Act),
            (b"\x64type\x6bhello_reply".as_slice(), Self::HelloReply),
            (b"\x64type\x65error".as_slice(), Self::Error),
        ] {
            if bytes.ends_with(suffix) {
                return kind;
            }
        }
        Self::Other
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct CallTiming {
    pub sequence: u64,
    pub send: bool,
    pub entry_us: u64,
    pub clock_us: Option<u64>,
    pub tick_us: Option<u64>,
    pub ready_us: Option<u64>,
    pub ready_bytes: usize,
    pub output_depth: usize,
    pub output_kind: OutputKind,
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
    pub fn finish(&mut self, call: CallTiming, status: i32) {
        self.finish_at(call, status, self.now());
    }
    fn finish_at(&mut self, mut call: CallTiming, status: i32, now: u64) {
        call.return_us = now;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_ring_is_bounded_and_retains_first_failure() {
        let mut history = History::default();
        for sequence in 1..=100 {
            let call = history.begin(false);
            assert_eq!(call.sequence, sequence);
            let returned = call.entry_us + if sequence == 4 { 1_000_000 } else { 1 };
            history.finish_at(call, if sequence == 8 { -2 } else { 0 }, returned);
        }
        let (recent, first) = history.snapshot();
        assert_eq!(recent.len(), 16);
        assert_eq!(recent.first().unwrap().sequence, 85);
        assert_eq!(recent.last().unwrap().sequence, 100);
        assert_eq!(first.unwrap().sequence, 4);
        assert_eq!(first.unwrap().status, 0);
        let mut fatal = History::default();
        let call = fatal.begin(true);
        fatal.finish_at(call, -2, call.entry_us + 1);
        assert_eq!(fatal.snapshot().1.unwrap().status, -2);
    }

    #[test]
    fn output_kind_matches_real_encoded_frames_without_private_payloads() {
        for (name, expected) in [
            ("health", OutputKind::Health),
            ("read", OutputKind::Read),
            ("act", OutputKind::Act),
            ("hello_reply", OutputKind::HelloReply),
            ("error", OutputKind::Error),
            ("unknown", OutputKind::Other),
        ] {
            let bytes = crate::cbor(&serde_json::json!({
                "type":name,"body":{"private":"not retained in timing"}
            }))
            .unwrap();
            assert_eq!(OutputKind::of_valid_frame(&bytes), expected);
        }
        assert_eq!(OutputKind::of_valid_frame(&[]), OutputKind::Other);
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
            self.state.lock().unwrap_or_else(|e| e.into_inner()).0
        }
        pub fn release(&self) {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
            self.changed.notify_all();
        }
        pub(crate) fn enter(&self) {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
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
