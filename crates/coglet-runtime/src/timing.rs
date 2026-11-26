//! Test-hook-only, bounded, in-memory timing ring. Compiled out of production:
//! without `ffi-test-hooks` the `timing_span!` macro expands to nothing. No I/O
//! happens per span; the ring is dumped once, at the first ABI refusal. A bounded
//! cross-thread completed-span summary is exported after stop, including success.
//! Records
//! hold only static labels, a sequence counter, monotonic elapsed time and a
//! raw size: never ids, frame bodies or private values.

#[cfg(feature = "ffi-test-hooks")]
#[macro_export]
#[doc(hidden)]
macro_rules! timing_span {
    ($label:literal) => {
        let _timing_span = $crate::timing::Span::begin($label, 0);
    };
    ($label:literal, $size:expr) => {
        let _timing_span = $crate::timing::Span::begin($label, ($size) as u64);
    };
}
#[cfg(not(feature = "ffi-test-hooks"))]
#[macro_export]
#[doc(hidden)]
macro_rules! timing_span {
    ($($tt:tt)*) => {};
}

#[cfg(feature = "ffi-test-hooks")]
pub(crate) use imp::*;

#[cfg(feature = "ffi-test-hooks")]
mod imp {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::OnceLock;
    use std::time::Instant;

    pub(crate) const RING_CAPACITY: usize = 512;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Edge {
        Begin,
        End,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct Record {
        pub seq: u64,
        pub edge: Edge,
        pub label: &'static str,
        /// Nanoseconds since this thread's first record (monotonic).
        pub elapsed_ns: u64,
        pub size: u64,
    }
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct Snapshot {
        pub dropped: u64,
        pub records: Vec<Record>,
    }

    struct Ring {
        epoch: Instant,
        seq: u64,
        dropped: u64,
        cap: usize,
        records: VecDeque<Record>,
    }
    impl Ring {
        fn new(cap: usize) -> Self {
            Self {
                epoch: Instant::now(),
                seq: 0,
                dropped: 0,
                cap,
                records: VecDeque::with_capacity(cap),
            }
        }
        fn push(&mut self, edge: Edge, label: &'static str, size: u64) {
            if self.records.len() == self.cap {
                self.records.pop_front();
                self.dropped += 1;
            }
            let elapsed_ns = u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.records.push_back(Record {
                seq: self.seq,
                edge,
                label,
                elapsed_ns,
                size,
            });
            self.seq += 1;
        }
        fn snapshot(&self) -> Snapshot {
            Snapshot {
                dropped: self.dropped,
                records: self.records.iter().copied().collect(),
            }
        }
    }
    thread_local! {
        static RING: RefCell<Ring> = RefCell::new(Ring::new(RING_CAPACITY));
    }
    static FIRST: OnceLock<Snapshot> = OnceLock::new();

    fn record(edge: Edge, label: &'static str, size: u64) {
        // try_with/try_borrow_mut: never panic or block, even during TLS teardown.
        let _ = RING.try_with(|ring| {
            if let Ok(mut ring) = ring.try_borrow_mut() {
                ring.push(edge, label, size);
            }
        });
    }

    /// RAII span: records Begin now and End on drop (including early return).
    pub(crate) struct Span {
        label: &'static str,
        size: u64,
        start: Instant,
    }
    impl Span {
        pub(crate) fn begin(label: &'static str, size: u64) -> Self {
            let start = Instant::now();
            record(Edge::Begin, label, size);
            Self { label, size, start }
        }
    }
    impl Drop for Span {
        fn drop(&mut self) {
            let ns = u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            record(Edge::End, self.label, self.size);
            crate::timing_summary::completed(self.label, ns, self.size);
        }
    }

    pub(crate) fn snapshot_current() -> Option<Snapshot> {
        RING.try_with(|r| r.try_borrow().ok().map(|r| r.snapshot()))
            .ok()
            .flatten()
    }

    /// Capture this thread's ring at the FIRST refusal only; later calls are
    /// no-ops. Returns the immutable snapshot when this call was first.
    pub(crate) fn flush_first_refusal(stage: &str) -> Option<&'static Snapshot> {
        let snapshot = snapshot_current()?;
        if FIRST.set(snapshot).is_err() {
            return None;
        }
        let first = FIRST.get()?;
        let mut out = format!(
            "embedded-abi-timing stage={stage} records={} dropped={}\n",
            first.records.len(),
            first.dropped
        );
        for r in &first.records {
            let edge = match r.edge {
                Edge::Begin => "B",
                Edge::End => "E",
            };
            out.push_str(&format!(
                "t {} {edge} {} ns={} size={}\n",
                r.seq, r.label, r.elapsed_ns, r.size
            ));
        }
        eprint!("{out}");
        Some(first)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn fresh_thread<F: FnOnce() + Send + 'static>(f: F) {
            std::thread::spawn(f).join().unwrap();
        }
        fn early(fail: bool) -> Result<(), ()> {
            let _s = Span::begin("early", 7);
            if fail {
                return Err(());
            }
            Ok(())
        }

        #[test]
        fn timing_ring_bounds_retention_and_orders_by_sequence() {
            fresh_thread(|| {
                for _ in 0..(RING_CAPACITY + 10) {
                    record(Edge::Begin, "x", 0);
                }
                let s = snapshot_current().unwrap();
                assert_eq!(s.records.len(), RING_CAPACITY);
                assert_eq!(s.dropped, 10);
                assert_eq!(s.records[0].seq, 10);
                assert!(
                    s.records.windows(2).all(|w| {
                        w[1].seq == w[0].seq + 1 && w[1].elapsed_ns >= w[0].elapsed_ns
                    })
                );
            });
        }
        #[test]
        fn timing_span_records_nested_begin_end_and_open_spans_visible() {
            fresh_thread(|| {
                let open = Span::begin("outer", 1);
                {
                    let _inner = Span::begin("inner", 2);
                }
                let s = snapshot_current().unwrap();
                let shape: Vec<_> = s.records.iter().map(|r| (r.edge, r.label)).collect();
                assert_eq!(
                    shape,
                    [
                        (Edge::Begin, "outer"),
                        (Edge::Begin, "inner"),
                        (Edge::End, "inner")
                    ]
                );
                drop(open);
                assert_eq!(snapshot_current().unwrap().records.len(), 4);
            });
        }
        #[test]
        fn timing_span_ends_on_error_return() {
            fresh_thread(|| {
                assert!(early(true).is_err());
                assert!(early(false).is_ok());
                let s = snapshot_current().unwrap();
                let edges: Vec<_> = s.records.iter().map(|r| r.edge).collect();
                assert_eq!(edges, [Edge::Begin, Edge::End, Edge::Begin, Edge::End]);
                assert_eq!(s.records[1].size, 7);
            });
        }
        #[test]
        fn timing_first_refusal_snapshot_is_taken_once() {
            fresh_thread(|| {
                {
                    let _s = Span::begin("a", 0);
                }
                let first = flush_first_refusal("t1").expect("first");
                assert_eq!(first.records.len(), 2);
                {
                    let _s = Span::begin("b", 0);
                }
                assert!(flush_first_refusal("t2").is_none());
                assert_eq!(FIRST.get().unwrap().records.len(), 2);
            });
        }
    }
}
