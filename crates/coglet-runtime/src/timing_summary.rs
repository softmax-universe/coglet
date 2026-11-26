//! Completed-span diagnostics only; no protocol clock or production behavior.
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::ThreadId;

const CAPACITY: usize = 64;
#[derive(Clone, Copy, Debug)]
struct Entry {
    label: &'static str,
    count: u64,
    total_ns: u64,
    total_size: u64,
    max_ns: u64,
    max_size: u64,
    max_drop_thread: ThreadId,
}
#[derive(Clone)]
struct Data {
    entries: [Option<Entry>; CAPACITY],
    overflow: u64,
}
impl Data {
    const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            overflow: 0,
        }
    }
    fn record(&mut self, label: &'static str, ns: u64, size: u64, thread: ThreadId) {
        let slot = self
            .entries
            .iter()
            .position(|e| e.is_some_and(|e| e.label == label))
            .or_else(|| self.entries.iter().position(Option::is_none));
        let Some(slot) = slot else {
            self.overflow = self.overflow.saturating_add(1);
            return;
        };
        let e = self.entries[slot].get_or_insert(Entry {
            label,
            count: 0,
            total_ns: 0,
            total_size: 0,
            max_ns: ns,
            max_size: size,
            max_drop_thread: thread,
        });
        e.count = e.count.saturating_add(1);
        e.total_ns = e.total_ns.saturating_add(ns);
        e.total_size = e.total_size.saturating_add(size);
        // First occurrence wins equal-duration ties; all max metadata stays together.
        if ns > e.max_ns {
            e.max_ns = ns;
            e.max_size = size;
            e.max_drop_thread = thread;
        }
    }
    fn format(&self, export: u64, dropped: u64, status: i32) -> String {
        let labels = self.entries.iter().flatten().count();
        let mut out = format!(
            "embedded-abi-summary export={export} stop_status={status} scope=completed-spans cumulative=true open_spans=excluded ordering=none totals=inclusive labels={labels} dropped_contention={dropped} overflow={} complete={}\n",
            self.overflow,
            dropped == 0 && self.overflow == 0
        );
        for e in self.entries.iter().flatten() {
            out.push_str(&format!("s {} count={} total_ns={} total_size={} max_ns={} max_size={} max_drop_thread={:?}\n", e.label, e.count, e.total_ns, e.total_size, e.max_ns, e.max_size, e.max_drop_thread));
        }
        out
    }
}
struct Aggregate {
    data: Mutex<Data>,
    dropped: AtomicU64,
    exports: AtomicU64,
}
impl Aggregate {
    const fn new() -> Self {
        Self {
            data: Mutex::new(Data::new()),
            dropped: AtomicU64::new(0),
            exports: AtomicU64::new(0),
        }
    }
    fn record(&self, label: &'static str, ns: u64, size: u64, thread: ThreadId) {
        // Fixed array: no allocations or waiting while holding this lock.
        if let Ok(mut data) = self.data.try_lock() {
            data.record(label, ns, size, thread);
        } else {
            // Native Apple toolchain predates try_update; fetch_update is its
            // identical supported API, deprecated only on newer Rust hosts.
            #[allow(deprecated)]
            let _ = self
                .dropped
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    Some(n.saturating_add(1))
                });
        }
    }
    fn export(&self, status: i32) {
        let export = self.exports.fetch_add(1, Ordering::Relaxed);
        let snapshot = self.data.try_lock().ok().map(|data| data.clone());
        // Never format or write with an aggregate or runtime lock held.
        let out = match snapshot {
            Some(data) => data.format(export, self.dropped.load(Ordering::Relaxed), status),
            None => format!(
                "embedded-abi-summary export={export} stop_status={status} complete=false snapshot=unavailable\n"
            ),
        };
        let _ = std::io::stderr().write_all(out.as_bytes());
    }
}
static AGGREGATE: Aggregate = Aggregate::new();
pub(crate) fn completed(label: &'static str, ns: u64, size: u64) {
    AGGREGATE.record(label, ns, size, std::thread::current().id());
}
fn preserve_result(status: i32, export: impl FnOnce()) -> i32 {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(export));
    status
}
pub(crate) fn export_after_stop(status: i32) -> i32 {
    preserve_result(status, || AGGREGATE.export(status))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_summary_retains_early_slow_span_after_ring_sized_traffic() {
        let mut d = Data::new();
        let t = std::thread::current().id();
        d.record("slow", 5_000_000_000, 7, t);
        for _ in 0..2048 {
            d.record("slow", 1, 999, t);
        }
        let e = d.entries[0].unwrap();
        assert_eq!((e.count, e.max_ns, e.max_size), (2049, 5_000_000_000, 7));
        assert_eq!(e.total_ns, 5_000_002_048);
    }
    #[test]
    fn completed_summary_max_occurrence_thread_and_tie_are_coherent() {
        let other = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        let t = std::thread::current().id();
        let mut d = Data::new();
        d.record("nested", 10, 3, other);
        d.record("nested", 9, 1000, t);
        d.record("nested", 10, 2000, t);
        let e = d.entries[0].unwrap();
        assert_eq!((e.max_ns, e.max_size, e.max_drop_thread), (10, 3, other));
        d.record("nested", 11, 2, t);
        let e = d.entries[0].unwrap();
        assert_eq!((e.max_ns, e.max_size, e.max_drop_thread), (11, 2, t));
    }
    #[test]
    fn completed_summary_bound_overflow_and_saturation() {
        let mut d = Data::new();
        let t = std::thread::current().id();
        // Populate fixed slots without leaking test strings for static labels.
        d.entries.fill(Some(Entry {
            label: "existing",
            count: u64::MAX,
            total_ns: u64::MAX,
            total_size: u64::MAX,
            max_ns: 1,
            max_size: 1,
            max_drop_thread: t,
        }));
        d.record("new", 1, 1, t);
        assert_eq!(d.overflow, 1);
        d.record("existing", 2, 2, t);
        let e = d.entries[0].unwrap();
        assert_eq!(
            (e.count, e.total_ns, e.total_size),
            (u64::MAX, u64::MAX, u64::MAX)
        );
        assert!(d.format(0, 0, 0).contains("overflow=1 complete=false"));
    }
    #[test]
    fn completed_summary_empty_and_repeated_snapshots_do_not_reset() {
        let a = Aggregate::new();
        let empty = a.data.lock().unwrap().clone();
        assert!(
            empty
                .format(0, 0, 0)
                .contains("labels=0 dropped_contention=0 overflow=0 complete=true")
        );
        a.record("outer", 12, 1, std::thread::current().id());
        a.record("inner", 7, 1, std::thread::current().id());
        let before = a.data.lock().unwrap().format(1, 0, 1);
        a.export(1);
        assert_eq!(before, a.data.lock().unwrap().format(1, 0, 1));
        assert!(before.contains("totals=inclusive"));
        assert_eq!(a.exports.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn completed_summary_contention_is_nonblocking_and_visible() {
        let a = std::sync::Arc::new(Aggregate::new());
        let _held = a.data.lock().unwrap();
        let b = a.clone();
        std::thread::spawn(move || b.record("blocked", 1, 0, std::thread::current().id()))
            .join()
            .unwrap();
        assert_eq!(a.dropped.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn completed_summary_export_panic_never_changes_stop_status() {
        for status in [0, 1, -2] {
            assert_eq!(
                preserve_result(status, || panic!("injected export panic")),
                status
            );
        }
    }
}
