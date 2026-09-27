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
    let document = vec![b'a'; 40_000];
    let patterns = [
        ("word-120", r"\w{120}".to_string()),
        ("word-150", r"\w{150}".to_string()),
        ("word-200", r"\w{200}".to_string()),
        ("letter-120", r"\p{L}{120}".to_string()),
        ("letter-150", r"\p{L}{150}".to_string()),
        ("letter-200", r"\p{L}{200}".to_string()),
    ];
    for (name, pattern) in patterns {
        let baseline = LIVE.load(Ordering::SeqCst);
        PEAK.store(baseline, Ordering::SeqCst);
        let result = RegexExecutor::compile(&pattern);
        let compile_peak = PEAK.load(Ordering::SeqCst) - baseline;
        match result {
            Ok(executor) => {
                let compile_live = LIVE.load(Ordering::SeqCst) - baseline;
                let matched = executor.verify(&document);
                let verify_peak = PEAK.load(Ordering::SeqCst) - baseline;
                let verify_live = LIVE.load(Ordering::SeqCst) - baseline;
                drop(executor);
                println!("{name}\t{}	OK\t{compile_live}\t{compile_peak}\t{verify_live}\t{verify_peak}\t{matched}", pattern.len());
            }
            Err(error) => println!("{name}\t{}	ERR\t{}\t{compile_peak}", pattern.len(), error),
        }
    }
}
