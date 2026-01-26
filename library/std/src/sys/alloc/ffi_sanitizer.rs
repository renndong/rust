use core::libffisan;

use super::{MIN_ALIGN, realloc_fallback};
use crate::alloc::{GlobalAlloc, Layout, System};
use crate::ptr;

#[stable(feature = "alloc_system_type", since = "1.28.0")]
unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= MIN_ALIGN && layout.align() <= layout.size() {
            unsafe { libffisan::__ffi_sanitizer_rust_alloc(layout.size()) as *mut u8 }
        } else {
            unsafe { aligned_malloc(&layout) }
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // See the comment above in `alloc` for why this check looks the way it does.
        if layout.align() <= MIN_ALIGN && layout.align() <= layout.size() {
            unsafe { libffisan::__ffi_sanitizer_rust_calloc(layout.size(), 1) as *mut u8 }
        } else {
            let ptr = unsafe { self.alloc(layout) };
            if !ptr.is_null() {
                unsafe { ptr::write_bytes(ptr, 0, layout.size()) };
            }
            ptr
        }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { libffisan::__ffi_sanitizer_rust_free(ptr as *mut libc::c_void) }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= MIN_ALIGN && layout.align() <= new_size {
            unsafe {
                libffisan::__ffi_sanitizer_rust_realloc(ptr as *mut libc::c_void, new_size)
                    as *mut u8
            }
        } else {
            unsafe { realloc_fallback(self, ptr, layout, new_size) }
        }
    }
}

unsafe fn aligned_malloc(layout: &Layout) -> *mut u8 {
    let mut out = ptr::null_mut();
    // We prefer posix_memalign over aligned_alloc since it is more widely available, and
    // since with aligned_alloc, implementations are making almost arbitrary choices for
    // which alignments are "supported", making it hard to use. For instance, some
    // implementations require the size to be a multiple of the alignment (wasi emmalloc),
    // while others require the alignment to be at least the pointer size (Illumos, macOS).
    // posix_memalign only has one, clear requirement: that the alignment be a multiple of
    // `sizeof(void*)`. Since these are all powers of 2, we can just use max.
    let align = layout.align().max(size_of::<usize>());
    let ret =
        unsafe { libffisan::__ffi_sanitizer_rust_posix_memalign(&mut out, align, layout.size()) };
    if ret != 0 { ptr::null_mut() } else { out as *mut u8 }
}
