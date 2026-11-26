//! The host boundary for platform/main.roc. The checked Roc layouts are
//! covered by size and field-offset assertions below.

use crate::roc_platform_abi::{
    self as glue, AskForHost as Ask, CoreKept, InitForHost as Step, RocListWith, roc_engine_drop,
    roc_engine_event, roc_engine_files, roc_engine_init, roc_engine_keys, roc_engine_version,
};
use crate::{Event, Result, effects};
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::ffi::c_void;
use std::mem::{align_of, offset_of, size_of};
use std::ptr::{copy_nonoverlapping, null_mut};
use std::slice;
use std::sync::OnceLock;

const WORD: usize = size_of::<usize>();

pub(crate) type RocBox = *mut c_void;

pub(crate) type RocList = RocListWith<u8, false>;

impl RocListWith<u8, false> {
    pub(crate) const EMPTY: Self = Self {
        elements: null_mut(),
        length: 0,
        capacity_or_alloc_ptr: 0,
    };

    pub(crate) fn owned_bytes(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return Self::EMPTY;
        }
        // SAFETY: Allocated space covers the refcount plus the entire list.
        unsafe {
            let base = roc_alloc(WORD + bytes.len(), WORD);
            base.cast::<usize>().write(1);
            let elements = base.cast::<u8>().add(WORD);
            copy_nonoverlapping(bytes.as_ptr(), elements, bytes.len());
            Self {
                elements,
                length: bytes.len(),
                capacity_or_alloc_ptr: bytes.len() << 1,
            }
        }
    }

    pub(crate) fn owned_handles(handles: &[u64]) -> Self {
        if handles.is_empty() {
            return Self::EMPTY;
        }
        // SAFETY: Box(U64) lists store the element count and an initial
        // refcount before the aligned array of owned box pointers.
        unsafe {
            let base = roc_alloc((2 + handles.len()) * WORD, WORD).cast::<usize>();
            base.write(handles.len());
            base.add(1).write(1);
            let elements = base.add(2).cast::<RocBox>();
            for (index, token) in handles.iter().copied().enumerate() {
                elements.add(index).write(box_handle(token));
            }
            Self {
                elements: elements.cast(),
                length: handles.len(),
                capacity_or_alloc_ptr: handles.len() << 1,
            }
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        if self.length == 0 {
            return &[];
        }
        // SAFETY: This is either a host-allocated list or an owned Roc
        // result. Its logical (possibly sliced) data is live until dropped.
        unsafe { slice::from_raw_parts(self.elements, self.length) }
    }

    /// The caller owns one reference to a live Roc List(U8).
    pub(crate) unsafe fn release_bytes(self) {
        let elements = if self.capacity_or_alloc_ptr & 1 == 0 {
            self.elements
        } else {
            std::ptr::with_exposed_provenance_mut(self.capacity_or_alloc_ptr & !1)
        };
        if elements.is_null() {
            return;
        }
        // SAFETY: For a byte list the allocation starts at the refcount.
        // Only its Roc instance owns references on this thread.
        unsafe {
            let count = elements.cast::<isize>().sub(1);
            match count.read() {
                n if n <= 0 => {} // Static data or Roc frame storage.
                1 => roc_dealloc(count.cast(), WORD),
                n => count.write(n - 1),
            }
        }
    }
}

pub(crate) fn box_handle(token: u64) -> RocBox {
    // SAFETY: Box(U64) has a one-word refcount followed by its aligned
    // U64 payload. Register the allocation address before giving it to Roc.
    unsafe {
        let base = roc_alloc(WORD + size_of::<u64>(), align_of::<u64>());
        base.cast::<usize>().write(1);
        let payload = base.cast::<u8>().add(WORD).cast::<u64>();
        payload.write(token);
        effects::register_handle_box(base, token);
        payload.cast()
    }
}

const _: () = {
    assert!(size_of::<RocList>() == 24);
    assert!(offset_of!(RocList, length) == 8);
    assert!(offset_of!(RocList, capacity_or_alloc_ptr) == 16);
    assert!(size_of::<Step>() == 56);
    assert!(offset_of!(Step, state) == 24);
    assert!(offset_of!(Step, work) == 32);
    assert!(size_of::<Ask>() == 32);
    assert!(offset_of!(Ask, out) == 8);
};

fn handles(list: RocList) -> glue::RocList<*mut u64> {
    glue::RocList {
        elements: list.elements.cast(),
        length: list.length,
        capacity_or_alloc_ptr: list.capacity_or_alloc_ptr,
    }
}

pub(crate) unsafe fn roc_engine_work(task: RocList, files: RocList) -> RocList {
    // SAFETY: The worker owns these Roc lists and gives them to this turn.
    unsafe { glue::roc_engine_work(task, handles(files)) }
}

pub(crate) unsafe fn roc_engine_prepare(task: RocList, files: RocList) -> *mut CoreKept {
    // SAFETY: The worker owns these Roc lists and gives them to this turn.
    unsafe { glue::roc_engine_prepare(task, handles(files)) }
}

pub(crate) unsafe fn roc_engine_ask(held: *mut CoreKept, task: RocList, files: RocList) -> Ask {
    // SAFETY: All input values belong to the worker instance on this thread.
    unsafe { glue::roc_engine_ask(held, task, handles(files)) }
}

pub(crate) use glue::roc_engine_forget;

/// Owns the page's opaque Roc state and the two recyclable list allocations.
/// Roc takes these allocations by value and returns their replacements.
#[derive(Default)]
pub(crate) struct Engine(Option<Step>);

