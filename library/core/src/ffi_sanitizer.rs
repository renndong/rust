
#[allow(missing_docs)]
#[allow(unused_imports)]
// use crate::alloc;
// use crate::core::ptr;
use crate::marker::PointeeSized;

unsafe extern "C" {
    fn __ffi_sanitizer_get_metadata(pointer: *mut u8) -> i32;
}

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_non_null"]
pub unsafe fn ffi_sanitizer_non_null<T: PointeeSized>(pointer: *mut T) {
    if pointer.is_null() {
        panic!("FFI Sanitizer: pointer should non null");
    }
}

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_non_free"]
pub unsafe fn ffi_sanitizer_non_free<T: PointeeSized>(pointer: *mut T) {
    let raw = pointer as *mut u8;
    let metadata = unsafe {
        __ffi_sanitizer_get_metadata(raw)
    };
    if metadata != 0 {
        panic!("non free");
    }
}
