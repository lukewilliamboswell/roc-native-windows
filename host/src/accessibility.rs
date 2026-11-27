//! Accessibility state follows committed view state and never calls into Roc from COM.

#![allow(non_upper_case_globals)] // UI Automation identifiers follow the Windows SDK spelling.

use crate::Result;
use crate::accessibility_abi::SimpleAbi;
use crate::view::wide;
use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{
    E_NOINTERFACE, E_OUTOFMEMORY, HWND, LPARAM, VARIANT_BOOL, WPARAM,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Accessibility::*;
use windows::core::{BSTR, Error, HRESULT, IUnknown, Interface, PCWSTR, implement};
use windows_sys::Win32::Foundation::{
    HWND as RawHwnd, LPARAM as RawLparam, LRESULT, WPARAM as RawWparam,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, ISMEX_NOSEND, InSendMessageEx, IsWindowVisible, OBJID_CLIENT,
    WM_GETOBJECT, WM_NCDESTROY,
};

thread_local! {
    static RETIRING: RefCell<Vec<IRawElementProviderSimple>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn disconnect_retired() {
    // SAFETY: Only the owning UI thread calls this, and UIA forbids disconnect
    // during an inter-thread synchronous message.
    unsafe {
        if InSendMessageEx(std::ptr::null()) != ISMEX_NOSEND {
            return;
        }
        let pending = RETIRING.with(|queue| std::mem::take(&mut *queue.borrow_mut()));
        for provider in pending {
            let _ = UiaDisconnectProvider(&provider);
        }
    }
}

pub(crate) struct Apartment;

impl Apartment {
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: Paired with CoUninitialize on the UI thread.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|error| format!("initialize Windows accessibility: {error}"))?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: This guard lives on the thread that initialized the apartment.
        unsafe { CoUninitialize() };
    }
}

pub(crate) struct NativeName {
    hwnd: HWND,
    service: IAccPropServices,
    help: String,
    related: bool,
}

/// The native EDIT proxy does not consistently expose dynamically annotated
/// HelpText. Supply that property while leaving its native patterns available.
pub(crate) struct NativeHelp {
    text: Arc<Mutex<Option<String>>>,
    mount: Mount,
    announced: String,
}

impl NativeHelp {
    pub(crate) fn new(hwnd: RawHwnd) -> Result<Self> {
        let text = Arc::new(Mutex::new(Some(String::new())));
        let provider: SimpleAbi = HelpProvider {
            hwnd: hwnd as usize,
            text: Arc::clone(&text),
        }
        .into();
        Ok(Self {
            text,
            mount: Mount::new(hwnd, provider.into_provider())?,
            announced: String::new(),
        })
    }

    pub(crate) fn update(&self, details: &str) {
        *self.text.lock().unwrap() = Some(details.to_owned());
    }

    pub(crate) fn announce(&mut self) {
        if !self.mount.events_allowed() {
            return;
        }
        let details = self.text.lock().unwrap().clone().unwrap_or_default();
        if self.announced != details {
            text_changed(
                self.mount.provider(),
                UIA_HelpTextPropertyId,
                &self.announced,
                &details,
            );
            self.announced = details;
        }
    }
}

impl Drop for NativeHelp {
    fn drop(&mut self) {
        *self.text.lock().unwrap() = None;
    }
}

#[implement(SimpleAbi)]
struct HelpProvider {
    hwnd: usize,
    text: Arc<Mutex<Option<String>>>,
}

impl IRawElementProviderSimple_Impl for HelpProvider_Impl {
    fn ProviderOptions(&self) -> windows::core::Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_OverrideProvider)
    }

    fn GetPatternProvider(&self, _: UIA_PATTERN_ID) -> windows::core::Result<IUnknown> {
        Err(Error::from_hresult(E_NOINTERFACE))
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> windows::core::Result<VARIANT> {
        let text = self.text.lock().unwrap();
        let text = text.as_deref().ok_or_else(unavailable)?;
        Ok(match id {
            UIA_HelpTextPropertyId => string(text),
            _ => VARIANT::default(),
        })
    }

    fn HostRawElementProvider(&self) -> windows::core::Result<IRawElementProviderSimple> {
        if self.text.lock().unwrap().is_none() {
            return Err(unavailable());
        }
        // SAFETY: The provider is mounted on this EDIT HWND. UIA validates it.
        unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as _)) }
    }
}

