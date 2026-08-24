//! Measure allocations performed by one `split_sentences` call.
//!
//! Run with:
//!
//! ```text
//! cargo run --release --example allocation_metrics
//! ```
//!
//! The counters are reset after each workload has been warmed up. Formatting
//! and printing happen only after the counters have been read, so output from
//! this example cannot be included in a measurement.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use bunkai_rs::split_sentences;

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid layout, and System is the
        // process allocator used for the actual allocation.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid layout, and System is the
        // process allocator used for the actual allocation.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The pointer and layout are those returned by this allocator.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The pointer and layout are those returned by this allocator;
        // the requested new size is supplied by the Vec allocation machinery.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            // A realloc is an allocation event from the caller's point of
            // view. Count the resulting size so growth costs are visible.
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size, Ordering::Relaxed);
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

struct Workload {
    name: &'static str,
    text: String,
}

#[derive(Clone, Copy)]
struct Metrics {
    bytes: usize,
    allocations: usize,
    allocated_bytes: usize,
}

fn measure(text: &str) -> Metrics {
    // Prime lazy runtime/library state and allocator paths before resetting
    // the counters used for the reported call.
    for _ in 0..8 {
        std::hint::black_box(split_sentences(text));
    }
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);

    let result = split_sentences(text);
    std::hint::black_box(&result);
    let metrics = Metrics {
        bytes: text.len(),
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
        allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
    };
    drop(result);
    metrics
}

fn workloads() -> [Workload; 5] {
    [
        Workload {
            name: "empty",
            text: String::new(),
        },
        Workload {
            name: "tiny",
            text: "A".to_owned(),
        },
        Workload {
            name: "no-boundary",
            text: "This is one ASCII sentence with no terminal punctuation."
                .trim_end_matches('.')
                .to_owned(),
        },
        Workload {
            name: "ascii-heavy",
            text: "The quick brown fox jumps over the lazy dog. ".repeat(128),
        },
        Workload {
            name: "medium",
            text: "宿を予約しました😄まだ2ヶ月も先だけど。早すぎかな(笑)楽しみです★\n".repeat(128),
        },
    ]
}

fn main() {
    let workloads = workloads();
    let mut results = [(
        "",
        Metrics {
            bytes: 0,
            allocations: 0,
            allocated_bytes: 0,
        },
    ); 5];

    for (slot, workload) in results.iter_mut().zip(&workloads) {
        *slot = (workload.name, measure(&workload.text));
    }

    println!("workload             input      allocations/call  allocated-bytes/call");
    for (name, metrics) in results {
        println!(
            "{name:<18} {:>7} B {:>17} {:>22}",
            metrics.bytes, metrics.allocations, metrics.allocated_bytes
        );
    }
}
