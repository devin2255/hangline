//! Native file drag-out (P0-E4), implemented in-house on top of OLE.
//!
//! # Why OLE instead of Tauri
//!
//! Tauri 2.12.5 has **no outbound file-drag API**: `WebviewWindow::start_dragging()` moves the
//! window (`WM_NCLBUTTONDOWN` + `HTCAPTION` in tao) and takes no paths or drag image, and
//! `tauri`/`tauri-runtime`/`tauri-runtime-wry` contain no `CF_HDROP`, `IDataObject`, `IDropSource`
//! or `DoDragDrop` (verified against the installed crate sources; see
//! `docs/experiments/P0-E4.md` §2). The `windows` crate is already a dependency, so the drag
//! source is implemented directly:
//!
//! * **Data object** — by default the shell's own `IDataObject` for the file
//!   (`SHCreateItemFromParsingName` + `IShellItem::BindToHandler(BHID_DataObject)`), which
//!   publishes a real `CF_HDROP` and the file's thumbnail metadata. If the shell cannot provide
//!   one, [`FileDataObject`] builds `CF_HDROP` by hand (`GMEM_MOVEABLE` + `DROPFILES` + a
//!   double-NUL-terminated UTF-16 path list).
//! * **Drop source** — [`DropSource`] implements `IDropSource`: Escape cancels, releasing the
//!   left button performs the drop, otherwise the drag continues; `GiveFeedback` asks OLE for
//!   its default drop cursors (no custom drag image yet).
//! * **Threading** — `DoDragDrop` runs a modal loop and must be called from a COM STA after
//!   `OleInitialize`. The Tauri IPC thread is not guaranteed to be an STA and blocking it would
//!   stall the app's event loop, so [`start_drag`] returns as soon as the session is accepted and
//!   the drag runs on a dedicated thread ([`DRAG_THREAD_NAME`]) for its whole duration.
//! * **Session flag** — [`DragSession`] keeps [`is_dragging`] set for the whole `DoDragDrop`
//!   call, so `hittest` never flips the panel to click-through mid-drag.
//! * **Outcome** — the drag thread emits the `drag-ended` event to the panel with the final
//!   `DROPEFFECT` (one of `copy`/`move`/`link`/`none`); the command itself only acknowledges the
//!   start and reports immediate errors (bad path, drag already running, thread spawn failure).
//! * **Stuck loop** — `DoDragDrop` only calls `IDropSource` while input reaches its modal loop, so
//!   a locked/secure desktop or lost mouse capture can idle it forever. A watchdog thread cancels
//!   a drag that has not seen a single `QueryContinueDrag` callback within [`DRAG_WATCHDOG`] and
//!   force-clears the session if the loop still does not return; [`DragEnded`] then carries
//!   `ok: false` and an error message.
//! * **Lost events** — each drag gets a monotonically increasing session token that is included
//!   in `drag-ended`, so the panel can ignore a delayed event from an older session.
//!
//! `DoDragDrop` offers `DROPEFFECT_COPY | DROPEFFECT_MOVE` (never `LINK`): dropping into an app
//! copies the file, while Explorer performs the move for folder/desktop targets and a shell delete
//! for the Recycle Bin. The drag source only advertises the effects; the drop target performs the
//! operation. This module never copies, moves or deletes the source file. [`after_drag_action`] is
//! the post-drag policy: it maps the final effect and the source file's existence onto
//! [`AfterDragAction`], and the `after_drag` command then either prunes
//! ([`crate::app_state::AppState::prune`]), retries a slow shell `MOVE`, or leaves the card alone.
//!
//! The two COM objects are hand-written vtable implementations rather than
//! `windows::core::implement`: that macro expands to `::windows_core::...` absolute paths and
//! therefore requires a direct `windows-core` dependency, which this task must not add.

use std::any::Any;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, Type, PCWSTR};
use windows::Win32::Foundation::{
    GlobalFree, HGLOBAL, HWND, LPARAM, POINT, WPARAM, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP,
    DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC, DV_E_TYMED, E_NOINTERFACE, E_NOTIMPL,
    E_OUTOFMEMORY, E_POINTER, OLE_E_ADVISENOTSUPPORTED, S_OK,
};
use windows::Win32::System::Com::{
    IDataObject, IDataObject_Vtbl, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Vtbl, OleInitialize, OleUninitialize, CF_HDROP, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows::Win32::UI::Shell::{
    IShellItem, SHCreateItemFromParsingName, BHID_DataObject, DROPFILES,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetGUIThreadInfo, PostMessageW, PostThreadMessageW, GUITHREADINFO, WM_CANCELMODE, WM_KEYDOWN,
    WM_MOUSEMOVE,
};

use crate::panel;

/// Directory (under `%TEMP%`) holding the P0-E4 demo file; never committed.
pub const DEMO_DIR: &str = "hangline-e4-demo";

/// Demo file name used by the placeholder card until real items land (Task 10).
pub const DEMO_FILE: &str = "hangline-e4-demo.png";

/// Event emitted to the panel when an OLE drag session finishes.
///
/// Payload: [`DragEnded`]. The frontend clears its pressed-card state from this event, so it is
/// emitted for cancellations and failures too.
pub const DRAG_ENDED_EVENT: &str = "drag-ended";

/// Name of the dedicated STA thread that owns `DoDragDrop` for one session.
pub const DRAG_THREAD_NAME: &str = "hangline-ole-drag";

/// Name of the watchdog thread that guards one drag session.
pub const DRAG_WATCHDOG_THREAD_NAME: &str = "hangline-drag-watchdog";

/// How long `DoDragDrop` may run without a single `IDropSource::QueryContinueDrag` callback
/// before the drag is treated as stuck.
///
/// `DoDragDrop` runs its own modal message loop and only calls the drop source while input
/// arrives. If the mouse is lost to a locked/UAC/secure desktop, the loop can wait forever: the
/// session flag would stay set (the hit-test loop would never touch click-through again) and
/// every later drag would fail with "already in progress".
pub const DRAG_WATCHDOG: Duration = Duration::from_secs(8);

/// How long the drag loop gets to return after the watchdog posted its cancel messages, before
/// the watchdog force-clears the session and reports the failure itself.
pub const DRAG_WATCHDOG_GRACE: Duration = Duration::from_secs(1);

/// Poll cadence of the watchdog wait loops.
const WATCHDOG_POLL: Duration = Duration::from_millis(50);

/// Error reported when the watchdog had to abort a drag that never received input.
const WATCHDOG_ABORT_ERROR: &str =
    "the OLE drag loop received no input (locked or secure desktop?); the drag watchdog canceled \
     the session";

/// Session token of the drag that currently owns the mouse, or `0` when no drag is running.
///
/// The 30 Hz hit-test loop must not change `ignore_cursor_events` while this is non-zero: the OS
/// drag loop owns the mouse, and flipping the window to click-through mid-drag can cancel the
/// drag feedback. The value is the session id (not a bare bool) so a stale drop guard can never
/// clear the flag of a newer session.
static ACTIVE_DRAG_SESSION: AtomicU64 = AtomicU64::new(0);

/// Allocates the monotonic session tokens carried by `drag-ended` events.
static NEXT_DRAG_SESSION: AtomicU64 = AtomicU64::new(0);

/// Returns whether a native drag session is currently in flight.
pub fn is_dragging() -> bool {
    ACTIVE_DRAG_SESSION.load(Ordering::SeqCst) != 0
}

/// Allocates the token for a new drag session; starts at 1 so `0` can mean "none".
fn next_session_id() -> u64 {
    NEXT_DRAG_SESSION.fetch_add(1, Ordering::SeqCst) + 1
}

/// RAII drag-session marker; clears the flag even if the drag engine panics/unwinds.
pub struct DragSession {
    session_id: u64,
}

impl DragSession {
    /// Starts a drag session, or fails if another drag is already running.
    ///
    /// One drag at a time: a second `DoDragDrop` on another thread would fight over the mouse
    /// capture and the panel's drag flag.
    fn begin(session_id: u64) -> Result<Self, String> {
        ACTIVE_DRAG_SESSION
            .compare_exchange(0, session_id, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self { session_id })
            .map_err(|_| "a native drag-out is already in progress".to_owned())
    }
}

impl Drop for DragSession {
    fn drop(&mut self) {
        // Only clear the flag while this session still owns it. The watchdog may already have
        // force-cleared a stuck session and a newer drag may own the flag by now; a stale guard
        // (e.g. an OLE loop that finally returned) must not clear that newer session.
        let _ = ACTIVE_DRAG_SESSION.compare_exchange(
            self.session_id,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
}

/// Force-clears a session's claim on the global drag flag.
///
/// Called by the drag watchdog when a stuck `DoDragDrop` ignores the cancel request: the hit-test
/// loop and new gestures must not stay blocked by a thread that never returns. Returns whether
/// this call cleared the flag (i.e. the session still owned it).
fn force_end_session(session_id: u64) -> bool {
    ACTIVE_DRAG_SESSION
        .compare_exchange(session_id, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// Payload of the `drag-ended` event.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragEnded {
    /// Monotonic identity of the drag session this event belongs to (never reused).
    ///
    /// The panel ignores events whose token is not newer than the last token it handled, so a
    /// delayed event from an older session can never clear a newer drag's pressed state.
    pub session_id: u64,
    /// `true` when OLE returned normally (drop *or* cancel); `false` when the drag failed, the
    /// drag thread panicked, or the watchdog aborted a loop that received no input.
    pub ok: bool,
    /// Final `DROPEFFECT` reported by OLE: `"copy"`, `"move"`, `"link"` or `"none"`.
    pub effect: &'static str,
    /// `true` when `DoDragDrop` returned `DRAGDROP_S_DROP` (the user released over a target).
    ///
    /// Note that `dropped` only says *the source performed a drop*: a drop on a window that is
    /// not a registered OLE drop target still returns `DRAGDROP_S_DROP` with effect `"none"`.
    pub dropped: bool,
    /// Present when the drag could not run (COM/OLE initialisation or `DoDragDrop` error), the
    /// drag thread panicked, or the watchdog had to break a stuck modal loop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DragEnded {
    fn finished(effect: DROPEFFECT, dropped: bool) -> Self {
        Self {
            session_id: 0,
            ok: true,
            effect: effect_name(effect),
            dropped,
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            session_id: 0,
            ok: false,
            effect: "none",
            dropped: false,
            error: Some(error),
        }
    }

    /// Attaches the session token allocated by `start_drag` (constructors do not need it).
    fn with_session(mut self, session_id: u64) -> Self {
        self.session_id = session_id;
        self
    }
}

/// Final effect advertised to a drop target, mapped from OLE's `DROPEFFECT` bits.
///
/// `COPY` means the target takes a copy and the source file stays; `MOVE` means a shell target
/// (folder, desktop, Recycle Bin) performs a move/delete and the source path disappears; `LINK`
/// is mapped for completeness but never offered. `NONE` covers a cancel, a target that refused the
/// drop and every failed drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragEffect {
    Copy,
    Move,
    Link,
    None,
}

impl DragEffect {
    /// Maps an OLE `DROPEFFECT` bit set to the effect the panel and the prune policy use.
    fn from_ole(effect: DROPEFFECT) -> Self {
        if effect.contains(DROPEFFECT_COPY) {
            Self::Copy
        } else if effect.contains(DROPEFFECT_MOVE) {
            Self::Move
        } else if effect.contains(DROPEFFECT_LINK) {
            Self::Link
        } else {
            Self::None
        }
    }

    /// Parses the lowercase effect name carried by the `drag-ended` event.
    ///
    /// Returns `None` for an unknown name; the `after_drag` command logs that and treats it as
    /// [`Self::None`] (the conservative "not a prune signal unless the file is gone" policy).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "copy" => Some(Self::Copy),
            "move" => Some(Self::Move),
            "link" => Some(Self::Link),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    /// Returns the lowercase name carried by logs and the `drag-ended` event.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Link => "link",
            Self::None => "none",
        }
    }
}

/// Maps an OLE `DROPEFFECT` bit set to the name used by logs and the `drag-ended` event.
fn effect_name(effect: DROPEFFECT) -> &'static str {
    DragEffect::from_ole(effect).as_str()
}