impl NativeName {
    pub(crate) fn annotate(hwnd: RawHwnd, name: &str, current: &mut Option<Self>) {
        if name.trim().is_empty() {
            *current = None;
            return;
        }
        // SAFETY: This is a live UI-thread HWND and COM is initialized there.
        unsafe {
            if current.is_none()
                && let Ok(service) =
                    CoCreateInstance(&CLSID_AccPropServices, None, CLSCTX_INPROC_SERVER)
            {
                *current = Some(Self {
                    hwnd: HWND(hwnd),
                    service,
                    help: String::new(),
                    related: false,
                });
            }
            if let Some(current) = current {
                let name = wide(name);
                for property in [PROPID_ACC_NAME, Name_Property_GUID] {
                    let _ = current.service.SetHwndPropStr(
                        current.hwnd,
                        OBJID_CLIENT as u32,
                        0,
                        property,
                        PCWSTR(name.as_ptr()),
                    );
                }
            }
        }
    }

    pub(crate) fn relate(&mut self, label: RawHwnd, description: Option<RawHwnd>) {
        if self.related || label.is_null() {
            return;
        }
        self.related = true;
        // These sibling HWNDs are owned by the same mounted control.
        self.set_relation(LabeledBy_Property_GUID, label, false);
        if let Some(description) = description {
            self.set_relation(DescribedBy_Property_GUID, description, true);
        }
    }

    fn set_relation(&self, property: windows::core::GUID, target: RawHwnd, list: bool) {
        if let Ok(mut value) = related_element(target, list) {
            // SAFETY: The target and owner are live on this UI thread. The
            // annotation copies the variant before the call returns.
            unsafe {
                let _ =
                    self.service
                        .SetHwndProp(self.hwnd, OBJID_CLIENT as u32, 0, property, &value);
                let _ = VariantClear(&mut value);
            }
        }
    }

    pub(crate) fn help(&mut self, details: &str) {
        if self.help == details {
            return;
        }
        self.help = details.to_owned();
        let details = wide(details);
        // SAFETY: The same live annotation owns both platform help properties.
        unsafe {
            for property in [PROPID_ACC_HELP, HelpText_Property_GUID] {
                let _ = self.service.SetHwndPropStr(
                    self.hwnd,
                    OBJID_CLIENT as u32,
                    0,
                    property,
                    PCWSTR(details.as_ptr()),
                );
            }
        }
    }
}

impl Drop for NativeName {
    fn drop(&mut self) {
        // SAFETY: Clearing the native annotation is safe before window teardown.
        unsafe {
            let _ = self.service.ClearHwndProps(
                self.hwnd,
                OBJID_CLIENT as u32,
                0,
                &[
                    PROPID_ACC_NAME,
                    Name_Property_GUID,
                    PROPID_ACC_HELP,
                    HelpText_Property_GUID,
                    LabeledBy_Property_GUID,
                    DescribedBy_Property_GUID,
                ],
            );
        }
    }
}

