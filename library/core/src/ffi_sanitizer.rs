
#[allow(missing_docs)]
#[allow(unused_imports)]
// use crate::alloc;
// use crate::core::ptr;
use crate::marker::PointeeSized;
use crate::libffisan;
use crate::ffi;

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
    let raw = pointer as *mut ffi::c_void;
    let metadata = unsafe {
        libffisan::__ffi_sanitizer_get_header(raw)
    };
    if metadata.is_null() {
        panic!("non free");
    }
}