/// What the `after_drag` command should do once a drag has ended and the source file was probed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterDragAction {
    /// The card's file is gone: run `AppState::prune` (which persists and bumps the revision).
    Prune,
    /// A shell `MOVE` whose source still exists: the move may not have landed yet, so retry.
    KeepWaiting,
    /// The effect and existence agree that the card stays on the line.
    Done,
}

/// Post-drag prune policy: the effect-driven half of the decision.
///
/// The shell, not Hangline, performs the drop: a `MOVE` target (folder, desktop, Recycle Bin) has
/// already handed the file over, a `COPY` target left it in place, and a cancelled drag (`NONE`)
/// must change nothing. `MOVE` + existing source is the one case that needs patience: a cross-volume
/// or slow shell move can report the effect before the source path disappears, so the command polls
/// instead of pruning or keeping immediately. Every effect whose source vanished prunes — a card
/// must never point at a missing file, even when the loss came from elsewhere (`COPY`/`LINK`/`NONE`
/// safety rows). `LINK` is never offered by this drag source; it is mapped for completeness.
pub fn after_drag_action(effect: DragEffect, file_exists: bool) -> AfterDragAction {
    match (effect, file_exists) {
        // The shell move may still be in flight: re-check instead of keeping a doomed card.
        (DragEffect::Move, true) => AfterDragAction::KeepWaiting,
        // A folder/desktop/Recycle Bin MOVE took the source file away.
        (DragEffect::Move, false) => AfterDragAction::Prune,
        // A copy target keeps the source; a copy whose source vanished still loses the card.
        (DragEffect::Copy, true) => AfterDragAction::Done,
        (DragEffect::Copy, false) => AfterDragAction::Prune,
        // Never offered, but a vanished source must not leave a card pointing at a missing file.
        (DragEffect::Link, true) => AfterDragAction::Done,
        (DragEffect::Link, false) => AfterDragAction::Prune,
        // A cancel/no-target is not a prune signal, unless the file is gone (safety net).
        (DragEffect::None, true) => AfterDragAction::Done,
        (DragEffect::None, false) => AfterDragAction::Prune,
    }
}

/// Effects this drag source offers: copy (apps) or move (folder/desktop/Recycle Bin), never link.
fn allowed_effects() -> DROPEFFECT {
    DROPEFFECT_COPY | DROPEFFECT_MOVE
}

/// Returns the absolute temp path of the P0 demo file.
pub fn demo_file_path() -> PathBuf {
    std::env::temp_dir().join(DEMO_DIR).join(DEMO_FILE)
}

/// Validates the drag source path.
///
/// Only existing absolute regular files are accepted: `CF_HDROP` hands Explorer a path, so a
/// relative path, a directory or a missing file would fail at the shell boundary instead of
/// here. The path is returned unchanged (no canonicalisation) so the file the user sees is the
/// file the shell receives.
pub fn validate_path(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(format!("drag path must be absolute: {}", path.display()));
    }
    let metadata = path
        .metadata()
        .map_err(|err| format!("drag path is not accessible: {}: {err}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("drag path is not a file: {}", path.display()));
    }
    Ok(path)
}

/// Creates the demo PNG when missing and returns its path.
fn ensure_demo_file(path: &Path) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("demo file path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("failed to create demo dir {}: {err}", parent.display()))?;

    // Deterministic gradient so repeated experiments produce identical bytes (stable hash).
    let img = image::RgbaImage::from_fn(320, 200, |x, y| {
        let r = (x * 255 / 319) as u8;
        let g = (y * 255 / 199) as u8;
        image::Rgba([r, 128, 255u8.saturating_sub(g), 255])
    });
    image::DynamicImage::ImageRgba8(img)
        .save_with_format(path, image::ImageFormat::Png)
        .map_err(|err| format!("failed to create demo PNG {}: {err}", path.display()))
}

