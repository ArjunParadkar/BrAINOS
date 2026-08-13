//! Heap for the portable core, backed by UEFI pool allocations.
//!
//! BrAInOS has no filesystem and no processes, but it does have growing
//! structures — the state graph, the body map, KIRA's audit log — so the
//! core carries its own allocator. Domains above the HAL just use
//! `alloc` types and never see where the bytes come from.

use crate::efi::{BootServices, MEM_LOADER_DATA, SUCCESS};
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::null_mut;

struct BsPtr(UnsafeCell<*mut BootServices>);
// single-threaded environment: the experience loop is the only executor
unsafe impl Sync for BsPtr {}

static BS: BsPtr = BsPtr(UnsafeCell::new(null_mut()));

/// Must be called once, before any allocation, with the boot services table.
pub fn init(bs: *mut BootServices) {
    unsafe { *BS.0.get() = bs };
}

pub struct UefiAlloc;

unsafe impl GlobalAlloc for UefiAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let bs = *BS.0.get();
        if bs.is_null() {
            return null_mut();
        }
        let align = layout.align().max(8);
        // UEFI pool guarantees 8-byte alignment; over-allocate for more
        // and stash the original pointer just below the aligned block.
        let total = layout.size() + align + core::mem::size_of::<usize>();
        let mut raw: *mut u8 = null_mut();
        if ((*bs).allocate_pool)(MEM_LOADER_DATA, total, &mut raw) != SUCCESS {
            return null_mut();
        }
        let base = raw as usize + core::mem::size_of::<usize>();
        let aligned = (base + align - 1) & !(align - 1);
        *(aligned as *mut usize).sub(1) = raw as usize;
        aligned as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        let bs = *BS.0.get();
        if bs.is_null() || ptr.is_null() {
            return;
        }
        let raw = *(ptr as *mut usize).sub(1) as *mut u8;
        ((*bs).free_pool)(raw);
    }
}

#[global_allocator]
static ALLOCATOR: UefiAlloc = UefiAlloc;
