use super::{MIN_ALIGN, realloc_fallback};
use crate::alloc::{GlobalAlloc, Layout, System};

unsafe extern "C" {
    fn __ffi_sanitizer_rust_alloc(size: libc::size_t) -> *mut libc::c_void;
    fn __ffi_sanitizer_rust_free(p: *mut libc::c_void);
}

#[stable(feature = "alloc_system_type", since = "1.28.0")]
unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { __ffi_sanitizer_rust_alloc(layout.size()) as *mut u8 }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // See the comment above in `alloc` for why this check looks the way it does.
        // if layout.align() <= MIN_ALIGN && layout.align() <= layout.size() {
        //     unsafe { libc::calloc(layout.size(), 1) as *mut u8 }
        // } else {
        //     let ptr = unsafe { self.alloc(layout) };
        //     if !ptr.is_null() {
        //         unsafe { ptr::write_bytes(ptr, 0, layout.size()) };
        //     }
        //     ptr
        // }
        unsafe { self.alloc(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { __ffi_sanitizer_rust_free(ptr as *mut libc::c_void) }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= MIN_ALIGN && layout.align() <= new_size {
            unsafe { libc::realloc(ptr as *mut libc::c_void, new_size) as *mut u8 }
        } else {
            unsafe { realloc_fallback(self, ptr, layout, new_size) }
        }
    }
}