/// Returns a real temp PNG the placeholder card can offer to the drag engine.
///
/// This is P0 scaffolding: Task 10 replaces the placeholder card with real items and the path
/// will come from the item store. The file is generated in `%TEMP%` and is never committed.
#[tauri::command]
pub fn drag_demo_file() -> Result<String, String> {
    let path = demo_file_path();
    ensure_demo_file(&path)?;
    log::debug!("dragout: demo file ready at {}", path.display());
    Ok(path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------------------------
// OLE apartment
// ---------------------------------------------------------------------------------------------

/// `OleInitialize`/`OleUninitialize` for one thread, as a drop guard.
///
/// `OleInitialize` puts the thread into an STA and is required before `DoDragDrop`; it is
/// reference counted per thread, so the guard only uninitialises its own call.
struct OleApartment;

impl OleApartment {
    fn init() -> Result<Self, String> {
        unsafe { OleInitialize(None) }.map_err(|err| format!("OleInitialize failed: {err}"))?;
        Ok(Self)
    }
}

impl Drop for OleApartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

// ---------------------------------------------------------------------------------------------
// COM plumbing (hand-written vtables; no `windows-core` dependency)
// ---------------------------------------------------------------------------------------------

/// Common first fields of a hand-written COM object: vtable pointer (offset 0, as COM requires)
/// and reference count.
#[repr(C)]
struct ComHeader {
    vtbl: *const c_void,
    refs: AtomicU32,
}

/// Wraps a freshly allocated object pointer into an interface without adding a reference.
///
/// # Safety
/// `raw` must point to a valid COM object whose vtable belongs to `T`'s interface and whose
/// reference count already accounts for the returned reference.
unsafe fn wrap_interface<T: Interface>(raw: *mut c_void) -> T {
    unsafe { Type::from_abi(raw) }.expect("a freshly allocated COM object pointer is never null")
}

unsafe fn add_ref(this: *mut c_void) -> u32 {
    let header = unsafe { &*(this as *const ComHeader) };
    header.refs.fetch_add(1, Ordering::SeqCst) + 1
}

/// Releases one reference and frees `T` when the count reaches zero.
///
/// # Safety
/// `this` must point to a live object of type `T` whose first field is a [`ComHeader`].
unsafe fn release<T>(this: *mut c_void) -> u32 {
    let header = unsafe { &*(this as *const ComHeader) };
    let refs = header.refs.fetch_sub(1, Ordering::SeqCst);
    if refs == 1 {
        drop(unsafe { Box::from_raw(this as *mut T) });
        0
    } else {
        refs - 1
    }
}

/// Answers `IUnknown::QueryInterface` for an object that exposes `supported`.
unsafe fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
    supported: &[GUID],
) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
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

// ---------------------------------------------------------------------------------------------
// Drop source
// ---------------------------------------------------------------------------------------------

/// How [`DropSource`] answers `IDropSource::QueryContinueDrag`.
#[derive(Clone, Copy)]
enum DropBehavior {
    /// Production: Escape cancels, releasing the left button drops, otherwise keep dragging.
    Interactive,
    /// Evidence harness: answer every query with this fixed HRESULT.
    #[allow(dead_code)] // only the #[ignore]d OLE evidence tests build this variant
    Fixed(HRESULT),
}

/// Callback counters, logged after each drag and used by the OLE evidence tests.
#[derive(Default)]
struct DropSourceCounters {
    query_continue_drag: AtomicUsize,
    give_feedback: AtomicUsize,
}

#[repr(C)]
struct DropSourceObj {
    header: ComHeader,
    behavior: DropBehavior,
    counters: Arc<DropSourceCounters>,
    /// Present for production drags: lets the watchdog observe the first callback and force a
    /// cancel when the modal loop never received any input.
    watchdog: Option<Arc<DragWatchdog>>,
}

static DROP_SOURCE_VTBL: IDropSource_Vtbl = IDropSource_Vtbl {
    base__: windows::core::IUnknown_Vtbl {
        QueryInterface: drop_source_query_interface,
        AddRef: drop_source_add_ref,
        Release: drop_source_release,
    },
    QueryContinueDrag: drop_source_query_continue_drag,
    GiveFeedback: drop_source_give_feedback,
};

unsafe extern "system" fn drop_source_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    const SUPPORTED: [GUID; 2] = [
        <IUnknown as Interface>::IID,
        <IDropSource as Interface>::IID,
    ];
    unsafe { query_interface(this, iid, out, &SUPPORTED) }
}

unsafe extern "system" fn drop_source_add_ref(this: *mut c_void) -> u32 {
    unsafe { add_ref(this) }
}

unsafe extern "system" fn drop_source_release(this: *mut c_void) -> u32 {
    unsafe { release::<DropSourceObj>(this) }
}

unsafe extern "system" fn drop_source_query_continue_drag(
    this: *mut c_void,
    escape_pressed: BOOL,
    keys: MODIFIERKEYS_FLAGS,
) -> HRESULT {
    let object = unsafe { &*(this as *const DropSourceObj) };
    object
        .counters
        .query_continue_drag
        .fetch_add(1, Ordering::Relaxed);
    // A callback means the modal loop is receiving input, so the watchdog can stand down.
    if let Some(watchdog) = &object.watchdog {
        watchdog.note_input();
    }
    match object.behavior {
        DropBehavior::Interactive => {
            // Set when the watchdog saw no input at all. The watchdog also posts a message that
            // forces this callback, so the drag ends through OLE's normal cancel path instead of
            // being abandoned in the modal loop.
            if object
                .watchdog
                .as_ref()
                .is_some_and(|watchdog| watchdog.cancel_requested())
            {
                DRAGDROP_S_CANCEL
            } else {
                query_continue_decision(escape_pressed.as_bool(), keys)
            }
        }
        DropBehavior::Fixed(hr) => hr,
    }
}

unsafe extern "system" fn drop_source_give_feedback(
    this: *mut c_void,
    _effect: DROPEFFECT,
) -> HRESULT {
    let object = unsafe { &*(this as *const DropSourceObj) };
    object.counters.give_feedback.fetch_add(1, Ordering::Relaxed);
    // OLE draws the standard copy/move/no-drop cursors for the effect each target returns.
    DRAGDROP_S_USEDEFAULTCURSORS
}

/// `IDropSource` for one drag session (owns a COM reference).
struct DropSource {
    interface: IDropSource,
    counters: Arc<DropSourceCounters>,
}

impl DropSource {
    fn interactive(watchdog: &Arc<DragWatchdog>) -> Result<Self, String> {
        Self::new(DropBehavior::Interactive, Some(watchdog.clone()))
    }

    fn new(behavior: DropBehavior, watchdog: Option<Arc<DragWatchdog>>) -> Result<Self, String> {
        let counters = Arc::new(DropSourceCounters::default());
        let object = Box::new(DropSourceObj {
            header: ComHeader {
                vtbl: &DROP_SOURCE_VTBL as *const IDropSource_Vtbl as *const c_void,
                refs: AtomicU32::new(1),
            },
            behavior,
            counters: counters.clone(),
            watchdog,
        });
        let raw = Box::into_raw(object) as *mut c_void;
        let interface = unsafe { wrap_interface::<IDropSource>(raw) };
        Ok(Self {
            interface,
            counters,
        })
    }
}

/// `QueryContinueDrag` decision, factored out so it can be unit-tested without OLE.
///
/// * Escape (OLE's `fEscapePressed`, plus a direct physical check because OLE does not always
///   report it while the button is still captured) cancels the drag.
/// * Left button still down → keep dragging.
/// * Left button released → complete the drop at the current position.
fn query_continue_decision(escape: bool, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
    if escape || escape_physically_down() {
        DRAGDROP_S_CANCEL
    } else if keys.contains(MK_LBUTTON) {
        S_OK
    } else {
        DRAGDROP_S_DROP
    }
}

/// Belt-and-braces Escape check; `GetAsyncKeyState` reports the physical key even while the
/// OLE drag loop owns mouse capture.
fn escape_physically_down() -> bool {
    let state = unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) };
    state as u16 & 0x8000 != 0
}

// ---------------------------------------------------------------------------------------------
// Drag watchdog
// ---------------------------------------------------------------------------------------------

/// What the watchdog should do after observing a running drag for some time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchdogAction {
    /// Still inside the timeout: keep waiting.
    KeepWaiting,
    /// The drag loop is alive (input arrived) or already ended: stop watching.
    Stop,
    /// No input arrived within the timeout: break the modal loop.
    Cancel,
}

