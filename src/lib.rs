pub mod api;
pub mod core;
pub mod error;
pub mod physics;
pub mod spatial;
pub mod types;
pub mod weather;

#[cfg(test)]
pub(crate) mod allocation_test {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
        static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    struct CountingAllocator;

    // The system allocator performs the allocation; thread-local counters only observe it.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            TRACK_ALLOCATIONS.with(|tracking| {
                if tracking.get() {
                    ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
                }
            });
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            TRACK_ALLOCATIONS.with(|tracking| {
                if tracking.get() {
                    ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
                }
            });
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            TRACK_ALLOCATIONS.with(|tracking| {
                if tracking.get() {
                    ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
                }
            });
            unsafe { System.realloc(pointer, layout, new_size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    pub(crate) fn start() {
        ALLOCATION_COUNT.with(|count| count.set(0));
        TRACK_ALLOCATIONS.with(|tracking| tracking.set(true));
    }

    pub(crate) fn stop() -> usize {
        TRACK_ALLOCATIONS.with(|tracking| tracking.set(false));
        ALLOCATION_COUNT.with(Cell::get)
    }
}
