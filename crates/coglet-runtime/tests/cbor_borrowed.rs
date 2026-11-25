//! Allocation regression for the borrowed JSON-to-profile encoder.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;
thread_local! {
    static LARGE_ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

fn count(size: usize) {
    if size >= 1024 * 1024 {
        let _ = LARGE_ALLOCATIONS.try_with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn large_text_is_copied_only_into_the_output() {
    for value in [
        serde_json::Value::String("x".repeat(2 * 1024 * 1024)),
        serde_json::json!({"x".repeat(2 * 1024 * 1024): null}),
    ] {
        LARGE_ALLOCATIONS.with(|count| count.set(Some(0)));
        let encoded = coglet_runtime::cbor(&value);
        let allocations = LARGE_ALLOCATIONS.with(|count| count.replace(None).unwrap());
        assert!(encoded.unwrap().len() > 2 * 1024 * 1024);
        assert_eq!(
            allocations, 1,
            "only the output may allocate a large buffer"
        );
    }
}