/// Pure watchdog decision, unit-tested without OLE.
///
/// A `QueryContinueDrag` callback is the liveness signal: OLE only calls the drop source while
/// its modal loop is receiving mouse/keyboard messages, so a drag that has seen a callback is a
/// normal (possibly long) drag. A drag with no callback after `timeout` is stuck.
fn watchdog_action(seen_input: bool, elapsed: Duration, timeout: Duration) -> WatchdogAction {
    if seen_input {
        WatchdogAction::Stop
    } else if elapsed >= timeout {
        WatchdogAction::Cancel
    } else {
        WatchdogAction::KeepWaiting
    }
}

/// Shared progress state for one drag session.
#[derive(Default)]
struct DragWatchdog {
    /// At least one `QueryContinueDrag` callback reached the drag source.
    input_seen: AtomicBool,
    /// `DoDragDrop` returned (the drag thread is about to report).
    finished: AtomicBool,
    /// The watchdog asked the drag loop to cancel; the outcome is reported as an error.
    canceled: AtomicBool,
    /// Exactly one `drag-ended` event may be emitted per session, even when the watchdog and the
    /// drag thread race to report the same stuck session.
    emit_claimed: AtomicBool,
}

impl DragWatchdog {
    fn note_input(&self) {
        self.input_seen.store(true, Ordering::SeqCst);
    }

    fn mark_finished(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }

    fn cancel_requested(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }

    fn request_cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }

    /// Claims the right to emit this session's `drag-ended` event.
    fn claim_emit(&self) -> bool {
        self.emit_claimed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

/// Waits until the drag reports input, ends, or `timeout` elapses without input.
fn wait_for_drag_progress(state: &DragWatchdog, timeout: Duration) -> WatchdogAction {
    let started = Instant::now();
    loop {
        if state.finished.load(Ordering::SeqCst) {
            return WatchdogAction::Stop;
        }
        match watchdog_action(
            state.input_seen.load(Ordering::SeqCst),
            started.elapsed(),
            timeout,
        ) {
            WatchdogAction::KeepWaiting => thread::sleep(WATCHDOG_POLL),
            action => return action,
        }
    }
}

/// Posts messages that break a stuck OLE modal loop.
///
/// `WM_CANCELMODE` is Windows' "cancel the current mode" message; it is posted to the drag thread
/// and to the window that thread holds mouse capture on (OLE captures the mouse for the drag). A
/// synthetic `WM_MOUSEMOVE` is the last resort: the drag loop only calls `IDropSource` when a
/// message arrives, and [`DropSource`] answers `DRAGDROP_S_CANCEL` once the cancel flag is set.
///
/// Returns how many messages were accepted by the OS (for logging).
fn post_cancel_messages(thread_id: u32) -> usize {
    let mut posted = 0usize;

    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let capture: Option<HWND> = match unsafe { GetGUIThreadInfo(thread_id, &mut info) } {
        Ok(()) if !info.hwndCapture.0.is_null() => Some(info.hwndCapture),
        Ok(()) => None,
        Err(err) => {
            log::debug!("dragout: GetGUIThreadInfo({thread_id}) failed: {err}");
            None
        }
    };

    if let Some(hwnd) = capture {
        // The drag window owns the capture; a cancel-mode there is OLE's own abort path.
        if unsafe { PostMessageW(Some(hwnd), WM_CANCELMODE, WPARAM(0), LPARAM(0)) }.is_ok() {
            posted += 1;
        }
        if unsafe {
            PostMessageW(
                Some(hwnd),
                WM_KEYDOWN,
                WPARAM(VK_ESCAPE.0 as usize),
                LPARAM(0),
            )
        }
        .is_ok()
        {
            posted += 1;
        }
    }

    if unsafe { PostThreadMessageW(thread_id, WM_CANCELMODE, WPARAM(0), LPARAM(0)) }.is_ok() {
        posted += 1;
    }
    if unsafe {
        PostThreadMessageW(
            thread_id,
            WM_KEYDOWN,
            WPARAM(VK_ESCAPE.0 as usize),
            LPARAM(0),
        )
    }
    .is_ok()
    {
        posted += 1;
    }
    // Wake the modal loop so it calls `IDropSource`, which now answers CANCEL.
    if unsafe { PostThreadMessageW(thread_id, WM_MOUSEMOVE, WPARAM(MK_LBUTTON.0 as usize), LPARAM(0)) }
        .is_ok()
    {
        posted += 1;
    }
    posted
}

/// Asks a drag loop that received no input to cancel, then waits `grace` for it to return.
///
/// Returns `true` when the loop ignored the messages; the caller must then force-clear the session
/// and report the failure itself, because the drag thread may never reach its own cleanup.
fn cancel_stuck_drag(
    state: &DragWatchdog,
    thread_id: u32,
    timeout: Duration,
    grace: Duration,
) -> bool {
    state.request_cancel();
    log::warn!(
        "dragout: OLE drag loop received no input within {} ms; posting WM_CANCELMODE to drag \
         thread {thread_id}",
        timeout.as_millis()
    );
    let posted = post_cancel_messages(thread_id);
    log::debug!("dragout: watchdog posted {posted} cancel message(s) to drag thread {thread_id}");

    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if state.finished.load(Ordering::SeqCst) {
            log::info!("dragout: OLE drag loop returned after the watchdog cancel request");
            return false;
        }
        thread::sleep(WATCHDOG_POLL);
    }
    true
}

/// Emits the one `drag-ended` event allowed for a session.
fn emit_drag_ended(app: &AppHandle, state: &DragWatchdog, ended: DragEnded) {
    if !state.claim_emit() {
        log::debug!(
            "dragout: drag-ended for session {} was already emitted",
            ended.session_id
        );
        return;
    }
    if let Err(err) = app.emit_to(panel::LABEL, DRAG_ENDED_EVENT, ended) {
        // The panel also has a 30 s watchdog and resets on the next pointerdown, so a failed
        // emit can no longer wedge the card.
        log::warn!("dragout: failed to emit {DRAG_ENDED_EVENT}: {err}");
    }
}

/// Starts the watchdog thread for one drag session.
fn spawn_drag_watchdog(
    app: AppHandle,
    session_id: u64,
    thread_id: u32,
    state: Arc<DragWatchdog>,
    timeout: Duration,
    grace: Duration,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name(DRAG_WATCHDOG_THREAD_NAME.to_owned())
        .spawn(move || match wait_for_drag_progress(&state, timeout) {
            WatchdogAction::Cancel => {
                if !cancel_stuck_drag(&state, thread_id, timeout, grace) {
                    return;
                }
                let cleared = force_end_session(session_id);
                log::error!(
                    "dragout: OLE drag loop is still blocked after the watchdog cancel request; \
                     session {session_id} force-cleared (owned by this drag: {cleared})"
                );
                emit_drag_ended(
                    &app,
                    &state,
                    DragEnded::failed(WATCHDOG_ABORT_ERROR.to_owned()).with_session(session_id),
                );
            }
            // Input arrived (normal drag) or the loop already ended: nothing to watch.
            WatchdogAction::Stop | WatchdogAction::KeepWaiting => {}
        })
}

/// Extracts a printable message from a `catch_unwind` payload.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

// ---------------------------------------------------------------------------------------------
// CF_HDROP data object
// ---------------------------------------------------------------------------------------------

/// `DROPFILES` size in bytes; `pFiles` must point just past it.
const DROPFILES_SIZE: usize = std::mem::size_of::<DROPFILES>();