impl Engine {
    pub(crate) fn declarations() -> Result<Vec<u8>> {
        // SAFETY: Both provided functions need no state. This is called by
        // the main thread before mount, and we release the returned list.
        unsafe {
            let version = roc_engine_version();
            if version != crate::wire::VERSION {
                return Err(format!(
                    "Roc wire version {version}, expected {}",
                    crate::wire::VERSION
                ));
            }
            let list = roc_engine_keys();
            let bytes = list.bytes().to_vec();
            list.release_bytes();
            Ok(bytes)
        }
    }

    pub(crate) fn mount(&mut self, boot: &[u8]) -> Vec<u8> {
        assert!(self.0.is_none());
        // SAFETY: The boot payload is an owned Roc allocation, transferred
        // to Roc. The UI thread calls the engine once per turn.
        let step = unsafe { roc_engine_init(RocList::owned_bytes(boot)) };
        self.store(step)
    }

    pub(crate) fn event(&mut self, event: Event) -> Result<Vec<u8>> {
        let Step { state, cmds, work } = self.0.take().ok_or("Roc event before mount")?;
        // SAFETY: The page's last turn transferred all three allocations to
        // us. They move into Roc here and are never used afterwards.
        let step = unsafe {
            match event {
                Event::Turn {
                    node,
                    handler,
                    payload,
                } => roc_engine_event(
                    state,
                    cmds,
                    work,
                    node,
                    handler,
                    RocList::owned_bytes(&payload),
                ),
                Event::Files {
                    node,
                    handler,
                    record,
                    handles: tokens,
                } => roc_engine_files(
                    state,
                    cmds,
                    work,
                    node,
                    handler,
                    RocList::owned_bytes(&record),
                    handles(RocList::owned_handles(&tokens)),
                ),
            }
        };
        Ok(self.store(step))
    }

    fn store(&mut self, step: Step) -> Vec<u8> {
        let batch = step.cmds.bytes().to_vec();
        self.0 = Some(step);
        batch
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(Step { state, cmds, work }) = self.0.take() {
            // SAFETY: Teardown consumes our still-owned state and buffers.
            unsafe { roc_engine_drop(state, cmds, work) };
        }
    }
}

#[repr(C)]
struct Allocation {
    size: usize,
    alignment: usize,
}

fn allocation_layout(size: usize, alignment: usize) -> (Layout, usize) {
    let alignment = alignment.max(16);
    let header = (size_of::<Allocation>() + alignment - 1) & !(alignment - 1);
    let size = size
        .max(1)
        .checked_add(header)
        .expect("Roc allocation too large");
    (
        Layout::from_size_align(size, alignment).expect("invalid Roc alignment or size"),
        header,
    )
}

#[unsafe(no_mangle)]
extern "C" fn roc_alloc(size: usize, alignment: usize) -> *mut c_void {
    let (layout, header) = allocation_layout(size, alignment);
    // SAFETY: The extra header bytes keep the returned address aligned,
    // while giving us the size needed to free it through Rust's allocator.
    unsafe {
        let base = alloc(layout);
        if base.is_null() {
            handle_alloc_error(layout);
        }
        let result = base.add(header);
        result
            .cast::<Allocation>()
            .sub(1)
            .write(Allocation { size, alignment });
        result.cast()
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn roc_dealloc(ptr: *mut c_void, _alignment: usize) {
    if ptr.is_null() {
        return;
    }
    effects::released_box(ptr);
    // SAFETY: Roc passes a previously allocated address at refcount zero.
    unsafe {
        let Allocation { size, alignment } = ptr.cast::<Allocation>().sub(1).read();
        let (layout, header) = allocation_layout(size, alignment);
        dealloc(ptr.cast::<u8>().sub(header), layout);
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn roc_realloc(ptr: *mut c_void, size: usize, alignment: usize) -> *mut c_void {
    let next = roc_alloc(size, alignment);
    if !ptr.is_null() {
        // SAFETY: The old and new allocations are distinct, live, and the
        // copy is bounded by both lengths. The old allocation is consumed.
        unsafe {
            let previous = &*ptr.cast::<Allocation>().sub(1);
            copy_nonoverlapping(ptr.cast::<u8>(), next.cast(), previous.size.min(size));
            roc_dealloc(ptr, previous.alignment);
        }
    }
    next
}

fn message(ptr: *const u8, length: usize) -> String {
    let bytes = if length == 0 || ptr.is_null() {
        &[]
    } else {
        // SAFETY: The Roc runtime passes a buffer live for this call.
        unsafe { slice::from_raw_parts(ptr, length) }
    };
    String::from_utf8_lossy(bytes).into_owned()
}

#[unsafe(no_mangle)]
extern "C" fn roc_dbg(ptr: *const u8, length: usize) {
    eprintln!("[dbg] {}", message(ptr, length));
}

#[unsafe(no_mangle)]
extern "C" fn roc_expect_failed(ptr: *const u8, length: usize) {
    eprintln!("[expect failed] {}", message(ptr, length));
}

static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

pub(crate) fn register_main_thread() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

#[unsafe(no_mangle)]
extern "C" fn roc_crashed(ptr: *const u8, length: usize) -> ! {
    let error = format!("Roc crashed: {}", message(ptr, length));
    if MAIN_THREAD.get() == Some(&std::thread::current().id()) {
        crate::window::report_error(&error);
    } else {
        // A worker cannot unwind Roc frames through Rust.
        eprintln!("roc-native-windows: {error}");
    }
    std::process::exit(1);
}
