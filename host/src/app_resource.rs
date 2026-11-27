use crate::Result;
use std::ptr::null;
use windows_sys::Win32::System::LibraryLoader::{
    FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
};

// These descriptors live in the current executable for the process lifetime.
pub(crate) fn embedded_app_data(id: u16, limit: usize) -> Result<Option<&'static [u8]>> {
    use windows_sys::Win32::Foundation::{
        ERROR_RESOURCE_DATA_NOT_FOUND, ERROR_RESOURCE_NAME_NOT_FOUND,
        ERROR_RESOURCE_TYPE_NOT_FOUND, GetLastError,
    };

    const RT_RCDATA: *const u16 = 10usize as *const u16;
    // SAFETY: Only the main executable is used, and its resource mapping remains loaded.
    unsafe {
        let instance = GetModuleHandleW(null());
        let resource = FindResourceW(instance, usize::from(id) as *const u16, RT_RCDATA);
        if resource.is_null() {
            return match GetLastError() {
                ERROR_RESOURCE_DATA_NOT_FOUND
                | ERROR_RESOURCE_NAME_NOT_FOUND
                | ERROR_RESOURCE_TYPE_NOT_FOUND => Ok(None),
                _ => Err(format!(
                    "locate embedded app metadata: {}",
                    std::io::Error::last_os_error()
                )),
            };
        }
        let len = SizeofResource(instance, resource) as usize;
        if len == 0 || len > limit {
            return Err("embedded app metadata has an invalid length".into());
        }
        let loaded = LoadResource(instance, resource);
        if loaded.is_null() {
            return Err(format!(
                "load embedded app metadata: {}",
                std::io::Error::last_os_error()
            ));
        }
        let data = LockResource(loaded);
        if data.is_null() {
            return Err("could not read embedded app metadata".into());
        }
        Ok(Some(std::slice::from_raw_parts(data.cast::<u8>(), len)))
    }
}