/// Builds the `CF_HDROP` payload for one path: `DROPFILES` header + UTF-16 path list.
///
/// The list is NUL-separated and terminated by an extra NUL (two NULs after the last path),
/// with `fWide = TRUE` so every character is a UTF-16 code unit.
fn hdrop_bytes(path: &Path) -> Vec<u8> {
    debug_assert_eq!(DROPFILES_SIZE, 20, "DROPFILES layout changed");
    let units: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .chain(std::iter::once(0))
        .collect();
    let header = DROPFILES {
        pFiles: DROPFILES_SIZE as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL::from(false),
        fWide: BOOL::from(true),
    };
    let mut bytes = Vec::with_capacity(DROPFILES_SIZE + units.len() * 2);
    // The header is a plain C struct; copying its bytes avoids alignment assumptions on Vec<u8>.
    bytes.extend_from_slice(unsafe {
        std::slice::from_raw_parts(&header as *const DROPFILES as *const u8, DROPFILES_SIZE)
    });
    for unit in units {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// Allocates an `HGLOBAL` of `GMEM_MOVEABLE` memory holding `bytes`, as OLE expects for
/// `TYMED_HGLOBAL`/`CF_HDROP` (`ReleaseStgMedium` frees it).
fn global_copy(bytes: &[u8]) -> windows::core::Result<HGLOBAL> {
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let target = GlobalLock(handle);
        if target.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
        // GlobalUnlock reports FALSE once the lock count reaches zero, which is exactly the
        // success case here; there is nothing actionable in that error.
        let _ = GlobalUnlock(handle);
        Ok(handle)
    }
}

/// Builds the `STGMEDIUM` that carries a `CF_HDROP` `HGLOBAL`.
fn hdrop_medium(path: &Path) -> windows::core::Result<STGMEDIUM> {
    Ok(STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 {
            hGlobal: global_copy(&hdrop_bytes(path))?,
        },
        pUnkForRelease: ManuallyDrop::new(None),
    })
}

/// Checks a `FORMATETC` describes the `TYMED_HGLOBAL` side of `CF_HDROP`.
///
/// # Safety
/// `format` must be null or point to a readable `FORMATETC`.
unsafe fn is_cf_hdrop(format: *const FORMATETC) -> Result<(), HRESULT> {
    if format.is_null() {
        return Err(DV_E_FORMATETC);
    }
    let format = unsafe { &*format };
    if format.cfFormat != CF_HDROP.0 {
        return Err(DV_E_FORMATETC);
    }
    if format.tymed & (TYMED_HGLOBAL.0 as u32) == 0 {
        return Err(DV_E_TYMED);
    }
    Ok(())
}

/// One-file `IDataObject` used when the shell refuses to provide one.
///
/// It only implements `CF_HDROP`, which is what file drop targets ask for. Format enumeration is
/// deliberately `E_NOTIMPL`: Explorer and other file targets call `QueryGetData`/`GetData`
/// directly for `CF_HDROP`, and implementing `IEnumFORMATETC` would add surface for no P0 gain.
#[repr(C)]
struct FileDataObjectObj {
    header: ComHeader,
    path: PathBuf,
}

static DATA_OBJECT_VTBL: IDataObject_Vtbl = IDataObject_Vtbl {
    base__: windows::core::IUnknown_Vtbl {
        QueryInterface: data_object_query_interface,
        AddRef: data_object_add_ref,
        Release: data_object_release,
    },
    GetData: data_object_get_data,
    GetDataHere: data_object_get_data_here,
    QueryGetData: data_object_query_get_data,
    GetCanonicalFormatEtc: data_object_get_canonical_format_etc,
    SetData: data_object_set_data,
    EnumFormatEtc: data_object_enum_format_etc,
    DAdvise: data_object_d_advise,
    DUnadvise: data_object_d_unadvise,
    EnumDAdvise: data_object_enum_d_advise,
};

/// Creates the hand-written `CF_HDROP` data object for `path`.
fn file_data_object(path: &Path) -> Result<IDataObject, String> {
    let object = Box::new(FileDataObjectObj {
        header: ComHeader {
            vtbl: &DATA_OBJECT_VTBL as *const IDataObject_Vtbl as *const c_void,
            refs: AtomicU32::new(1),
        },
        path: path.to_path_buf(),
    });
    let raw = Box::into_raw(object) as *mut c_void;
    Ok(unsafe { wrap_interface::<IDataObject>(raw) })
}

unsafe extern "system" fn data_object_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    const SUPPORTED: [GUID; 2] = [
        <IUnknown as Interface>::IID,
        <IDataObject as Interface>::IID,
    ];
    unsafe { query_interface(this, iid, out, &SUPPORTED) }
}

unsafe extern "system" fn data_object_add_ref(this: *mut c_void) -> u32 {
    unsafe { add_ref(this) }
}

unsafe extern "system" fn data_object_release(this: *mut c_void) -> u32 {
    unsafe { release::<FileDataObjectObj>(this) }
}

unsafe extern "system" fn data_object_get_data(
    this: *mut c_void,
    format: *const FORMATETC,
    out: *mut STGMEDIUM,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let object = unsafe { &*(this as *const FileDataObjectObj) };
    if let Err(hr) = unsafe { is_cf_hdrop(format) } {
        return hr;
    }
    match hdrop_medium(&object.path) {
        Ok(medium) => {
            unsafe { *out = medium };
            S_OK
        }
        Err(err) => {
            log::warn!("dragout: failed to allocate CF_HDROP medium: {err}");
            E_OUTOFMEMORY
        }
    }
}

