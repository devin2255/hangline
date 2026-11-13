//! Drag-in drop target (v1.5.0): hang external images on the line.
//!
//! The panel disables wry's drag-drop handler (`WebviewWindowBuilder::disable_drag_drop_handler`) and
//! registers this hand-written OLE `IDropTarget` on the panel window and every WebView2 child HWND —
//! the same registration strategy wry uses, but this target reads the actual `IDataObject` formats so
//! bitmap-only sources (WeChat/QQ chat images) work too, not just file paths.
//!
//! Accepted representations, in preference order (first usable one wins):
//! 1. `CF_HDROP` — image files dragged from Explorer/folders; each file is decoded;
//! 2. `"PNG"` — PNG bytes offered by browsers/screenshot tools;
//! 3. `CF_DIBV5` / `CF_DIB` — bitmap bytes (WeChat/QQ image drags).
//!
//! The OLE callback only extracts the raw payload (bounded to [`MAX_PAYLOAD_BYTES`]) and returns
//! immediately; decoding, dedupe and saving run on a worker thread so the OLE/UI thread never
//! blocks. Every accepted image is decoded to RGBA, re-encoded as PNG, hashed with BLAKE3-64, saved
//! as `Screenshots\Dropped yyyy-MM-dd HH.mm.ss.png`, gets a thumbnail, and is appended to the line
//! as `ItemKind::Dropped` (a copy: the source file/temp image can disappear afterwards).
//!
//! Events emitted to the panel webview: `dropin-hover {images}`, `dropin-leave {}`,
//! `dropin-result {added, duplicates, rejected}` (see `docs/superpowers/specs/2026-11-13-drag-in-drop-design.md`).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::core::{BOOL, Interface, Type, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINTL, E_NOINTERFACE, E_POINTER, S_OK};
use windows::Win32::System::Com::{
    IDataObject, FORMATETC, DVASPECT_CONTENT, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Vtbl, OleInitialize, RegisterDragDrop, ReleaseStgMedium,
    RevokeDragDrop, CF_DIB, CF_DIBV5, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetClassNameW};

use crate::item::{Item, ItemKind};

/// Event: an image drag is hovering the panel. Payload [`DropHoverDto`].
pub const EVENT_HOVER: &str = "dropin-hover";
/// Event: the drag left the panel (payload: none).
pub const EVENT_LEAVE: &str = "dropin-leave";
/// Event: one drop was processed. Payload [`DropResultDto`].
pub const EVENT_RESULT: &str = "dropin-result";

/// Maximum images processed from one drop (a multi-file drag must not stall the line).
pub const MAX_DROP_IMAGES: usize = 20;

/// Maximum raw payload accepted per representation (same cap as the clipboard watcher).
pub const MAX_PAYLOAD_BYTES: usize = crate::watchers::clipboard::MAX_PAYLOAD_BYTES;

/// Image file extensions accepted from `CF_HDROP`.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];

/// Hover payload (`images` is a cheap heuristic: image files present or one bitmap).
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropHoverDto {
    pub images: u32,
}

/// Result payload: how many cards were added, deduped, or rejected.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropResultDto {
    pub added: u32,
    pub duplicates: u32,
    pub rejected: u32,
}

/// One drag's owned payload, extracted synchronously in the OLE callback.
enum DropPayload {
    /// Image files (already filtered by extension; at most [`MAX_DROP_IMAGES`]).
    Files(Vec<PathBuf>),
    /// PNG bytes.
    Png(Vec<u8>),
    /// DIB (`CF_DIB` / `CF_DIBV5`) bytes.
    Dib(Vec<u8>),
}

/// Common first fields of a hand-written COM object (same layout as `dragout`'s objects).
#[repr(C)]
struct ComHeader {
    vtbl: *const c_void,
    refs: AtomicU32,
}

