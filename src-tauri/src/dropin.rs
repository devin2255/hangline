//! Drag-in drop target (v1.5.0) — **feasibility probe stage**.
//!
//! This module registers a hand-written OLE `IDropTarget` on the panel window and every WebView2
//! child HWND, replacing the handler wry installs (the panel builder calls
//! `disable_drag_drop_handler`, so nothing competes for the registration). It logs which clipboard
//! formats a real drag carries (`CF_HDROP`, `CF_DIB`, `CF_DIBV5`, `PNG`). The goal of this stage is
//! to prove that a native drop target can see drags over the transparent panel at all — the full
//! ingestion path (decode / dedupe / save / add_item) is implemented after that proof.
//!
//! Enabled only when `HANG_DROP_SPIKE` is set; the production gate (drag polling + reveal + click
//! through toggling) lands in the next stage.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use tauri::WebviewWindow;
use windows::core::{BOOL, Interface, Type, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINTL, E_NOINTERFACE, E_POINTER, S_OK};
use windows::Win32::System::Com::{IDataObject, FORMATETC};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Vtbl, OleInitialize, RegisterDragDrop, RevokeDragDrop, CF_DIB,
    CF_DIBV5, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::WindowsAndMessaging::EnumChildWindows;

/// Environment flag that enables the feasibility probe (debug builds only; release builds ignore
/// it because `panel::create` only calls [`install`] when it is set — the production wiring comes
/// with the gate stage).
pub const SPIKE_ENV: &str = "HANG_DROP_SPIKE";

/// Common first fields of a hand-written COM object (same layout as `dragout`'s objects).
#[repr(C)]
struct ComHeader {
    vtbl: *const c_void,
    refs: AtomicU32,
}

#[repr(C)]
struct DropTargetObj {
    header: ComHeader,
}

static DROP_TARGET_VTBL: IDropTarget_Vtbl = IDropTarget_Vtbl {
    base__: windows::core::IUnknown_Vtbl {
        QueryInterface: drop_target_query_interface,
        AddRef: drop_target_add_ref,
        Release: drop_target_release,
    },
    DragEnter: drop_target_drag_enter,
    DragOver: drop_target_drag_over,
    DragLeave: drop_target_drag_leave,
    Drop: drop_target_drop,
};

/// Registers the drop target on `window` and every WebView2 child HWND.
///
/// The COM objects are intentionally kept alive for the process lifetime (`mem::forget` below):
/// OLE does not add a reference in `RegisterDragDrop`, the panel window is created once and lives
/// until exit, and leaking a few small COM objects is safer than a `Send`-violating global that
/// could release a target while OLE still uses it.
pub fn install(window: &WebviewWindow) -> Result<(), String> {
    // RegisterDragDrop requires an OLE-initialised (STA) thread. The UI thread hosts WebView2, which
    // already initialises COM; OleInitialize is reference counted, so a second call is harmless.
    if let Err(error) = unsafe { OleInitialize(None) } {
        log::warn!("dropin: OleInitialize failed; continuing (already initialised is fine): {error}");
    }
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    let mut count = 0usize;
    register(hwnd, &mut count)?;
    // SAFETY: `count` outlives the enumeration call; the callback only increments it.
    unsafe {
        let _ = EnumChildWindows(
            Some(hwnd),
            Some(register_child),
            LPARAM(&mut count as *mut usize as isize),
        );
    }
    if count == 0 {
        return Err("dropin: no HWND accepted the drop target".to_string());
    }
    log::info!(
        "dropin: registered {count} drop target(s) on the panel window {hwnd:?}",
    );
    Ok(())
}

/// `EnumChildWindows` callback: registers the drop target on one child and continues.
unsafe extern "system" fn register_child(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let count = unsafe { &mut *(lparam.0 as *mut usize) };
    if let Err(error) = register(hwnd, count) {
        log::debug!("dropin: child {hwnd:?} not registered: {error}");
    }
    BOOL(1)
}

fn register(hwnd: HWND, count: &mut usize) -> Result<(), String> {
    let object = Box::new(DropTargetObj {
        header: ComHeader {
            vtbl: &DROP_TARGET_VTBL as *const IDropTarget_Vtbl as *const c_void,
            refs: AtomicU32::new(1),
        },
    });
    let raw = Box::into_raw(object) as *mut c_void;
    let target: IDropTarget = unsafe { Type::from_abi(raw) }
        .map_err(|error| format!("dropin: cannot wrap the drop target object: {error}"))?;
    // Replace whatever was registered (WebView2's HTML5 drop target, wry's handler, ...).
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
    let class = window_class(hwnd);
    match unsafe { RegisterDragDrop(hwnd, &target) } {
        Ok(()) => {
            log::info!("dropin: registered on {hwnd:?} class={class:?}");
            // OLE keeps a borrowed pointer for the lifetime of the registration; never free it.
            std::mem::forget(target);
            *count += 1;
            Ok(())
        }
        Err(error) => {
            // Dropping `target` here releases our reference and frees the object.
            log::warn!("dropin: RegisterDragDrop failed for {hwnd:?}: {error}");
            Err(error.to_string())
        }
    }
}

