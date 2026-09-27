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
    let patterns = [
        ("greek-fold-120", r"(?i)needle\p{Greek}{120}".to_string()),
        ("letters-fold-120", r"(?i)needle\p{L}{120}".to_string()),
        ("any-fold-1000", r"(?i)needle\p{Any}{1000}".to_string()),
        ("word-fold-120", r"(?i)needle\w{120}".to_string()),
        ("greek-1000", r"needle\p{Greek}{1000}".to_string()),
        ("ascii-class-10000", r"needle[a-z]{10000}".to_string()),
        ("digits-10000", r"needle[0-9]{10000}".to_string()),
        ("alt-1000", format!("needle(?:{})", (0..1000).map(|n| format!("arm{n}")).collect::<Vec<_>>().join("|"))),
    ];
    for (name, pattern) in patterns {
        let baseline=LIVE.load(Ordering::SeqCst);
        PEAK.store(baseline,Ordering::SeqCst);
        match RegexExecutor::prepare(&pattern) {
            Ok(plan) => {
                let states=plan.estimated_states();
                let prep=PEAK.load(Ordering::SeqCst)-baseline;
                match RegexExecutor::compile_prepared(plan) {
                    Ok(_exec) => {
                        let peak=PEAK.load(Ordering::SeqCst)-baseline;
                        let charge=(16*1024*1024+256*states) as isize;
                        println!("{name}\t{}\t{states}\t{prep}\t{peak}\t{charge}\t{}",pattern.len(),peak<=charge);
                    }
                    Err(e)=>println!("{name}\t{}\t{states}\t{prep}\tERR {e}",pattern.len()),
                }
            }
            Err(e)=>println!("{name}\t{}\tERR {e}",pattern.len()),
        }
    }
}
