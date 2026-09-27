use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use roaring::RoaringBitmap;
struct Tracker;
static LIVE: AtomicIsize = AtomicIsize::new(0);
unsafe impl GlobalAlloc for Tracker {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() { LIVE.fetch_add(layout.size() as isize, Ordering::SeqCst); }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::SeqCst);
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, old: Layout, new: usize) -> *mut u8 {
        let next = unsafe { System.realloc(p, old, new) };
        if !next.is_null() { LIVE.fetch_add(new as isize-old.size() as isize, Ordering::SeqCst); }
        next
    }
}
#[global_allocator] static ALLOC: Tracker = Tracker;
fn main() {
    println!("warmup");
    for count in [3u32, 513, 1025] {
        let before = LIVE.load(Ordering::SeqCst);
        let mut set = RoaringBitmap::new();
        for id in 0..count { set.insert(id << 16); }
        let after = LIVE.load(Ordering::SeqCst);
        let stats = set.statistics();
        let accounted = 32u64 + stats.n_bytes_array_containers + stats.n_bytes_run_containers
            + u64::from(stats.n_bitset_containers)*8192 + u64::from(stats.n_containers)*32;
        println!("count={count} live_heap={} accounted={accounted} containers={} array_bytes={} run_bytes={}", after-before, stats.n_containers, stats.n_bytes_array_containers, stats.n_bytes_run_containers);
        std::hint::black_box(set);
    }
}
