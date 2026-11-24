use super::*;
use coglet_engine::{Error, GuestMemory, StepBudget};
use std::cell::Cell as Counter;

// Only the requested ranges exist in the test host. A full-linear-memory copy
// would fail, not silently allocate a small vector and pass.
struct SparseMemory {
    input: Vec<u8>,
    output: Vec<u8>,
    copied: Counter<u64>,
    fail_write: bool,
}
const INPUT: u32 = 0xffff_0000;
const OUTPUT: u32 = 0x8000_0000;
impl GuestMemory for SparseMemory {
    fn memory_len(&self) -> u64 {
        1u64 << 32
    }
    fn read(&self, ptr: u32, len: u32) -> coglet_engine::Result<Vec<u8>> {
        assert_eq!((ptr, len), (INPUT, self.input.len() as u32));
        self.copied.set(self.copied.get() + u64::from(len));
        Ok(self.input.clone())
    }
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> coglet_engine::Result<()> {
        if self.fail_write {
            return Err(Error::DeadlineExceeded);
        }
        let offset = ptr.checked_sub(OUTPUT).expect("output pointer") as usize;
        self.output[offset..offset + bytes.len()].copy_from_slice(bytes);
        self.copied.set(self.copied.get() + bytes.len() as u64);
        Ok(())
    }
}
fn memory() -> SparseMemory {
    SparseMemory {
        input: request("scan", "read"),
        output: vec![0x55; 65540],
        copied: Counter::new(0),
        fail_write: false,
    }
}
fn budget(n: u64) -> StepBudget {
    StepBudget::new(1_000_000, n, Instant::now() + Duration::from_secs(10)).unwrap()
}
#[test]
fn sparse_wasm32_memory_charges_production_once_and_only_copies_ranges() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let mut unit = begin(&mut db, &cat);
    let mut memory = memory();
    let mut budget = budget(1_000_000);
    let result = unit
        .call_engine(
            &mut memory,
            [
                INPUT as i32,
                request("scan", "read").len() as i32,
                OUTPUT as i32,
                65540,
            ],
            &mut budget,
        )
        .unwrap();
    let CallResult::Return {
        result: 0,
        response: Some(response),
    } = result
    else {
        panic!("missing DB reply")
    };
    assert_eq!(budget.host_bytes_used(), response.len() as u64);
    budget.charge_host_bytes(memory.copied.get()).unwrap(); // actual engine settlement
    assert_eq!(
        budget.host_bytes_used(),
        memory.input.len() as u64 + 2 * response.len() as u64 + 4
    );
    assert_eq!(&memory.output[..4], &(response.len() as u32).to_be_bytes());
    assert_eq!(&memory.output[4..4 + response.len()], &response);
    assert!(
        memory.output[4 + response.len()..]
            .iter()
            .all(|b| *b == 0x55)
    );
    assert_eq!(unit.revision(), 0);
}

#[test]
fn engine_bridge_reserves_maximum_reply_before_starting_sql() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let mut unit = begin(&mut db, &cat);
    let mut memory = memory();
    let n = memory.input.len() as u64;
    let mut budget = budget(n + 2 * 65536 + 3);
    assert_eq!(
        unit.call_engine(
            &mut memory,
            [INPUT as i32, n as i32, OUTPUT as i32, 65540],
            &mut budget
        )
        .unwrap(),
        CallResult::Return {
            result: 3,
            response: None
        }
    );
    assert!(!unit.transaction);
    assert!(unit.poisoned);
    assert_eq!(unit.calls, 1);
    assert_eq!(memory.copied.get(), n);
    assert_eq!(budget.host_bytes_used(), 0); // engine will settle input
    assert!(memory.output.iter().all(|b| *b == 0x55));
}

#[test]
fn engine_bridge_invalid_pointer_and_capacity_never_copy() {
    for args in [
        [-1, 2, OUTPUT as i32, 65540],
        [INPUT as i32, 10, INPUT as i32, 65540],
        [INPUT as i32, 10, OUTPUT as i32, 65539],
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut db = setup(root.path());
        let cat = catalog();
        let mut unit = begin(&mut db, &cat);
        let mut memory = memory();
        let mut budget = budget(1_000_000);
        let result = unit.call_engine(&mut memory, args, &mut budget).unwrap();
        assert!(matches!(
            result,
            CallResult::InvalidPointer
                | CallResult::Return {
                    result: 1,
                    response: None
                }
        ));
        assert_eq!(memory.copied.get(), 0);
        assert_eq!(budget.host_bytes_used(), 0);
        assert_eq!(unit.calls, 1);
        assert!(unit.poisoned);
        assert!(!unit.transaction);
    }
}

#[test]
fn engine_bridge_copy_failure_propagates_and_rolls_back_live_unit() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let mut unit = begin(&mut db, &cat);
    unit.statement(
        "put",
        Mode::Write,
        &[Cell::Text("new".into()), Cell::Text("value".into())],
        64,
        65536,
    )
    .unwrap();
    let mut memory = memory();
    memory.fail_write = true;
    let n = memory.input.len();
    let mut budget = budget(1_000_000);
    assert_eq!(
        unit.call_engine(
            &mut memory,
            [INPUT as i32, n as i32, OUTPUT as i32, 65540],
            &mut budget
        ),
        Err(Error::DeadlineExceeded)
    );
    assert!(unit.poisoned);
    assert!(!unit.transaction);
    assert!(budget.host_bytes_used() > 0); // produced reply remains charged
    assert_eq!(memory.copied.get(), n as u64);
    assert!(memory.output.iter().all(|b| *b == 0x55));
    // Unit stays alive, but neither the provisional write nor its lock survives.
    let other = rusqlite::Connection::open(&unit.db.path).unwrap();
    other.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
    let found: i64 = unit
        .db
        .connection
        .query_row("SELECT count(*) FROM objects WHERE id='new'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(found, 0);
}
