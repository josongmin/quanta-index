use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use quanta_index_lq_regex::RegexExecutor;

struct Count;
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
fn change(delta: isize) {
    let now = LIVE.fetch_add(delta, Ordering::SeqCst) + delta;
    PEAK.fetch_max(now, Ordering::SeqCst);
}
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() { change(layout.size() as isize); }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        change(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(ptr, old, new_size) };
        if !next.is_null() { change(new_size as isize - old.size() as isize); }
        next
    }
}
#[global_allocator]
static ALLOC: Count = Count;
fn main() {
    for groups in [500, 2000, 5000, 10000] {
        let pattern = format!("needle{}", "(a?)".repeat(groups));
        let baseline = LIVE.load(Ordering::SeqCst);
        PEAK.store(baseline, Ordering::SeqCst);
        let result = RegexExecutor::prepare(&pattern);
        let prepare_peak = PEAK.load(Ordering::SeqCst) - baseline;
        match result {
            Ok(plan) => {
                let prepare_live = LIVE.load(Ordering::SeqCst) - baseline;
                println!("{groups}\t{}\t{}\t{prepare_live}\t{prepare_peak}", pattern.len(), plan.estimated_states());
            }
            Err(error) => println!("{groups}\t{}\tERR\t{}\t{prepare_peak}", pattern.len(), error),
        }
    }
}
