//! Nullable COM outputs for UI Automation interfaces.
//!
//! The typed windows-rs provider traits use non-null interfaces on success.
//! UI Automation also requires success with a null output for an absent
//! pattern, relative, host, or focused element.

#![allow(non_snake_case)] // Keep the SDK vtable method names beside their adapters.

use std::ffi::c_void;
use std::ptr::null_mut;
use windows::Win32::Foundation::{E_NOINTERFACE, E_POINTER, HWND, S_OK};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::core::{HRESULT, IUnknown, IUnknown_Vtbl, Interface, Result, interface};

fn write<T>(out: *mut T, answer: impl FnOnce() -> Result<T>) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    match answer() {
        Ok(value) => {
            // SAFETY: A nonnull COM output belongs to the caller.
            unsafe { out.write(value) };
            S_OK
        }
        Err(error) => error.code(),
    }
}

fn optional<T: Interface>(out: *mut *mut c_void, answer: impl FnOnce() -> Result<T>) -> HRESULT {
    write(out, || match answer() {
        Ok(value) => Ok(value.into_raw()),
        Err(error) if error.code() == E_NOINTERFACE => Ok(null_mut()),
        Err(error) => Err(error),
    })
}

#[interface("d6dd68d1-86fd-4332-8666-9abedea2d24c")]
pub(crate) unsafe trait SimpleAbi: IUnknown {
    fn ProviderOptions(&self, out: *mut ProviderOptions) -> HRESULT;
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID, out: *mut *mut c_void) -> HRESULT;
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID, out: *mut VARIANT) -> HRESULT;
    fn HostRawElementProvider(&self, out: *mut *mut c_void) -> HRESULT;
}

impl<T: IRawElementProviderSimple_Impl> SimpleAbi_Impl for T {
    unsafe fn ProviderOptions(&self, out: *mut ProviderOptions) -> HRESULT {
        write(out, || {
            IRawElementProviderSimple_Impl::ProviderOptions(self)
        })
    }
    unsafe fn GetPatternProvider(&self, id: UIA_PATTERN_ID, out: *mut *mut c_void) -> HRESULT {
        optional(out, || {
            IRawElementProviderSimple_Impl::GetPatternProvider(self, id)
        })
    }
    unsafe fn GetPropertyValue(&self, id: UIA_PROPERTY_ID, out: *mut VARIANT) -> HRESULT {
        write(out, || {
            IRawElementProviderSimple_Impl::GetPropertyValue(self, id)
        })
    }
    unsafe fn HostRawElementProvider(&self, out: *mut *mut c_void) -> HRESULT {
        optional(out, || {
            IRawElementProviderSimple_Impl::HostRawElementProvider(self)
        })
    }
}

impl SimpleAbi {
    pub(crate) fn into_provider(self) -> IRawElementProviderSimple {
        // SAFETY: Both definitions use the same IID and vtable layout. Transfer the owned reference.
        unsafe { IRawElementProviderSimple::from_raw(self.into_raw()) }
    }
}

#[interface("f7063da8-8359-439c-9297-bbc5299a7d87")]
pub(crate) unsafe trait FragmentAbi: IUnknown {
    fn Navigate(&self, direction: NavigateDirection, out: *mut *mut c_void) -> HRESULT;
    fn GetRuntimeId(&self, out: *mut *mut SAFEARRAY) -> HRESULT;
    fn BoundingRectangle(&self, out: *mut UiaRect) -> HRESULT;
    fn GetEmbeddedFragmentRoots(&self, out: *mut *mut SAFEARRAY) -> HRESULT;
    fn SetFocus(&self) -> HRESULT;
    fn FragmentRoot(&self, out: *mut *mut c_void) -> HRESULT;
}

impl<T: IRawElementProviderFragment_Impl> FragmentAbi_Impl for T {
    unsafe fn Navigate(&self, direction: NavigateDirection, out: *mut *mut c_void) -> HRESULT {
        optional(out, || {
            IRawElementProviderFragment_Impl::Navigate(self, direction)
        })
    }
    unsafe fn GetRuntimeId(&self, out: *mut *mut SAFEARRAY) -> HRESULT {
        write(out, || IRawElementProviderFragment_Impl::GetRuntimeId(self))
    }
    unsafe fn BoundingRectangle(&self, out: *mut UiaRect) -> HRESULT {
        write(out, || {
            IRawElementProviderFragment_Impl::BoundingRectangle(self)
        })
    }
    unsafe fn GetEmbeddedFragmentRoots(&self, out: *mut *mut SAFEARRAY) -> HRESULT {
        write(out, || {
            IRawElementProviderFragment_Impl::GetEmbeddedFragmentRoots(self)
        })
    }
    unsafe fn SetFocus(&self) -> HRESULT {
        IRawElementProviderFragment_Impl::SetFocus(self)
            .map_or_else(|error| error.code(), |()| S_OK)
    }
    unsafe fn FragmentRoot(&self, out: *mut *mut c_void) -> HRESULT {
        optional(out, || IRawElementProviderFragment_Impl::FragmentRoot(self))
    }
}

#[interface("620ce2a5-ab8f-40a9-86cb-de3c75599b58")]
pub(crate) unsafe trait FragmentRootAbi: IUnknown {
    fn ElementProviderFromPoint(&self, x: f64, y: f64, out: *mut *mut c_void) -> HRESULT;
    fn GetFocus(&self, out: *mut *mut c_void) -> HRESULT;
}

impl<T: IRawElementProviderFragmentRoot_Impl> FragmentRootAbi_Impl for T {
    unsafe fn ElementProviderFromPoint(&self, x: f64, y: f64, out: *mut *mut c_void) -> HRESULT {
        optional(out, || {
            IRawElementProviderFragmentRoot_Impl::ElementProviderFromPoint(self, x, y)
        })
    }
    unsafe fn GetFocus(&self, out: *mut *mut c_void) -> HRESULT {
        optional(out, || IRawElementProviderFragmentRoot_Impl::GetFocus(self))
    }
}

#[interface("1d5df27c-8947-4425-b8d9-79787bb460b8")]
pub(crate) unsafe trait HwndOverrideAbi: IUnknown {
    fn GetOverrideProviderForHwnd(&self, hwnd: HWND, out: *mut *mut c_void) -> HRESULT;
}

impl<T: IRawElementProviderHwndOverride_Impl> HwndOverrideAbi_Impl for T {
    unsafe fn GetOverrideProviderForHwnd(&self, hwnd: HWND, out: *mut *mut c_void) -> HRESULT {
        optional(out, || {
            IRawElementProviderHwndOverride_Impl::GetOverrideProviderForHwnd(self, hwnd)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_optional_interface_is_success_with_null() {
        let mut value = std::ptr::dangling_mut();
        let result = optional::<IUnknown>(&mut value, || {
            Err(windows::core::Error::from_hresult(E_NOINTERFACE))
        });
        assert_eq!(result, S_OK);
        assert!(value.is_null());
        assert_eq!(
            optional::<IUnknown>(null_mut(), || panic!("null output should be rejected")),
            E_POINTER
        );
    }
}
