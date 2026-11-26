//! Private, lossy observational evidence; never a runtime clock or authority.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

pub(crate) const CAPACITY: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
// Native-only sites are intentionally absent in non-native builds.
#[allow(dead_code)]
pub(crate) enum Phase {
    SessionStart,
    ReturnError,
    ReturnClosed,
    ReturnEof,
    HeaderEnter,
    HeaderReturn,
    BodyEnter,
    BodyReturn,
    ClockEnter,
    ClockReturn,
    TickEnter,
    TickReturn,
    SendEnter,
    SendReturn,
    PumpEnter,
    PumpReturn,
    WireEnter,
    WireReturn,
    DisconnectEnter,
    DisconnectReturn,
    PrepareEnter,
    PrepareReturn,
    StepEnter,
    StepReturn,
    EngineEnter,
    EngineReturn,
    ReconcileEnter,
    ReconcileReturn,
    SerdeEnter,
    SerdeReturn,
    EncodeEnter,
    EncodeReturn,
    WalWriteEnter,
    WalWriteDone,
    WalSyncEnter,
    WalSyncDone,
    CacheEnter,
    CacheReturn,
    ReadEnter,
    ReadReturn,
    SupervisorEnter,
    SupervisorReturn,
    DispatchEnter,
    DispatchReturn,
    FenceEnter,
    FenceReturn,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    pub(crate) micros: u64,
    pub(crate) phase: Phase,
}
const EMPTY: Entry = Entry {
    micros: 0,
    phase: Phase::SessionStart,
};
struct Ring {
    entries: [Entry; CAPACITY],
    next: usize,
    count: usize,
}
pub(crate) struct Recorder {
    epoch: Instant,
    ring: Mutex<Ring>,
    lost: AtomicU64,
}
pub(crate) type Sink = Arc<Recorder>;
#[derive(Clone, Copy)]
pub(crate) struct Snapshot {
    pub(crate) entries: [Entry; CAPACITY],
    pub(crate) count: usize,
    pub(crate) lost: u64,
    pub(crate) available: bool,
}
impl Recorder {
    pub(crate) fn new() -> Sink {
        Arc::new(Self {
            epoch: Instant::now(),
            ring: Mutex::new(Ring {
                entries: [EMPTY; CAPACITY],
                next: 0,
                count: 0,
            }),
            lost: AtomicU64::new(0),
        })
    }
    #[cfg(test)]
    fn record_at(&self, phase: Phase, micros: u64) {
        let Ok(mut ring) = self.ring.try_lock() else {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        };
        Self::push(&mut ring, phase, micros);
    }
    fn push(ring: &mut Ring, phase: Phase, micros: u64) {
        let index = ring.next;
        ring.entries[index] = Entry { micros, phase };
        ring.next = (index + 1) % CAPACITY;
        ring.count = (ring.count + 1).min(CAPACITY);
    }
    pub(crate) fn snapshot(&self) -> Snapshot {
        let mut snapshot = Snapshot {
            entries: [EMPTY; CAPACITY],
            count: 0,
            lost: self.lost.load(Ordering::Relaxed),
            available: false,
        };
        if let Ok(ring) = self.ring.try_lock() {
            snapshot.available = true;
            snapshot.count = ring.count;
            let first = (ring.next + CAPACITY - ring.count) % CAPACITY;
            for i in 0..ring.count {
                snapshot.entries[i] = ring.entries[(first + i) % CAPACITY];
            }
        }
        snapshot
    }
}
#[inline]
pub(crate) fn mark(sink: &Option<Sink>, phase: Phase) {
    let Some(sink) = sink else {
        return;
    };
    let Ok(mut ring) = sink.ring.try_lock() else {
        sink.lost.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let micros = u64::try_from(sink.epoch.elapsed().as_micros()).unwrap_or(u64::MAX);
    Recorder::push(&mut ring, phase, micros);
}

/// A single best-effort write AFTER teardown: never retry, flush, or replace result.
pub(crate) fn report<T>(
    result: std::result::Result<T, String>,
    snapshot: Option<Snapshot>,
    terminal: bool,
    writer: &mut dyn std::io::Write,
) -> std::result::Result<T, String> {
    if (result.is_err() || terminal)
        && let Some(snapshot) = snapshot
    {
        use std::fmt::Write as _;
        struct Buffer {
            bytes: [u8; 4096],
            count: usize,
        }
        impl std::fmt::Write for Buffer {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                let end = self
                    .count
                    .checked_add(s.len())
                    .filter(|n| *n <= self.bytes.len())
                    .ok_or(std::fmt::Error)?;
                self.bytes[self.count..end].copy_from_slice(s.as_bytes());
                self.count = end;
                Ok(())
            }
        }
        let mut out = Buffer {
            bytes: [0; 4096],
            count: 0,
        };
        let _ = writeln!(
            out,
            "native-phase-v1 available={} lost={} count={}",
            snapshot.available, snapshot.lost, snapshot.count
        );
        // Numeric IDs follow enum declaration order. The fixed map contains no user data.
        let _ = writeln!(
            out,
            "phases=0:start,1:error,2:closed,3:eof,4/5:header,6/7:body,8/9:clock,10/11:tick,12/13:send,14/15:pump,16/17:wire,18/19:disconnect,20/21:prepare,22/23:step,24/25:engine,26/27:reconcile,28/29:serde,30/31:encode,32/33:write,34/35:sync,36/37:cache,38/39:read,40/41:supervisor,42/43:dispatch,44/45:fence; enter/return except write/sync done; aggregate latest64, missing=unknown"
        );
        for entry in &snapshot.entries[..snapshot.count.min(CAPACITY)] {
            let _ = writeln!(out, "{} {}", entry.micros, entry.phase as u8);
        }
        let _ = writer.write(&out.bytes[..out.count]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phase_ring_services_are_isolated_and_contention_is_reported() {
        let a = Recorder::new();
        let b = Recorder::new();
        a.record_at(Phase::EngineEnter, 7);
        assert_eq!(b.snapshot().count, 0);
        let guard = a.ring.lock().unwrap();
        a.record_at(Phase::EngineReturn, 8);
        assert!(!a.snapshot().available);
        drop(guard);
        assert_eq!(a.snapshot().lost, 1);
        assert_eq!(a.snapshot().count, 1);
        let clone = a.clone();
        let _ = std::thread::spawn(move || {
            let _guard = clone.ring.lock().unwrap();
            panic!("test-only poison");
        })
        .join();
        a.record_at(Phase::EngineReturn, 9);
        assert!(!a.snapshot().available);
        assert_eq!(a.snapshot().lost, 2);
    }
    #[test]
    fn phase_failure_output_is_bounded_and_preserves_original_error() {
        let a = Recorder::new();
        for _ in 0..100 {
            a.record_at(Phase::EngineEnter, u64::MAX);
        }
        a.lost.store(u64::MAX, Ordering::Relaxed);
        let mut out = Vec::new();
        let original = "original socket error".to_string();
        let result: std::result::Result<(), String> =
            report(Err(original.clone()), Some(a.snapshot()), true, &mut out);
        assert_eq!(result, Err(original.clone()));
        assert!(!out.is_empty(), "missing bounded failure evidence");
        assert!(out.len() <= 4096);
        assert!(
            std::str::from_utf8(&out)
                .unwrap()
                .contains("18446744073709551615")
        );
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::Interrupted.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                panic!("diagnostics must not flush");
            }
        }
        let result: std::result::Result<(), String> =
            report(Err(original.clone()), Some(a.snapshot()), true, &mut Broken);
        assert_eq!(result, Err(original));
        let mut clean = Vec::new();
        assert!(report(Ok(()), Some(a.snapshot()), false, &mut clean).is_ok());
        assert!(clean.is_empty());
        assert!(report(Ok(()), Some(a.snapshot()), true, &mut clean).is_ok());
        assert!(!clean.is_empty());
    }
    #[test]
    fn phase_ring_retains_latest_64_in_order() {
        let sink = Recorder::new();
        for n in 0..100 {
            sink.record_at(Phase::EngineEnter, n);
        }
        let snapshot = sink.snapshot();
        assert_eq!(snapshot.count, CAPACITY, "missing bounded ring recording");
        assert!(snapshot.available);
        for (i, e) in snapshot.entries.iter().enumerate() {
            assert_eq!(e.micros, i as u64 + 36);
            assert_eq!(e.phase, Phase::EngineEnter);
        }
        assert_eq!(std::mem::size_of::<Entry>(), 16);
    }
}
