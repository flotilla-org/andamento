//! Version 1, single-owner C ABI. Every returned string belongs to the caller
//! until andamento_string_free; request input is only borrowed during the call.
use andamento_core::{sidebar::Request, Sidebar};
use std::{
    ffi::{c_char, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
};

pub struct Andamento {
    sidebar: Sidebar,
    poisoned: bool,
}

fn string(text: String) -> *mut c_char {
    // Errors can contain host text including NUL; always return a valid C string.
    CString::new(text.replace('\0', "\\u0000"))
        .expect("NUL removed")
        .into_raw()
}

unsafe fn input<'a>(data: *const u8, len: usize) -> Result<&'a str, String> {
    if data.is_null() {
        return if len == 0 {
            Ok("")
        } else {
            Err("null input with nonzero length".into())
        };
    }
    std::str::from_utf8(std::slice::from_raw_parts(data, len)).map_err(|e| e.to_string())
}

#[no_mangle]
pub extern "C" fn andamento_abi_version() -> u32 {
    1
}

/// # Safety
/// config points to len readable bytes. error_out is null or a writable pointer.
/// Calls on the returned handle must be serialized by its owner.
#[no_mangle]
pub unsafe extern "C" fn andamento_create(
    config: *const u8,
    len: usize,
    error_out: *mut *mut c_char,
) -> *mut Andamento {
    if !error_out.is_null() {
        *error_out = ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| Sidebar::new(input(config, len)?)))
        .unwrap_or_else(|_| Err("core panicked while creating sidebar".into()));
    match result {
        Ok(sidebar) => Box::into_raw(Box::new(Andamento {
            sidebar,
            poisoned: false,
        })),
        Err(error) => {
            if !error_out.is_null() {
                *error_out = string(error);
            }
            ptr::null_mut()
        }
    }
}

/// Returns owned JSON: {"ok":true,"snapshot":...,"effects":[...]} or
/// {"ok":false,"error":"..."}. A panic poisons the handle; destroy and recreate it.
/// # Safety
/// handle is null or a live handle from andamento_create, with no concurrent call.
/// data points to len readable bytes; it need not be NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn andamento_request(
    handle: *mut Andamento,
    data: *const u8,
    len: usize,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<serde_json::Value, String> {
        let handle = handle.as_mut().ok_or("null sidebar handle")?;
        if handle.poisoned {
            return Err("sidebar handle is poisoned; recreate it".into());
        }
        let request: Request =
            serde_json::from_str(input(data, len)?).map_err(|e| e.to_string())?;
        let response = handle.sidebar.handle(request)?;
        Ok(serde_json::json!({"ok":true,"snapshot":response.snapshot,"effects":response.effects}))
    }));
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => serde_json::json!({"ok":false,"error":error}),
        Err(_) => {
            if let Some(handle) = handle.as_mut() {
                handle.poisoned = true;
            }
            serde_json::json!({"ok":false,"error":"core panicked; recreate sidebar"})
        }
    };
    string(value.to_string())
}

/// # Safety
/// handle is null or a live handle from andamento_create, freed exactly once
/// after all calls using it have finished.
#[no_mangle]
pub unsafe extern "C" fn andamento_destroy(handle: *mut Andamento) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// # Safety
/// value is null or a string returned by this ABI, freed exactly once.
#[no_mangle]
pub unsafe extern "C" fn andamento_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn owned_json_roundtrip_and_errors_do_not_destroy_the_handle() {
        unsafe {
            let config = include_str!("../../andamento-core/tests/fixtures/sidebar.kdl");
            let mut error = ptr::null_mut();
            let handle = andamento_create(config.as_ptr(), config.len(), &mut error);
            assert!(!handle.is_null());
            assert!(error.is_null());
            for (request, ok) in [("not json", false), (r#"{"request":"snapshot"}"#, true)] {
                let result = andamento_request(handle, request.as_ptr(), request.len());
                let value: serde_json::Value =
                    serde_json::from_str(CStr::from_ptr(result).to_str().unwrap()).unwrap();
                assert_eq!(value["ok"], ok);
                andamento_string_free(result);
            }
            andamento_destroy(handle);
        }
    }

    #[test]
    fn bad_utf8_and_null_arguments_return_errors() {
        unsafe {
            let mut error = ptr::null_mut();
            assert!(andamento_create([255].as_ptr(), 1, &mut error).is_null());
            assert!(!error.is_null());
            andamento_string_free(error);
            let response = andamento_request(ptr::null_mut(), ptr::null(), 1);
            assert!(CStr::from_ptr(response)
                .to_str()
                .unwrap()
                .contains("null sidebar handle"));
            andamento_string_free(response);
            andamento_destroy(ptr::null_mut());
            andamento_string_free(ptr::null_mut());
        }
    }
}
