use crate::libffisan::{F_ALLOC_C, F_ALLOC_R, F_FREE_R, header};
#[allow(missing_docs)]
#[allow(unused_imports)]
// use crate::alloc;
// use crate::core::ptr;
use crate::marker::PointeeSized;
use crate::{ffi, libffisan};

#[inline]
unsafe fn ffi_sanitizer_header_ref<T: PointeeSized>(
    pointer: *const T,
) -> Option<&'static mut header> {
    let raw = pointer as *mut ffi::c_void;
    let header_ptr = unsafe { libffisan::__ffi_sanitizer_get_header(raw) };
    if header_ptr.is_null() {
        return None;
    }

    unsafe { Some(&mut *header_ptr) }
}

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_non_null"]
pub unsafe fn ffi_sanitizer_non_null<T: PointeeSized>(pointer: *const T) {
    if pointer.is_null() {
        panic!("FFI Sanitizer: pointer should non null");
    }
}

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_non_free"]
pub unsafe fn ffi_sanitizer_non_free<T: PointeeSized>(pointer: *mut T) {
    let raw = pointer as *mut ffi::c_void;
    let metadata = unsafe { libffisan::__ffi_sanitizer_get_header(raw) };
    if metadata.is_null() {
        panic!("non free");
    }
}

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_ffi_pre_cond"]
pub unsafe fn ffi_sanitizer_ffi_pre_cond<T: PointeeSized>(pointer: *const T) {
    if pointer.is_null() {
        return;
    }
    unsafe {
        let Some(header) = ffi_sanitizer_header_ref(pointer) else { return };
        if header.cps.f_free() != 0 {
            panic!("FFI Sanitizer: object is freed before ffi call");
        }
        libffisan::__ffi_sanitizer_put_alloc_list(pointer as *mut ffi::c_void);
    }
}

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_ffi_post_cond"]
pub unsafe fn ffi_sanitizer_ffi_post_cond<T: PointeeSized>(pointer: *const T) {
    if pointer.is_null() {
        return;
    }
    unsafe {
        let Some(header) = ffi_sanitizer_header_ref(pointer) else { return };
        if header.cps.f_free() != header.cps.f_alloc() {
            let alloc_lang = if header.cps.f_alloc() == F_ALLOC_R { "rust" } else { "C" };
            let free_lang = if header.cps.f_free() == F_FREE_R { "rust" } else { "C" };
            panic!(
                "FFI Sanitizer: undefined behavior detected: object alloced in {} but freed in {}",
                alloc_lang, free_lang
            );
        }
    }
}

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_drop_pre_cond"]
pub unsafe fn ffi_sanitizer_drop_pre_cond<T: PointeeSized>(pointer: *const T) {
    unsafe {
        let Some(header) = ffi_sanitizer_header_ref(pointer) else { return };
        if header.cps.f_free() == F_ALLOC_R || header.cps.f_free() == F_ALLOC_C {
            panic!("FFI Sanitizer: double free detected");
        }
    }
}

#[track_caller]
#[unstable(feature = "ffi_sanitizer", issue = "none")]
#[rustc_diagnostic_item = "ffi_sanitizer_use_pre_cond"]
pub unsafe fn ffi_sanitizer_use_pre_cond<T: PointeeSized>(pointer: *const T) {
    unsafe {
        let Some(header) = ffi_sanitizer_header_ref(pointer) else { return };
        if header.cps.f_free() == F_ALLOC_R || header.cps.f_free() == F_ALLOC_C {
            panic!("FFI Sanitizer: use-after-free detected");
        }
    }
}