fn related_element(hwnd: RawHwnd, list: bool) -> windows::core::Result<VARIANT> {
    // SAFETY: The target is an owned, mounted STATIC. Its native provider stays
    // accessible through a COM reference while the property is annotated.
    let provider = unsafe { UiaHostProviderFromHwnd(HWND(hwnd)) }?.cast::<IUnknown>()?;
    if !list {
        return Ok(variant(
            VT_UNKNOWN,
            VARIANT_0_0_0 {
                punkVal: ManuallyDrop::new(Some(provider)),
            },
        ));
    }
    // SAFETY: This one-element SAFEARRAY owns a reference to the provider.
    unsafe {
        let array = SafeArrayCreateVector(VT_UNKNOWN, 0, 1);
        if array.is_null() {
            return Err(Error::from_hresult(E_OUTOFMEMORY));
        }
        if let Err(error) = SafeArrayPutElement(array, &0, provider.as_raw().cast_const()) {
            let _ = SafeArrayDestroy(array);
            return Err(error);
        }
        Ok(variant(
            VARENUM(VT_ARRAY.0 | VT_UNKNOWN.0),
            VARIANT_0_0_0 { parray: array },
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Graphic { included: bool },
    Status { polite: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Semantics {
    pub(crate) name: String,
    pub(crate) details: String,
    pub(crate) role: Role,
}

#[implement(SimpleAbi)]
struct Element {
    hwnd: usize,
    semantics: Arc<Mutex<Option<Semantics>>>,
}

impl IRawElementProviderSimple_Impl for Element_Impl {
    fn ProviderOptions(&self) -> windows::core::Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, _: UIA_PATTERN_ID) -> windows::core::Result<IUnknown> {
        Err(Error::from_hresult(E_NOINTERFACE))
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> windows::core::Result<VARIANT> {
        let state = self.semantics.lock().unwrap();
        let state = state.as_ref().ok_or_else(unavailable)?;
        Ok(match (property, &state.role) {
            (UIA_NamePropertyId, _) => string(&state.name),
            (UIA_ControlTypePropertyId, Role::Graphic { .. }) => integer(UIA_ImageControlTypeId.0),
            (UIA_ControlTypePropertyId, Role::Status { .. }) => integer(UIA_TextControlTypeId.0),
            (
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId,
                Role::Graphic { included },
            ) => boolean(*included),
            (
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId,
                Role::Status { .. },
            ) => boolean(true),
            (UIA_FullDescriptionPropertyId, _) if !state.details.is_empty() => {
                string(&state.details)
            }
            (UIA_LiveSettingPropertyId, Role::Status { polite }) => {
                integer(if *polite { Polite.0 } else { Off.0 })
            }
            _ => VARIANT::default(),
        })
    }

    fn HostRawElementProvider(&self) -> windows::core::Result<IRawElementProviderSimple> {
        if self.semantics.lock().unwrap().is_none() {
            return Err(unavailable());
        }
        // SAFETY: UIA resolves the native provider from the live HWND. It can
        // return an error if the surface was retired during this call.
        unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as _)) }
    }
}

pub(crate) fn unavailable() -> Error {
    Error::from_hresult(HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32))
}

pub(crate) fn integer(value: i32) -> VARIANT {
    variant(VT_I4, VARIANT_0_0_0 { lVal: value })
}

pub(crate) fn boolean(value: bool) -> VARIANT {
    variant(
        VT_BOOL,
        VARIANT_0_0_0 {
            boolVal: VARIANT_BOOL(if value { -1 } else { 0 }),
        },
    )
}

pub(crate) fn string(value: &str) -> VARIANT {
    variant(
        VT_BSTR,
        VARIANT_0_0_0 {
            bstrVal: ManuallyDrop::new(BSTR::from(value)),
        },
    )
}

pub(crate) fn variant(vt: VARENUM, value: VARIANT_0_0_0) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt,
                Anonymous: value,
                ..Default::default()
            }),
        },
    }
}

pub(crate) fn text_changed(
    provider: &IRawElementProviderSimple,
    property: UIA_PROPERTY_ID,
    old: &str,
    new: &str,
) {
    let mut old = string(old);
    let mut new = string(new);
    // SAFETY: The variants are initialized as BSTRs and are cleared after the synchronous call.
    unsafe {
        let _ = UiaRaiseAutomationPropertyChangedEvent(provider, property, &old, &new);
        let _ = VariantClear(&mut old);
        let _ = VariantClear(&mut new);
    }
}

pub(crate) struct ElementProvider {
    mount: Mount,
    semantics: Arc<Mutex<Option<Semantics>>>,
    announced: Semantics,
}

impl ElementProvider {
    pub(crate) fn new(hwnd: RawHwnd, initial: Semantics) -> Result<Self> {
        let semantics = Arc::new(Mutex::new(Some(initial.clone())));
        let provider: SimpleAbi = Element {
            hwnd: hwnd as usize,
            semantics: semantics.clone(),
        }
        .into();
        Ok(Self {
            mount: Mount::new(hwnd, provider.into_provider())?,
            semantics,
            announced: initial,
        })
    }

