#![no_std]
//! Small single-owner ABI around the same ARM/Sonix models as the native app.
//! No desktop libraries, file system, network stack or ROM-sized allocations.
extern crate alloc;
#[cfg(not(target_arch = "xtensa"))]
extern crate std;

pub mod emulator;
pub use emulator::mmu::flash;
#[cfg(feature = "recomp")]
mod recomp;
mod runtime;
pub use runtime::*;

#[cfg(target_arch = "xtensa")]
mod platform {
    use core::alloc::{GlobalAlloc, Layout};
    extern "C" {
        fn h717_alloc(size: usize, align: usize) -> *mut u8;
        fn h717_free(ptr: *mut u8, size: usize, align: usize);
        fn h717_panic() -> !;
    }
    struct Heap;
    unsafe impl GlobalAlloc for Heap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            h717_alloc(layout.size(), layout.align())
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            h717_free(ptr, layout.size(), layout.align());
        }
    }
    #[global_allocator]
    static HEAP: Heap = Heap;
    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! { unsafe { h717_panic() } }
}