// ---------------------------------------------------------------------------------------------
// COM plumbing
// ---------------------------------------------------------------------------------------------

unsafe fn add_ref(this: *mut c_void) -> u32 {
    let header = unsafe { &*(this as *const ComHeader) };
    header.refs.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe fn release(this: *mut c_void) -> u32 {
    let header = unsafe { &*(this as *const ComHeader) };
    let refs = header.refs.fetch_sub(1, Ordering::SeqCst);
    if refs == 1 {
        drop(unsafe { Box::from_raw(this as *mut DropTargetObj) });
        0
    } else {
        refs - 1
    }
}

unsafe fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let supported = [
        <windows::core::IUnknown as Interface>::IID,
        <IDropTarget as Interface>::IID,
    ];
    let requested = unsafe { &*iid };
    if supported.contains(requested) {
        unsafe {
            add_ref(this);
            *out = this;
        }
        S_OK
    } else {
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn drop_target_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe { query_interface(this, iid, out) }
}

unsafe extern "system" fn drop_target_add_ref(this: *mut c_void) -> u32 {
    unsafe { add_ref(this) }
}

unsafe extern "system" fn drop_target_release(this: *mut c_void) -> u32 {
    unsafe { release(this) }
}

unsafe extern "system" fn drop_target_drag_enter(
    _this: *mut c_void,
    data_object: *mut c_void,
    _keys: MODIFIERKEYS_FLAGS,
    _point: POINTL,
    effect: *mut DROPEFFECT,
) -> HRESULT {
    log_formats("DragEnter", data_object);
    set_effect(effect);
    S_OK
}

unsafe extern "system" fn drop_target_drag_over(
    _this: *mut c_void,
    _keys: MODIFIERKEYS_FLAGS,
    _point: POINTL,
    effect: *mut DROPEFFECT,
) -> HRESULT {
    set_effect(effect);
    S_OK
}

unsafe extern "system" fn drop_target_drag_leave(_this: *mut c_void) -> HRESULT {
    log::info!("dropin: DragLeave");
    S_OK
}

unsafe extern "system" fn drop_target_drop(
    _this: *mut c_void,
    data_object: *mut c_void,
    _keys: MODIFIERKEYS_FLAGS,
    _point: POINTL,
    effect: *mut DROPEFFECT,
) -> HRESULT {
    log::info!("dropin: Drop");
    log_formats("Drop", data_object);
    set_effect(effect);
    S_OK
}

fn set_effect(effect: *mut DROPEFFECT) {
    if !effect.is_null() {
        unsafe { *effect = DROPEFFECT_COPY };
    }
}

/// Logs which of the clipboard formats we care about the dragged data object offers.
fn log_formats(tag: &str, data_object: *mut c_void) {
    if data_object.is_null() {
        log::info!("dropin: {tag} with a null data object");
        return;
    }
    // Borrow the caller's IDataObject without taking ownership of its reference.
    let data: IDataObject = match unsafe { Type::from_abi(data_object) } {
        Ok(data) => data,
        Err(error) => {
            log::warn!("dropin: {tag} data object is not usable: {error}");
            return;
        }
    };
    let png = unsafe { RegisterClipboardFormatW(PCWSTR(wide("PNG").as_ptr())) };
    let formats = [
        ("CF_HDROP", CF_HDROP.0),
        ("CF_DIBV5", CF_DIBV5.0),
        ("CF_DIB", CF_DIB.0),
        ("PNG", u16::try_from(png).unwrap_or(u16::MAX)),
    ];
    let mut present: Vec<&str> = Vec::new();
    for (name, format) in formats {
        let format_etc = FORMATETC {
            cfFormat: format,
            ptd: std::ptr::null_mut(),
            dwAspect: 1, // DVASPECT_CONTENT
            lindex: -1,
            tymed: 1, // TYMED_HGLOBAL
        };
        if unsafe { data.QueryGetData(&format_etc) } == S_OK {
            present.push(name);
        }
    }
    log::info!(
        "dropin: {tag} formats available: {}",
        if present.is_empty() {
            "<none of CF_HDROP/CF_DIBV5/CF_DIB/PNG>".to_string()
        } else {
            present.join(", ")
        }
    );
    // Drop the borrowed wrapper without releasing the caller's reference.
    std::mem::forget(data);
}

/// Class name of a window, for diagnostics.
fn window_class(hwnd: HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if length > 0 {
        String::from_utf16_lossy(&buffer[..length as usize])
    } else {
        String::new()
    }
}

/// UTF-16 buffer with the trailing NUL, for the Win32 `PCWSTR` parameters.
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