#[repr(C)]
struct DropTargetObj {
    header: ComHeader,
    app: AppHandle,
    /// Whether a hover event is currently outstanding (DragEnter/DragLeave can repeat).
    hover: AtomicBool,
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
pub fn install(window: &tauri::WebviewWindow, app: &AppHandle) -> Result<(), String> {
    // RegisterDragDrop requires an OLE-initialised (STA) thread. The UI thread hosts WebView2, which
    // already initialises COM; OleInitialize is reference counted, so a second call is harmless.
    if let Err(error) = unsafe { OleInitialize(None) } {
        log::warn!("dropin: OleInitialize failed; continuing (already initialised is fine): {error}");
    }
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    // The callback receives (`count`, `AppHandle`) through the LPARAM; the tuple must outlive the
    // enumeration call.
    let mut payload = (0usize, app.clone());
    register(hwnd, app, &mut payload.0)?;
    // SAFETY: `payload` outlives the enumeration call; the callback only touches its fields.
    unsafe {
        let _ = EnumChildWindows(
            Some(hwnd),
            Some(register_child),
            LPARAM(&mut payload as *mut (usize, AppHandle) as isize),
        );
    }
    let count = payload.0;
    if count == 0 {
        return Err("dropin: no HWND accepted the drop target".to_string());
    }
    log::info!("dropin: registered {count} drop target(s) on the panel window {hwnd:?}");
    Ok(())
}

/// `EnumChildWindows` callback: registers the drop target on one child and continues.
unsafe extern "system" fn register_child(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let (count, app) = unsafe { &mut *(lparam.0 as *mut (usize, AppHandle)) };
    if let Err(error) = register(hwnd, app, count) {
        log::debug!("dropin: child {hwnd:?} not registered: {error}");
    }
    BOOL(1)
}

fn register(hwnd: HWND, app: &AppHandle, count: &mut usize) -> Result<(), String> {
    let object = Box::new(DropTargetObj {
        header: ComHeader {
            vtbl: &DROP_TARGET_VTBL as *const IDropTarget_Vtbl as *const c_void,
            refs: AtomicU32::new(1),
        },
        app: app.clone(),
        hover: AtomicBool::new(false),
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
            log::debug!("dropin: registered on {hwnd:?} class={class:?}");
            // OLE keeps a borrowed pointer for the lifetime of the registration; never free it.
            std::mem::forget(target);
            *count += 1;
            Ok(())
        }
        Err(error) => {
            log::warn!("dropin: RegisterDragDrop failed for {hwnd:?} ({class}): {error}");
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

unsafe fn query_interface(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let supported = [
        <windows::core::IUnknown as Interface>::IID,
        <IDropTarget as Interface>::IID,
    ];
    if supported.contains(unsafe { &*iid }) {
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

/// The object behind `this` (the COM pointer is our own `DropTargetObj` allocation).
unsafe fn target<'a>(this: *mut c_void) -> &'a DropTargetObj {
    unsafe { &*(this as *const DropTargetObj) }
}

unsafe extern "system" fn drop_target_drag_enter(
    this: *mut c_void,
    data_object: *mut c_void,
    _keys: MODIFIERKEYS_FLAGS,
    _point: POINTL,
    effect: *mut DROPEFFECT,
) -> HRESULT {
    set_effect(effect);
    if data_object.is_null() {
        return S_OK;
    }
    let object = unsafe { target(this) };
    let data = unsafe { borrow_data_object(data_object) };
    let formats = unsafe { formats_present(&data) };
    log::debug!(
        "dropin: DragEnter hdrop={} png={} dibv5={} dib={}",
        formats.hdrop,
        formats.png,
        formats.dibv5,
        formats.dib
    );
    if formats.any_image() && !object.hover.swap(true, Ordering::SeqCst) {
        emit(&object.app, EVENT_HOVER, &DropHoverDto { images: 1 });
    }
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

unsafe extern "system" fn drop_target_drag_leave(this: *mut c_void) -> HRESULT {
    let object = unsafe { target(this) };
    if object.hover.swap(false, Ordering::SeqCst) {
        emit(&object.app, EVENT_LEAVE, &());
    }
    S_OK
}

unsafe extern "system" fn drop_target_drop(
    this: *mut c_void,
    data_object: *mut c_void,
    _keys: MODIFIERKEYS_FLAGS,
    _point: POINTL,
    effect: *mut DROPEFFECT,
) -> HRESULT {
    set_effect(effect);
    let object = unsafe { target(this) };
    if object.hover.swap(false, Ordering::SeqCst) {
        emit(&object.app, EVENT_LEAVE, &());
    }
    if data_object.is_null() {
        emit_drop_result(&object.app, DropResultDto { rejected: 1, ..Default::default() });
        return S_OK;
    }
    let data = unsafe { borrow_data_object(data_object) };
    match unsafe { extract_payload(&data) } {
        Some(payload) => spawn_ingest(object.app.clone(), payload),
        None => {
            log::info!("dropin: drop carried no supported image representation");
            emit_drop_result(&object.app, DropResultDto { rejected: 1, ..Default::default() });
        }
    }
    S_OK
}

fn set_effect(effect: *mut DROPEFFECT) {
    if !effect.is_null() {
        unsafe { *effect = DROPEFFECT_COPY };
    }
}

/// Borrows the caller's `IDataObject` without taking ownership of its reference.
unsafe fn borrow_data_object(raw: *mut c_void) -> IDataObject {
    // SAFETY: OLE passes a live IDataObject pointer for the duration of the callback; the wrapper is
    // forgotten so its Drop never releases the caller's reference.
    let data: IDataObject = unsafe { Type::from_abi(raw) }.expect("OLE passes a valid data object");
    std::mem::forget(data.clone());
    data
}

// ---------------------------------------------------------------------------------------------
// Format extraction (synchronous, bounded)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct Formats {
    hdrop: bool,
    png: bool,
    dibv5: bool,
    dib: bool,
}

impl Formats {
    fn any_image(self) -> bool {
        self.hdrop || self.png || self.dibv5 || self.dib
    }
}

fn png_format() -> u16 {
    static PNG: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *PNG.get_or_init(|| {
        let name = wide("PNG");
        u16::try_from(unsafe { RegisterClipboardFormatW(PCWSTR(name.as_ptr())) }).unwrap_or(u16::MAX)
    })
}

fn format_etc(cf_format: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf_format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn has_format(data: &IDataObject, cf_format: u16) -> bool {
    let format = format_etc(cf_format);
    (unsafe { data.QueryGetData(&format) }) == S_OK
}

unsafe fn formats_present(data: &IDataObject) -> Formats {
    Formats {
        hdrop: has_format(data, CF_HDROP.0),
        png: has_format(data, png_format()),
        dibv5: has_format(data, CF_DIBV5.0),
        dib: has_format(data, CF_DIB.0),
    }
}

/// Extracts the first usable image representation from the data object.
unsafe fn extract_payload(data: &IDataObject) -> Option<DropPayload> {
    // 1. File list: prefer real files (best quality), filtered to image extensions.
    if has_format(data, CF_HDROP.0) {
        let files = unsafe { hdrop_paths(data) };
        let images: Vec<PathBuf> = files
            .into_iter()
            .filter(|path| is_image_file(path))
            .take(MAX_DROP_IMAGES)
            .collect();
        if !images.is_empty() {
            log::info!("dropin: drop carries {} image file(s)", images.len());
            return Some(DropPayload::Files(images));
        }
    }
    // 2. PNG bytes.
    if has_format(data, png_format()) {
        if let Some(bytes) = unsafe { hglobal_bytes(data, png_format()) } {
            if bytes.len() <= MAX_PAYLOAD_BYTES {
                log::info!("dropin: drop carries PNG bytes ({} B)", bytes.len());
                return Some(DropPayload::Png(bytes));
            }
        }
    }
    // 3. Bitmap bytes (CF_DIBV5 preferred: it has alpha).
    for (label, format) in [("CF_DIBV5", CF_DIBV5.0), ("CF_DIB", CF_DIB.0)] {
        if has_format(data, format) {
            if let Some(bytes) = unsafe { hglobal_bytes(data, format) } {
                if bytes.len() <= MAX_PAYLOAD_BYTES {
                    log::info!("dropin: drop carries {label} bytes ({} B)", bytes.len());
                    return Some(DropPayload::Dib(bytes));
                }
            }
        }
    }
    None
}

/// Reads the `CF_HDROP` file list from the data object (empty when unavailable).
unsafe fn hdrop_paths(data: &IDataObject) -> Vec<PathBuf> {
    let format = format_etc(CF_HDROP.0);
    let mut medium = match unsafe { data.GetData(&format) } {
        Ok(medium) => medium,
        Err(error) => {
            log::warn!("dropin: cannot read CF_HDROP: {error}");
            return Vec::new();
        }
    };
    let mut paths = Vec::new();
    if medium.tymed == TYMED_HGLOBAL.0 as u32 {
        let hdrop = HDROP(medium.u.hGlobal.0);
        let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
        for index in 0..count {
            let length = unsafe { DragQueryFileW(hdrop, index, None) };
            let mut buffer = vec![0u16; length as usize + 1];
            let written = unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) };
            if written > 0 {
                paths.push(PathBuf::from(String::from_utf16_lossy(
                    &buffer[..written as usize],
                )));
            }
        }
    }
    unsafe { ReleaseStgMedium(&mut medium) };
    paths
}

/// Copies one `TYMED_HGLOBAL` payload out of the data object.
unsafe fn hglobal_bytes(data: &IDataObject, cf_format: u16) -> Option<Vec<u8>> {
    let format = format_etc(cf_format);
    let mut medium = match unsafe { data.GetData(&format) } {
        Ok(medium) => medium,
        Err(error) => {
            log::warn!("dropin: cannot read format {cf_format}: {error}");
            return None;
        }
    };
    let mut bytes = None;
    if medium.tymed == TYMED_HGLOBAL.0 as u32 {
        let handle = medium.u.hGlobal;
        let size = unsafe { GlobalSize(handle) };
        if size > 0 && size <= MAX_PAYLOAD_BYTES {
            let pointer = unsafe { GlobalLock(handle) };
            if !pointer.is_null() {
                let slice = unsafe { std::slice::from_raw_parts(pointer as *const u8, size) };
                bytes = Some(slice.to_vec());
                unsafe {
                    let _ = GlobalUnlock(handle);
                }
            }
        }
    }
    unsafe { ReleaseStgMedium(&mut medium) };
    bytes
}

fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            let lower = extension.to_ascii_lowercase();
            IMAGE_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------
// Ingestion worker (decode → PNG → dedupe → save → thumbnail → add_item)
// ---------------------------------------------------------------------------------------------

/// Decodes and hangs one drop's images, then broadcasts the state and the result event.
fn spawn_ingest(app: AppHandle, payload: DropPayload) {
    std::thread::Builder::new()
        .name("hangline-dropin-ingest".to_string())
        .spawn(move || {
            let result = ingest(&app, &payload);
            if result.added > 0 {
                // `emit_state` returns the newest snapshot (or `None` when the app is shutting down).
                let _ = crate::app_state::emit_state(&app);
            }
            emit_drop_result(&app, result);
        })
        .ok();
}

fn ingest(app: &AppHandle, payload: &DropPayload) -> DropResultDto {
    let dir = match crate::capture::screenshots_dir() {
        Ok(dir) => dir,
        Err(error) => {
            log::error!("dropin: cannot resolve the screenshot directory: {error:#}");
            return DropResultDto { rejected: 1, ..Default::default() };
        }
    };
    let state = app.state::<crate::app_state::AppState>();
    let deduper = app.state::<crate::dedupe::Deduper>();
    let mut result = DropResultDto::default();
    match payload {
        DropPayload::Files(paths) => {
            for path in paths {
                match decode_image_file(path) {
                    Ok(image) => {
                        if save_and_hang(app, &state, &deduper, &dir, image, &mut result) {
                            // counts are updated inside save_and_hang
                        }
                    }
                    Err(error) => {
                        log::warn!("dropin: cannot decode {}: {error}", path.display());
                        result.rejected += 1;
                    }
                }
            }
        }
        DropPayload::Png(bytes) => {
            let decoded = crate::watchers::clipboard::decode_png(bytes);
            match_image_decoded(decoded, app, &state, &deduper, &dir, &mut result);
        }
        DropPayload::Dib(bytes) => {
            let decoded = crate::watchers::clipboard::decode_dib(bytes);
            match_image_decoded(decoded, app, &state, &deduper, &dir, &mut result);
        }
    }
    log::info!(
        "dropin: processed drop (added={}, duplicates={}, rejected={})",
        result.added,
        result.duplicates,
        result.rejected
    );
    result
}

fn match_image_decoded(
    decoded: anyhow::Result<image::RgbaImage>,
    app: &AppHandle,
    state: &crate::app_state::AppState,
    deduper: &crate::dedupe::Deduper,
    dir: &Path,
    result: &mut DropResultDto,
) {
    match decoded {
        Ok(image) => {
            save_and_hang(app, state, deduper, dir, image, result);
        }
        Err(error) => {
            log::warn!("dropin: cannot decode the dragged bitmap: {error}");
            result.rejected += 1;
        }
    }
}

fn decode_image_file(path: &Path) -> Result<image::RgbaImage, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() as usize > MAX_PAYLOAD_BYTES {
        return Err(format!("file is too large ({} bytes)", metadata.len()));
    }
    let (width, height) = image::image_dimensions(path).map_err(|error| error.to_string())?;
    crate::watchers::clipboard::check_pixel_cap(width, height)
        .map_err(|error| format!("{error:#}"))?;
    let decoded = image::open(path).map_err(|error| error.to_string())?;
    Ok(decoded.to_rgba8())
}

/// Encodes, dedupes, saves and hangs one image. Updates `result`; returns whether a card was added.
fn save_and_hang(
    _app: &AppHandle,
    state: &crate::app_state::AppState,
    deduper: &crate::dedupe::Deduper,
    dir: &Path,
    image: image::RgbaImage,
    result: &mut DropResultDto,
) -> bool {
    let png = match crate::watchers::clipboard::encode_png(&image) {
        Ok(png) => png,
        Err(error) => {
            log::warn!("dropin: cannot encode the dropped image: {error:#}");
            result.rejected += 1;
            return false;
        }
    };
    let hash = crate::dedupe::hash_bytes(&png);
    // Cross-session duplicate: the dedupe LRU is in-memory only, so also compare against the items
    // already on the line (the same picture dropped in a later session must not hang twice).
    if state.snapshot().items.iter().any(|item| item.hash == hash) {
        log::info!("dropin: the same image content is already on the line (hash {hash:016x})");
        result.duplicates += 1;
        return false;
    }
    if !deduper.check_or_insert(hash) {
        log::info!("dropin: dedupe hit (hash {hash:016x}); the image is already hung");
        result.duplicates += 1;
        return false;
    }
    let path = crate::capture::unique_png_path(dir, &dropped_stem(unsafe { GetLocalTime() }));
    if let Err(error) = crate::watchers::clipboard::write_png_atomic(&path, &png) {
        deduper.forget(hash);
        log::warn!("dropin: cannot save {}: {error:#}", path.display());
        result.rejected += 1;
        return false;
    }
    crate::watchers::clipboard::apply_restrictive_permissions(&path);
    let (width, height) = (image.width(), image.height());
    let mut item = Item::new(path.clone(), ItemKind::Dropped, hash, width, height);
    crate::thumbnails::attach_from_image(&mut item, &image);
    match state.add_item(item) {
        Ok(_) => {
            result.added += 1;
            true
        }
        Err(error) => {
            log::error!(
                "dropin: cannot persist the dropped item for {}: {error:#}",
                path.display()
            );
            let _ = std::fs::remove_file(&path);
            deduper.forget(hash);
            result.rejected += 1;
            false
        }
    }
}

/// File stem for a dropped image: `Dropped yyyy-MM-dd HH.mm.ss` in local time.
fn dropped_stem(local: windows::Win32::Foundation::SYSTEMTIME) -> String {
    format!(
        "Dropped {:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

// ---------------------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------------------

fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: &T) {
    if let Err(error) = app.emit_to(crate::panel::LABEL, event, payload.clone()) {
        log::warn!("dropin: cannot emit {event}: {error}");
    }
}

fn emit_drop_result(app: &AppHandle, result: DropResultDto) {
    emit(app, EVENT_RESULT, &result);
}

/// Class name of a window, for diagnostics.
fn window_class(hwnd: HWND) -> String {
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

// ---------------------------------------------------------------------------------------------
// External drag watcher: click-through gate + top-edge reveal (v1.5.0)
// ---------------------------------------------------------------------------------------------

/// Poll cadence of the external-drag watcher.
pub const DRAG_POLL: Duration = Duration::from_millis(36);

/// Movement (physical pixels) after which a pressed left button counts as an external drag.
pub const DRAG_ARM_PX: i32 = 4;

/// Height of the top-edge reveal band below the monitor work-area top (dp).
pub const DROP_REVEAL_BAND_DP: f64 = 14.0;

/// Extra physical pixels around the panel window that still count as "over the panel".
pub const PANEL_MARGIN_PX: i32 = 12;

/// Starts the external-drag watcher (one thread for the app lifetime).
///
/// The watcher only reads global mouse state:
/// * a pressed left button plus DRAG_ARM_PX of movement arms the external-drag state;
/// * while armed, a cursor in the top reveal band of any monitor requests a drop reveal (the line
///   slides in, without pinning);
/// * while armed and the cursor is over the panel window, the hit-test thread keeps the panel
///   hit-testable (see crate::hittest::set_drag_hold) so the native drop target can receive the
///   OLE drop; otherwise click-through stays untouched.
///
/// Internal card drag-out is ignored via crate::dragout::is_dragging. Normal clicks never arm
/// (they have no movement), so click-through behaviour is unchanged.
pub fn spawn_watcher(app: &AppHandle) {
    let app = app.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("hangline-drop-watch".to_string())
        .spawn(move || watch(&app))
    {
        log::error!("dropin: cannot spawn the drag watcher: {error}");
    }
}

fn watch(app: &AppHandle) {
    let mut down_prev = false;
    let mut arm_origin: Option<(i32, i32)> = None;
    let mut armed = false;
    let mut reveal_requested = false;
    let mut gate_held = false;
    loop {
        std::thread::sleep(DRAG_POLL);
        if crate::dragout::is_dragging() {
            if gate_held || crate::reveal::drop_is_held() {
                crate::hittest::set_drag_hold(false);
                crate::reveal::set_drop_hold(false);
            }
            armed = false;
            arm_origin = None;
            reveal_requested = false;
            gate_held = false;
            down_prev = false;
            continue;
        }
        let down = left_button_down();
        let cursor = crate::geometry::cursor_position();
        if down && !down_prev {
            arm_origin = cursor;
            armed = false;
            reveal_requested = false;
        }
        if down {
            if let (Some(origin), Some(point)) = (arm_origin, cursor) {
                if !armed && distance_sq(origin, point) >= DRAG_ARM_PX * DRAG_ARM_PX {
                    armed = true;
                }
            }
            if armed {
                if !crate::reveal::drop_is_held() {
                    crate::reveal::set_drop_hold(true);
                }
                if !reveal_requested && !crate::panel::is_visible() {
                    if let Some(point) = cursor {
                        if in_reveal_band(point, &crate::geometry::monitors()) {
                            crate::reveal::request_drop_reveal(app);
                            reveal_requested = true;
                        }
                    }
                }
                let inside = cursor.is_some_and(|point| panel_contains(app, point));
                if inside != gate_held {
                    crate::hittest::set_drag_hold(inside);
                    gate_held = inside;
                }
            }
        } else {
            if armed || gate_held || crate::reveal::drop_is_held() {
                crate::hittest::set_drag_hold(false);
                crate::reveal::set_drop_hold(false);
                crate::reveal::end_drop_reveal(app);
                log::debug!("dropin: external drag ended");
            }
            armed = false;
            arm_origin = None;
            reveal_requested = false;
            gate_held = false;
        }
        down_prev = down;
    }
}

fn left_button_down() -> bool {
    // SAFETY: GetAsyncKeyState is a read-only query with no pointer arguments.
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

fn distance_sq(a: (i32, i32), b: (i32, i32)) -> i32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

/// Whether the point is inside the monitor's physical rect.
fn monitor_contains(monitor: &crate::geometry::MonitorInfo, point: (i32, i32)) -> bool {
    point.0 >= monitor.rect.x
        && point.0 < monitor.rect.x + monitor.rect.w
        && point.1 >= monitor.rect.y
        && point.1 < monitor.rect.y + monitor.rect.h
}

/// Whether the point is in the top reveal band of the monitor that contains it.
fn in_reveal_band(point: (i32, i32), monitors: &[crate::geometry::MonitorInfo]) -> bool {
    monitors
        .iter()
        .find(|monitor| monitor_contains(monitor, point))
        .is_some_and(|monitor| {
            let band = crate::geometry::dp_to_px(DROP_REVEAL_BAND_DP, monitor.scale_factor);
            let top = monitor.work_area.y;
            point.1 >= top - 2
                && point.1 <= top + band
                && point.0 >= monitor.work_area.x
                && point.0 < monitor.work_area.x + monitor.work_area.w
        })
}

/// Whether the point is inside the panel window (grown by PANEL_MARGIN_PX).
fn panel_contains(app: &AppHandle, point: (i32, i32)) -> bool {
    app.get_webview_window(crate::panel::LABEL)
        .and_then(|window| crate::panel::window_rect(&window))
        .is_some_and(|rect| {
            let left = rect.position.0 - PANEL_MARGIN_PX;
            let top = rect.position.1 - PANEL_MARGIN_PX;
            let right = rect.position.0 + rect.size.0 as i32 + PANEL_MARGIN_PX;
            let bottom = rect.position.1 + rect.size.1 as i32 + PANEL_MARGIN_PX;
            point.0 >= left && point.0 < right && point.1 >= top && point.1 < bottom
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{MonitorInfo, PhysicalRect};

    fn monitor(x: i32, y: i32, w: i32, h: i32, scale: f64) -> MonitorInfo {
        MonitorInfo {
            id: "test".to_string(),
            rect: PhysicalRect { x, y, w, h },
            work_area: PhysicalRect { x, y, w, h: h - 40 },
            scale_factor: scale,
            is_primary: true,
        }
    }

    #[test]
    fn drag_arming_needs_four_pixels_of_movement() {
        assert!(distance_sq((0, 0), (3, 0)) < DRAG_ARM_PX * DRAG_ARM_PX);
        assert!(distance_sq((0, 0), (4, 0)) >= DRAG_ARM_PX * DRAG_ARM_PX);
        assert!(distance_sq((10, 10), (13, 14)) >= DRAG_ARM_PX * DRAG_ARM_PX);
    }

    #[test]
    fn reveal_band_only_matches_the_top_edge_of_the_monitor_under_the_cursor() {
        let monitors = [monitor(0, 0, 1920, 1080, 1.0), monitor(-1920, 0, 1920, 1080, 1.5)];
        assert!(in_reveal_band((500, 0), &monitors));
        assert!(in_reveal_band((500, 14), &monitors));
        assert!(!in_reveal_band((500, 15), &monitors));
        assert!(in_reveal_band((-1000, 0), &monitors));
        assert!(in_reveal_band((-1000, 21), &monitors));
        assert!(!in_reveal_band((-1000, 22), &monitors));
        assert!(!in_reveal_band((5000, 5000), &monitors));
    }

    #[test]
    fn reveal_band_uses_the_work_area_top_not_the_monitor_top() {
        let mut top_taskbar = monitor(0, 60, 1920, 1080, 1.0);
        top_taskbar.work_area = PhysicalRect { x: 0, y: 60, w: 1920, h: 980 };
        let monitors = [top_taskbar];
        assert!(in_reveal_band((500, 60), &monitors));
        assert!(!in_reveal_band((500, 0), &monitors));
    }

    #[test]
    fn image_file_filter_accepts_common_extensions_case_insensitively() {
        for name in ["a.png", "b.JPG", "c.Jpeg", "d.bmp", "e.gif", "f.WEBP"] {
            assert!(is_image_file(Path::new(name)), "{name} must be accepted");
        }
        for name in ["a.txt", "b.mp4", "c.heic", "no-extension"] {
            assert!(!is_image_file(Path::new(name)), "{name} must be rejected");
        }
    }

    #[test]
    fn dropped_stem_is_local_time_and_prefixed() {
        let local = windows::Win32::Foundation::SYSTEMTIME {
            wYear: 2026,
            wMonth: 11,
            wDay: 13,
            wHour: 9,
            wMinute: 5,
            wSecond: 7,
            ..Default::default()
        };
        assert_eq!(dropped_stem(local), "Dropped 2026-11-13 09.05.07");
    }
}
