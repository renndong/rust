#[allow(missing_docs)]
#[allow(unused_imports)]
// use crate::alloc;
// use crate::core::ptr;
use crate::marker::PointeeSized;

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_non_null"]
pub const unsafe fn ffi_sanitizer_non_null<T: PointeeSized>(pointer: *mut T) {
    if pointer.is_null() {
        panic!("FFI Sanitizer: pointer should non null");
    }
}