    pub(crate) fn update(&self, next: Semantics) {
        *self.semantics.lock().unwrap() = Some(next);
    }

    pub(crate) fn announce(&mut self) {
        let current = self.semantics.lock().unwrap().as_ref().unwrap().clone();
        if current == self.announced {
            return;
        }
        let old = std::mem::replace(&mut self.announced, current);
        let new = &self.announced;
        if !self.mount.events_allowed() {
            return;
        }
        if let Role::Status { polite: true } = new.role {
            if !new.name.trim().is_empty() && new.name != old.name {
                // SAFETY: The provider is retained for this synchronous event call.
                unsafe {
                    let _ = UiaRaiseAutomationEvent(
                        self.mount.provider(),
                        UIA_LiveRegionChangedEventId,
                    );
                }
            }
        } else if matches!(new.role, Role::Graphic { .. }) {
            if old.name != new.name {
                text_changed(
                    self.mount.provider(),
                    UIA_NamePropertyId,
                    &old.name,
                    &new.name,
                );
            }
            if old.details != new.details {
                text_changed(
                    self.mount.provider(),
                    UIA_FullDescriptionPropertyId,
                    &old.details,
                    &new.details,
                );
            }
            if old.role != new.role {
                // SAFETY: The semantic node membership changed on this HWND.
                unsafe {
                    let _ = UiaRaiseStructureChangedEvent(
                        self.mount.provider(),
                        StructureChangeType_ChildrenInvalidated,
                        std::ptr::null_mut(),
                        0,
                    );
                }
            }
        }
    }
}

impl Drop for ElementProvider {
    fn drop(&mut self) {
        *self.semantics.lock().unwrap() = None;
    }
}

pub(crate) struct Mount {
    hwnd: RawHwnd,
    provider: Box<IRawElementProviderSimple>,
}

impl Mount {
    pub(crate) fn new(hwnd: RawHwnd, provider: IRawElementProviderSimple) -> Result<Self> {
        let provider = Box::new(provider);
        let pointer = std::ptr::from_ref(provider.as_ref()) as usize;
        // SAFETY: The Box outlives the subclass and the owning View stays on the UI thread.
        if unsafe { SetWindowSubclass(hwnd, Some(serve), 1, pointer) } == 0 {
            return Err("could not attach Windows accessibility provider".into());
        }
        Ok(Self { hwnd, provider })
    }

    pub(crate) fn provider(&self) -> &IRawElementProviderSimple {
        &self.provider
    }

    pub(crate) fn events_allowed(&self) -> bool {
        // SAFETY: The mounted window and its owner are queried on the UI thread.
        unsafe {
            UiaClientsAreListening().as_bool()
                && IsWindowVisible(self.hwnd) != 0
                && IsWindowEnabled(GetAncestor(self.hwnd, GA_ROOT)) != 0
        }
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        // SAFETY: Remove the subclass before releasing its owned data pointer.
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(serve), 1);
            clear_event_map(self.hwnd);
        }
        RETIRING.with(|queue| queue.borrow_mut().push(self.provider().clone()));
    }
}

unsafe fn clear_event_map(hwnd: RawHwnd) {
    // SAFETY: This call releases UIA's event associations with the retiring HWND.
    unsafe {
        UiaReturnRawElementProvider(
            HWND(hwnd),
            WPARAM(0),
            LPARAM(0),
            None::<&IRawElementProviderSimple>,
        )
    };
}

unsafe extern "system" fn serve(
    hwnd: RawHwnd,
    message: u32,
    wp: RawWparam,
    lp: RawLparam,
    id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: The subclass data is owned by Mount on this UI thread.
    unsafe {
        if message == WM_GETOBJECT && lp as i32 == UiaRootObjectId {
            return UiaReturnRawElementProvider(
                HWND(hwnd),
                WPARAM(wp),
                LPARAM(lp),
                &*(data as *const IRawElementProviderSimple),
            )
            .0;
        }
        if message == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(serve), id);
            clear_event_map(hwnd);
        }
        DefSubclassProc(hwnd, message, wp, lp)
    }
}