unsafe extern "system" fn data_object_get_data_here(
    _this: *mut c_void,
    _format: *const FORMATETC,
    _out: *mut STGMEDIUM,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn data_object_query_get_data(
    _this: *mut c_void,
    format: *const FORMATETC,
) -> HRESULT {
    match unsafe { is_cf_hdrop(format) } {
        Ok(()) => S_OK,
        Err(hr) => hr,
    }
}

unsafe extern "system" fn data_object_get_canonical_format_etc(
    _this: *mut c_void,
    _format: *const FORMATETC,
    _out: *mut FORMATETC,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn data_object_set_data(
    _this: *mut c_void,
    _format: *const FORMATETC,
    _medium: *const STGMEDIUM,
    _release: BOOL,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn data_object_enum_format_etc(
    _this: *mut c_void,
    _direction: u32,
    _out: *mut *mut c_void,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn data_object_d_advise(
    _this: *mut c_void,
    _format: *const FORMATETC,
    _flags: u32,
    _sink: *mut c_void,
    _connection: *mut u32,
) -> HRESULT {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn data_object_d_unadvise(_this: *mut c_void, _connection: u32) -> HRESULT {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn data_object_enum_d_advise(
    _this: *mut c_void,
    _out: *mut *mut c_void,
) -> HRESULT {
    OLE_E_ADVISENOTSUPPORTED
}

// ---------------------------------------------------------------------------------------------
// Data object selection + DoDragDrop
// ---------------------------------------------------------------------------------------------

/// Creates the shell's `IDataObject` for `path` (`SHCreateItemFromParsingName` +
/// `IShellItem::BindToHandler(BHID_DataObject)`).
///
/// This is the preferred data object: the shell builds the `CF_HDROP` and the file's drag
/// metadata itself, exactly as it does for Explorer's own drags.
fn shell_data_object(path: &Path) -> windows::core::Result<IDataObject> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR::from_raw(wide.as_ptr()), None)?;
        item.BindToHandler(None, &BHID_DataObject)
    }
}

/// Returns the data object handed to `DoDragDrop`, preferring the shell's.
fn create_data_object(path: &Path) -> Result<IDataObject, String> {
    match shell_data_object(path) {
        Ok(data) => {
            log::debug!("dragout: using the shell IDataObject (CF_HDROP via BHID_DataObject)");
            Ok(data)
        }
        Err(err) => {
            log::warn!(
                "dragout: shell data object unavailable ({err}); using the built-in CF_HDROP \
                 data object"
            );
            file_data_object(path)
        }
    }
}

/// Runs `DoDragDrop` with the given data object/source. The caller must have initialised OLE
/// on this thread; [`allowed_effects`] lets a target copy (apps), move (folder/desktop) or delete
/// (Recycle Bin) the file, and the final effect is reported in [`DragEnded`].
fn run_drag(data: &IDataObject, source: &IDropSource) -> Result<DragEnded, String> {
    let mut effect = DROPEFFECT_NONE;
    let hr = unsafe { DoDragDrop(data, source, allowed_effects(), &mut effect) };
    if hr == DRAGDROP_S_DROP {
        Ok(DragEnded::finished(effect, true))
    } else if hr == DRAGDROP_S_CANCEL {
        Ok(DragEnded::finished(DROPEFFECT_NONE, false))
    } else {
        Err(format!("DoDragDrop failed: 0x{:08X}", hr.0 as u32))
    }
}

/// Creates the data object and runs the drag on the current thread (which must be an STA with
/// OLE initialised). Kept separate from [`drag_file`] so tests can inject a drop source.
fn run_drag_session(path: &Path, source: &IDropSource) -> Result<DragEnded, String> {
    let data = create_data_object(path)?;
    run_drag(&data, source)
}

/// Drags `path` out of the app with OLE; returns only when the OS drag loop finishes.
///
/// Must be called on the dedicated drag thread: `DoDragDrop` runs its own modal message loop. The
/// watchdog observes the session through `watchdog` (first `QueryContinueDrag` callback / cancel
/// request), so a loop that never receives input can be broken from the outside.
fn drag_file(path: &Path, watchdog: &Arc<DragWatchdog>) -> Result<DragEnded, String> {
    // `_ole` is declared first so it is uninitialised last: the data object and the drop source
    // are released before `OleUninitialize`, as COM requires.
    let _ole = OleApartment::init()?;
    let source = DropSource::interactive(watchdog)?;
    let outcome = run_drag_session(path, &source.interface)?;
    log::debug!(
        "dragout: OLE called QueryContinueDrag {} times and GiveFeedback {} times",
        source.counters.query_continue_drag.load(Ordering::Relaxed),
        source.counters.give_feedback.load(Ordering::Relaxed)
    );
    Ok(outcome)
}

/// Starts a native file drag-out for `path`.
///
/// Returns as soon as the drag session is accepted; the OLE loop itself runs on the dedicated
/// [`DRAG_THREAD_NAME`] thread, which keeps the Tauri IPC thread (and the app's event loop)
/// responsive for the whole drag. Completion, including cancellation and failure, is reported to
/// the panel through the [`DRAG_ENDED_EVENT`] event ([`DragEnded`] payload) exactly once per
/// session.
#[tauri::command]
pub fn start_drag(window: WebviewWindow, path: String) -> Result<(), String> {
    let file = validate_path(&path)?;
    let session_id = next_session_id();
    // Set before the thread starts so the hit-test loop cannot flip click-through in the gap
    // between this command returning and `DoDragDrop` taking over the mouse.
    let session = DragSession::begin(session_id)?;
    let app = window.app_handle().clone();

    log::info!(
        "dragout: OLE drag start requested window={} path={} session={session_id} (ipc thread {:?})",
        window.label(),
        file.display(),
        thread::current().id()
    );

    let spawned = thread::Builder::new()
        .name(DRAG_THREAD_NAME.to_owned())
        .spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
            let watchdog = Arc::new(DragWatchdog::default());
            let watchdog_thread = match spawn_drag_watchdog(
                app.clone(),
                session_id,
                thread_id,
                watchdog.clone(),
                DRAG_WATCHDOG,
                DRAG_WATCHDOG_GRACE,
            ) {
                Ok(handle) => Some(handle),
                Err(err) => {
                    log::warn!("dragout: failed to start the drag watchdog: {err}");
                    None
                }
            };

            let started = Instant::now();
            // A panic in the drag engine must still produce `drag-ended`; otherwise the session
            // flag (and the panel's pressed state) would stay set until the next pointerdown.
            // `AssertUnwindSafe` is sound here: the drag thread ends after this closure, and the
            // COM objects are owned by the unwound frames (their drop guards run during unwind).
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                drag_file(&file, &watchdog)
            }));
            let ended = match outcome {
                Ok(Ok(ended)) => {
                    if watchdog.cancel_requested() {
                        log::warn!(
                            "dragout: drag loop returned after the watchdog cancel request \
                             (session {session_id})"
                        );
                        DragEnded::failed(WATCHDOG_ABORT_ERROR.to_owned())
                    } else {
                        ended
                    }
                }
                Ok(Err(err)) => {
                    log::error!("dragout: drag failed: {err}");
                    DragEnded::failed(err)
                }
                Err(payload) => {
                    let message = panic_message(payload.as_ref());
                    log::error!("dragout: drag thread panicked: {message}");
                    DragEnded::failed(format!("drag thread panicked: {message}"))
                }
            }
            .with_session(session_id);

            // Tell the watchdog first: a normal drag must not be mistaken for a stuck one while
            // the guard/event are being cleaned up.
            watchdog.mark_finished();
            // Nothing owns the mouse any more: let the hit-test loop manage click-through again.
            drop(session);
            let elapsed = started.elapsed().as_millis();
            log::info!(
                "dragout: OLE drag ended after {elapsed} ms: session={} ok={} dropped={} \
                 effect={} error={:?} (drag thread {:?})",
                ended.session_id,
                ended.ok,
                ended.dropped,
                ended.effect,
                ended.error,
                thread::current().id()
            );
            emit_drag_ended(&app, &watchdog, ended);
            // The watchdog exits on `mark_finished`; join so a normal drag leaves no parked thread.
            if let Some(handle) = watchdog_thread {
                if handle.join().is_err() {
                    log::warn!("dragout: drag watchdog thread panicked");
                }
            }
        });

    match spawned {
        Ok(_) => Ok(()),
        // The closure (and with it the session guard) is dropped on failure, clearing the flag.
        Err(err) => Err(format!("failed to start the OLE drag thread: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    use windows::Win32::System::Com::DVASPECT_CONTENT;
    use windows::Win32::System::Ole::ReleaseStgMedium;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    /// Test scratch dir under `target/` (ignored by git).
    ///
    /// `%TEMP%` is not writable from an executable built inside the workspace in the agent
    /// sandbox, so unit tests stay inside the cargo target directory.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("dragout-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn hdrop_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    /// Reads the single path out of an `HGLOBAL` that carries `CF_HDROP` (the same call an
    /// Explorer drop target makes in its `Drop` implementation).
    ///
    /// # Safety
    /// `medium` must be a `TYMED_HGLOBAL` medium containing a `DROPFILES` block.
    unsafe fn hdrop_path(medium: &STGMEDIUM) -> PathBuf {
        let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);
        let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
        assert_eq!(count, 1, "CF_HDROP must contain exactly one file");
        let mut buffer = [0u16; 1024];
        let len = unsafe { DragQueryFileW(hdrop, 0, Some(&mut buffer)) } as usize;
        assert!(len > 0, "DragQueryFileW must report the path length");
        PathBuf::from(String::from_utf16_lossy(&buffer[..len]))
    }

    #[test]
    fn accepts_an_existing_absolute_file() {
        let dir = temp_dir("accept");
        let file = dir.join("shot.png");
        std::fs::write(&file, b"png").expect("write file");

        assert_eq!(validate_path(&file.to_string_lossy()), Ok(file.clone()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_relative_missing_and_directory_paths() {
        let dir = temp_dir("reject");
        let missing = dir.join("missing.png");
        let missing_str = missing.to_string_lossy().into_owned();

        assert!(validate_path("relative.png")
            .expect_err("relative path must be rejected")
            .contains("absolute"));
        assert!(validate_path(&missing_str)
            .expect_err("missing file must be rejected")
            .contains("not accessible"));
        assert!(validate_path(&dir.to_string_lossy())
            .expect_err("directory must be rejected")
            .contains("not a file"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn drag_session_allows_one_drag_at_a_time_and_ignores_stale_guards() {
        assert!(!is_dragging());
        let first = DragSession::begin(1).expect("first session");
        assert!(is_dragging(), "flag must be set for the drag duration");
        assert!(
            DragSession::begin(2).is_err(),
            "a concurrent drag must be rejected instead of fighting for the mouse"
        );

        // The watchdog force-clears a session whose thread never returns; a new drag may start.
        assert!(
            force_end_session(1),
            "the stuck session must still own the flag when it is force-cleared"
        );
        let second = DragSession::begin(3).expect("session after the force clear");
        assert!(is_dragging());

        // When the stuck thread finally unwinds, its stale guard must not clear the new session.
        drop(first);
        assert!(
            is_dragging(),
            "a stale drag guard must not clear a newer session's flag"
        );
        drop(second);
        assert!(!is_dragging(), "the owning guard clears the flag");

        // Force-clearing an unknown (or already ended) session is a no-op.
        assert!(!force_end_session(3));
        assert!(!is_dragging());
    }

    #[test]
    fn watchdog_cancels_only_when_no_input_arrives_before_the_timeout() {
        let timeout = DRAG_WATCHDOG;
        assert_eq!(
            watchdog_action(false, Duration::ZERO, timeout),
            WatchdogAction::KeepWaiting,
            "a freshly started drag is not stuck yet"
        );
        assert_eq!(
            watchdog_action(false, timeout - Duration::from_millis(1), timeout),
            WatchdogAction::KeepWaiting
        );
        assert_eq!(
            watchdog_action(false, timeout, timeout),
            WatchdogAction::Cancel,
            "no input within the timeout means the modal loop is stuck"
        );
        assert_eq!(
            watchdog_action(true, timeout * 10, timeout),
            WatchdogAction::Stop,
            "a drag that already called back is a normal (possibly long) drag"
        );
        assert_eq!(
            watchdog_action(true, Duration::ZERO, timeout),
            WatchdogAction::Stop
        );
    }

    #[test]
    fn watchdog_wait_stands_down_on_input_and_cancels_without_it() {
        // No input at all: the wait must time out and ask for a cancel.
        let quiet = DragWatchdog::default();
        let started = Instant::now();
        assert_eq!(
            wait_for_drag_progress(&quiet, Duration::from_millis(120)),
            WatchdogAction::Cancel
        );
        assert!(
            started.elapsed() >= Duration::from_millis(120),
            "the wait must actually last the timeout"
        );

        // One callback is enough: OLE is pumping input, so a long drag is legitimate.
        let active = Arc::new(DragWatchdog::default());
        let feeder = {
            let active = active.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(30));
                active.note_input();
            })
        };
        assert_eq!(
            wait_for_drag_progress(&active, Duration::from_secs(5)),
            WatchdogAction::Stop
        );
        feeder.join().expect("input feeder thread");
    }

    #[test]
    fn drop_effect_names_cover_every_ole_bit() {
        assert_eq!(effect_name(DROPEFFECT_COPY), "copy");
        assert_eq!(effect_name(DROPEFFECT_MOVE), "move");
        assert_eq!(effect_name(DROPEFFECT_LINK), "link");
        assert_eq!(effect_name(DROPEFFECT_NONE), "none");
    }

    #[test]
    fn drag_effects_offer_copy_and_move_but_never_link() {
        let effects = allowed_effects();
        assert!(
            effects.contains(DROPEFFECT_COPY),
            "a drop on an app must be able to copy"
        );
        assert!(
            effects.contains(DROPEFFECT_MOVE),
            "Explorer moves the file when a folder/desktop/Recycle Bin is the target"
        );
        assert!(
            !effects.contains(DROPEFFECT_LINK),
            "a shell shortcut is not a supported outcome"
        );
    }

    /// Task 13 decision table: given the final effect and whether the source file still exists,
    /// should the post-drag command prune, keep polling (slow shell move) or leave the card alone?
    #[test]
    fn after_drag_action_follows_effect_and_existence() {
        use AfterDragAction::{Done, KeepWaiting, Prune};

        let cases = [
            // A folder/desktop/Recycle Bin MOVE may still be settling.
            (DragEffect::Move, true, KeepWaiting),
            (DragEffect::Move, false, Prune),
            // A COPY target keeps the source; a vanished source is a safety prune.
            (DragEffect::Copy, true, Done),
            (DragEffect::Copy, false, Prune),
            // LINK is never offered, but a vanished source must still prune.
            (DragEffect::Link, true, Done),
            (DragEffect::Link, false, Prune),
            // Cancel/no-target: nothing to do unless the file is gone (safety net).
            (DragEffect::None, true, Done),
            (DragEffect::None, false, Prune),
        ];
        for (effect, file_exists, expected) in cases {
            assert_eq!(
                after_drag_action(effect, file_exists),
                expected,
                "effect={effect:?} file_exists={file_exists}"
            );
        }
    }

    #[test]
    fn drag_effect_names_round_trip_and_unknown_names_are_rejected() {
        for effect in [
            DragEffect::Copy,
            DragEffect::Move,
            DragEffect::Link,
            DragEffect::None,
        ] {
            assert_eq!(DragEffect::from_name(effect.as_str()), Some(effect));
        }
        assert_eq!(DragEffect::from_name("bogus"), None);
        assert_eq!(DragEffect::from_name(""), None);
    }

    #[test]
    fn query_continue_cancels_on_escape_and_drops_on_release() {
        assert_eq!(
            query_continue_decision(true, MK_LBUTTON),
            DRAGDROP_S_CANCEL,
            "Escape must cancel even while the button is down"
        );
        assert_eq!(
            query_continue_decision(false, MK_LBUTTON),
            S_OK,
            "a held left button must keep the drag going"
        );
        assert_eq!(
            query_continue_decision(false, MODIFIERKEYS_FLAGS(0)),
            DRAGDROP_S_DROP,
            "a released left button must complete the drop"
        );
    }

    #[test]
    fn hdrop_blob_is_a_wide_double_nul_terminated_path_list() {
        let path = Path::new("C:\\shots\\hangline.png");
        let bytes = hdrop_bytes(path);
        let units_expected = path.as_os_str().encode_wide().count() + 2;

        // DROPFILES { pFiles: u32, pt: POINT, fNC: BOOL, fWide: BOOL } == 20 bytes.
        assert_eq!(bytes.len(), 20 + units_expected * 2);
        // DROPFILES is a packed struct: copy the fields out instead of taking references.
        let header = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const DROPFILES) };
        let (p_files, f_nc, f_wide) = (header.pFiles, header.fNC, header.fWide);
        assert_eq!(p_files, 20, "pFiles must point past DROPFILES");
        assert_eq!(f_nc, BOOL::from(false));
        assert_eq!(f_wide, BOOL::from(true), "paths must be UTF-16");

        let units: Vec<u16> = bytes[20..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(&units[units.len() - 2..], &[0, 0], "double NUL terminator");
        assert_eq!(
            String::from_utf16_lossy(&units[..units.len() - 2]),
            "C:\\shots\\hangline.png"
        );
    }

    #[test]
    fn builtin_data_object_serves_cf_hdrop() {
        let dir = temp_dir("hdrop-builtin");
        let file = dir.join("shot.png");
        std::fs::write(&file, b"png").expect("write file");

        let data = file_data_object(&file).expect("built-in data object");
        let format = hdrop_format();
        assert_eq!(unsafe { data.QueryGetData(&format) }, S_OK);

        let mut medium = unsafe { data.GetData(&format) }.expect("GetData(CF_HDROP)");
        assert_eq!(unsafe { hdrop_path(&medium) }, file);
        unsafe { ReleaseStgMedium(&mut medium) };

        // Other formats are rejected, not silently served as a file list.
        let text = FORMATETC {
            cfFormat: 13, // CF_UNICODETEXT
            ..format
        };
        assert_eq!(unsafe { data.QueryGetData(&text) }, DV_E_FORMATETC);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shell_data_object_serves_cf_hdrop() {
        let dir = temp_dir("hdrop-shell");
        let file = dir.join("shot.png");
        std::fs::write(&file, b"png").expect("write file");

        let _ole = OleApartment::init().expect("OleInitialize");
        let data = shell_data_object(&file).expect("shell IDataObject");
        let format = hdrop_format();
        assert_eq!(
            unsafe { data.QueryGetData(&format) },
            S_OK,
            "the shell object for a real file must offer CF_HDROP"
        );

        let mut medium = unsafe { data.GetData(&format) }.expect("GetData(CF_HDROP)");
        assert_eq!(
            unsafe { hdrop_path(&medium) },
            file,
            "the shell object must hand over the exact source path"
        );
        unsafe { ReleaseStgMedium(&mut medium) };
        drop(data);
        drop(_ole);

        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------------------------------------------------------------------------------------
    // Evidence harness: these tests open a real OLE drag loop. `DoDragDrop`'s modal loop only
    // calls `IDropSource` while it receives input, so the tests post `WM_MOUSEMOVE` to the drag
    // thread — the same message a real drag receives from the physical mouse (on a locked
    // desktop OS input cannot reach the process). They are #[ignore]d and run explicitly with
    // `cargo test --lib -- --ignored --test-threads=1`. See docs/experiments/P0-E4.md.
    // ---------------------------------------------------------------------------------------

    /// Outcome plus the source's `QueryContinueDrag`/`GiveFeedback` call counts.
    type DragWorkerResult = (DragEnded, usize, usize);

    struct DragWorker {
        thread_id: mpsc::Receiver<u32>,
        outcome: mpsc::Receiver<Result<DragWorkerResult, String>>,
        handle: thread::JoinHandle<()>,
    }

    /// Runs one OLE drag on a dedicated STA thread (the same thread model the app uses) and
    /// exposes the thread id so the test can feed the drag loop an input message.
    fn drag_on_worker(file: PathBuf, behavior: DropBehavior) -> DragWorker {
        let (id_tx, thread_id) = mpsc::channel();
        let (outcome_tx, outcome) = mpsc::channel();
        let handle = thread::spawn(move || {
            let result = (|| -> Result<DragWorkerResult, String> {
                let _ole = OleApartment::init()?;
                let source = DropSource::new(behavior, None)?;
                let _ = id_tx.send(unsafe { GetCurrentThreadId() });
                let ended = run_drag_session(&file, &source.interface)?;
                Ok((
                    ended,
                    source.counters.query_continue_drag.load(Ordering::Relaxed),
                    source.counters.give_feedback.load(Ordering::Relaxed),
                ))
            })();
            let _ = outcome_tx.send(result);
        });
        DragWorker {
            thread_id,
            outcome,
            handle,
        }
    }

    /// Posts the mouse message OLE's drag loop consumes. `left_button_down` becomes the
    /// `grfKeyState` that reaches `IDropSource::QueryContinueDrag`.
    fn post_mouse_move(thread_id: u32, left_button_down: bool) {
        let keys = if left_button_down { MK_LBUTTON.0 } else { 0 };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let posted = unsafe {
                PostThreadMessageW(thread_id, WM_MOUSEMOVE, WPARAM(keys as usize), LPARAM(0))
            };
            if posted.is_ok() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the drag thread never created a message queue: {posted:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    #[ignore = "opens a real OLE drag loop; needs an interactive desktop (see P0-E4 evidence)"]
    fn ole_drag_loop_reports_drop_cancel_and_keeps_the_source() {
        let dir = temp_dir("ole-drag");
        let file = dir.join("source.png");
        let contents = b"hangline-e4-source-bytes";
        std::fs::write(&file, contents).expect("write source file");

        // --- DRAGDROP_S_DROP ---------------------------------------------------------------
        let worker = drag_on_worker(file.clone(), DropBehavior::Fixed(DRAGDROP_S_DROP));
        let tid = worker
            .thread_id
            .recv_timeout(Duration::from_secs(5))
            .expect("drag thread id");
        thread::sleep(Duration::from_millis(100));
        post_mouse_move(tid, true);
        let (dropped, queries, feedbacks) = worker
            .outcome
            .recv_timeout(Duration::from_secs(15))
            .expect("DoDragDrop did not return; its modal loop is stuck")
            .expect("drop-path drag");
        worker.handle.join().expect("drag thread");
        assert!(dropped.dropped, "expected DRAGDROP_S_DROP, got {dropped:?}");
        assert!(
            matches!(dropped.effect, "none" | "copy" | "move" | "link"),
            "unexpected effect: {dropped:?}"
        );
        assert!(queries >= 1, "OLE must call IDropSource::QueryContinueDrag");
        assert!(
            feedbacks >= 1,
            "OLE must call IDropSource::GiveFeedback"
        );
        assert_eq!(
            std::fs::read(&file).expect("read source after drop"),
            contents,
            "the drag source must not modify the source file"
        );

        // --- DRAGDROP_S_CANCEL -------------------------------------------------------------
        let worker = drag_on_worker(file.clone(), DropBehavior::Fixed(DRAGDROP_S_CANCEL));
        let tid = worker
            .thread_id
            .recv_timeout(Duration::from_secs(5))
            .expect("drag thread id");
        thread::sleep(Duration::from_millis(100));
        post_mouse_move(tid, true);
        let (cancelled, queries, _feedbacks) = worker
            .outcome
            .recv_timeout(Duration::from_secs(15))
            .expect("DoDragDrop did not return on cancel")
            .expect("cancel-path drag");
        worker.handle.join().expect("drag thread");
        assert!(
            !cancelled.dropped,
            "expected DRAGDROP_S_CANCEL, got {cancelled:?}"
        );
        assert_eq!(cancelled.effect, "none");
        assert!(
            cancelled.error.is_none(),
            "cancel is not an error: {cancelled:?}"
        );
        assert!(queries >= 1, "OLE must call IDropSource::QueryContinueDrag");
        assert_eq!(
            std::fs::read(&file).expect("read source after cancel"),
            contents,
            "a cancelled drag must leave the source file untouched"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[ignore = "opens a real OLE drag loop; needs an interactive desktop"]
    fn interactive_source_continues_then_drops_when_the_button_is_released() {
        let dir = temp_dir("ole-prod");
        let file = dir.join("source.png");
        std::fs::write(&file, b"prod").expect("write source file");

        let (id_tx, id_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let path = file.clone();
        let handle = thread::spawn(move || {
            let watchdog = Arc::new(DragWatchdog::default());
            let _ = id_tx.send(unsafe { GetCurrentThreadId() });
            let _ = tx.send(drag_file(&path, &watchdog));
        });
        let tid = id_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("drag thread id");
        thread::sleep(Duration::from_millis(100));

        // Button held: the production source must answer S_OK and keep the modal loop alive.
        post_mouse_move(tid, true);
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "the drag must not end while the left button is held"
        );

        // Button released: the production source must complete the drop.
        post_mouse_move(tid, false);
        let outcome = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("drag_file did not return after the release message")
            .expect("drag_file");
        handle.join().expect("drag thread");
        assert!(outcome.dropped, "released button means drop: {outcome:?}");
        assert_eq!(
            outcome.effect, "none",
            "no OLE drop target on the locked desktop"
        );
        assert!(
            std::fs::metadata(&file).expect("source metadata").len() > 0,
            "source file must still exist"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Regression test for the stuck-modal-loop finding: a real `DoDragDrop` receives **no**
    /// input at all, the watchdog decides to cancel, and its messages must break the loop so the
    /// session can be cleared. Only Win32 messages are used (no physical input), so it also runs
    /// on a locked desktop like the other evidence tests.
    #[test]
    #[ignore = "runs a real OLE modal loop and posts Win32 messages; see docs/experiments/P0-E4.md §10"]
    fn watchdog_cancels_a_drag_loop_that_never_receives_input() {
        let dir = temp_dir("watchdog");
        let file = dir.join("source.png");
        std::fs::write(&file, b"watchdog source").expect("write source file");

        let session_id = next_session_id();
        let session = DragSession::begin(session_id).expect("drag session");
        let state = Arc::new(DragWatchdog::default());
        let (id_tx, id_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let path = file.clone();
        let drag_state = state.clone();
        let handle = thread::spawn(move || {
            let result = (|| -> Result<DragEnded, String> {
                let _ole = OleApartment::init()?;
                let source = DropSource::interactive(&drag_state)?;
                let _ = id_tx.send(unsafe { GetCurrentThreadId() });
                // Give the main thread time to reach `wait_for_drag_progress` before `DoDragDrop`
                // starts pumping: the watchdog's clock must cover the modal loop, not thread start.
                thread::sleep(Duration::from_millis(150));
                run_drag_session(&path, &source.interface)
            })();
            drag_state.mark_finished();
            let _ = done_tx.send(());
            result
        });
        let thread_id = id_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("drag thread id");

        // No input is ever posted: the watchdog must treat the loop as stuck.
        assert_eq!(
            wait_for_drag_progress(&state, Duration::from_millis(400)),
            WatchdogAction::Cancel,
            "a drag loop without any input must be treated as stuck"
        );
        let forced = cancel_stuck_drag(
            &state,
            thread_id,
            Duration::from_millis(400),
            Duration::from_secs(5),
        );
        assert!(
            !forced,
            "the modal loop must return after the watchdog cancel messages"
        );

        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("DoDragDrop never returned after the watchdog cancel");
        let ended = handle
            .join()
            .expect("drag thread")
            .expect("drag session result");
        assert!(
            !ended.dropped,
            "a watchdog cancel must not report a drop: {ended:?}"
        );
        assert_eq!(ended.effect, "none");
        assert!(
            state.cancel_requested(),
            "the watchdog must have requested the cancel"
        );

        // Production path: the drag thread drops the guard when the loop returns.
        drop(session);
        assert!(
            !is_dragging(),
            "the drag session must be cleared after the watchdog abort"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
