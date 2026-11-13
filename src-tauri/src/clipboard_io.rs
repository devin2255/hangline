//! Clipboard writing for card copy (design spec §3.1 / §4.7).
//!
//! Image cards are published as `"PNG"` (preferred by image-aware consumers), `CF_DIBV5` and
//! `CF_HDROP`; recording cards only publish `CF_HDROP`. The format decision table is
//! [`formats_for`]. `CF_UNICODETEXT` is deliberately not written (spec §4.7): a plain-text paste of
//! a copied screenshot should not silently happen.
//!
//! # Failure safety and verification
//!
//! Every payload is decoded, encoded and copied into its `GMEM_MOVEABLE` `HGLOBAL` *before* the
//! clipboard is opened and emptied. A decode/encode/allocation failure therefore leaves the
//! previous clipboard content untouched. After `EmptyClipboard`, `SetClipboardData` transfers each
//! block to the system; a failed set frees the block through [`OwnedHGlobal`]'s `Drop`.
//!
//! Windows does not guarantee that a successful `SetClipboardData` is visible to consumers, and
//! `OpenClipboard(NULL)` is known to fail intermittently with `ERROR_CLIPBOARD_NOT_OPEN`. The writer
//! therefore opens the clipboard with the panel window as its owner and verifies every expected
//! format with `IsClipboardFormatAvailable` before reporting success. A partial write is retried as
//! a whole (initial attempt plus two retries); if a format is still missing the copy returns an
//! error, so the frontend never shows the `Copied` pill for a partial write.
//!
//! # DIB orientation
//!
//! The `CF_DIBV5` payload is a top-down bitmap: `bV5Height` is negative and the rows are stored in
//! display order. That is the Task 12 decision ("BGRA top-down"); the plan's older "bottom-up"
//! wording is equivalent on the wire once the sign is respected by consumers. Alpha is forced to
//! 255 because screenshot pixels are opaque; the PNG flavour keeps the real alpha.

use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context};
use image::{ImageFormat, RgbaImage};
use windows::core::{w, BOOL, HRESULT};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, POINT, E_OUTOFMEMORY};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIBV5, CF_HDROP};
use windows::Win32::UI::Shell::DROPFILES;

use crate::item::ItemKind;

/// `BITMAPV5HEADER::bV5Size` (the fixed 124-byte Windows structure).
const DIBV5_HEADER_SIZE: usize = 124;

/// `BI_BITFIELDS` compression: the masks below describe the BGRA layout.
const BI_BITFIELDS: u32 = 3;

/// `LCS_sRGB` as the four-character code 'sRGB' (`Win32_UI_ColorSystem` is not enabled).
const LCS_SRGB: u32 = 0x7352_4742;

/// `LCS_GM_IMAGES`: the sRGB endpoint/gamma values are ignored and images are the intent.
const LCS_GM_IMAGES: u32 = 4;

/// `CF_DIBV5` as a `u32` clipboard format id.
const CF_DIBV5_ID: u32 = CF_DIBV5.0 as u32;

/// `CF_HDROP` as a `u32` clipboard format id.
const CF_HDROP_ID: u32 = CF_HDROP.0 as u32;

/// Number of `OpenClipboard` attempts before a copy gives up (another app may hold it briefly).
const OPEN_ATTEMPTS: usize = 5;

/// Delay between `OpenClipboard` attempts.
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(15);

/// Whole write sequences tried after `EmptyClipboard` (initial attempt + two retries).
const WRITE_ATTEMPTS: usize = 3;

/// Delay before retrying a whole write sequence after a partial or retryable failure.
const WRITE_RETRY_DELAY: Duration = Duration::from_millis(30);

/// `ERROR_CLIPBOARD_NOT_OPEN` as the `HRESULT` Windows reports through the `windows` crate.
///
/// `HRESULT_FROM_WIN32(1418)` is `0x8007058A`; the `windows` crate exposes the Win32 error code
/// through this HRESULT, not as a plain `u32`, when a clipboard call fails.
const ERROR_CLIPBOARD_NOT_OPEN: HRESULT = HRESULT::from_win32(1418);

/// One clipboard representation Hangline can publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipFormat {
    /// Registered `"PNG"` format: lossless, self-describing image bytes.
    Png,
    /// `CF_DIBV5`: `BITMAPV5HEADER` + top-down 32 bpp BGRA pixels.
    DibV5,
    /// `CF_HDROP`: the path as a file drop, so Explorer/chat apps can accept the file itself.
    FileDrop,
}

/// Returns the clipboard formats to publish for `kind` (decision table, unit-tested).
///
/// The order is the write order: the PNG flavour first, because consumers that understand it
/// should prefer it over the DIB; `CF_HDROP` last (spec §4.7). Recordings are file-only.
pub fn formats_for(kind: ItemKind) -> Vec<ClipFormat> {
    match kind {
        ItemKind::Recording => vec![ClipFormat::FileDrop],
        ItemKind::Capture
        | ItemKind::SystemScreenshot
        | ItemKind::Clipboard
        | ItemKind::WatchedFolder
        | ItemKind::Dropped => {
            vec![ClipFormat::Png, ClipFormat::DibV5, ClipFormat::FileDrop]
        }
    }
}

/// The formats a successful copy of `kind` must leave available on the clipboard.
///
/// This is the verification contract for the copy path: it mirrors [`formats_for`] and is asserted
/// by unit tests, so the writer and the verifier cannot drift apart silently.
fn expected_formats(kind: ItemKind) -> Vec<ClipFormat> {
    formats_for(kind)
}

/// Publishes the image formats (`"PNG"` + `CF_DIBV5`) plus `CF_HDROP` for `path`.
///
/// `owner` is the panel window used as the clipboard owner; `None` is only a logged fallback when
/// the window handle cannot be resolved (the NULL-owner path is the flaky one this fix avoids).
pub fn copy_image(path: &Path, owner: Option<HWND>) -> anyhow::Result<()> {
    copy_with_formats(
        path,
        &[ClipFormat::Png, ClipFormat::DibV5, ClipFormat::FileDrop],
        owner,
    )
}

/// Publishes only `CF_HDROP` for `path` (recordings, spec §4.7).
pub fn copy_file(path: &Path, owner: Option<HWND>) -> anyhow::Result<()> {
    copy_with_formats(path, &[ClipFormat::FileDrop], owner)
}

/// Publishes the formats selected by [`expected_formats`] for one item.
pub fn copy_for_kind(kind: ItemKind, path: &Path, owner: Option<HWND>) -> anyhow::Result<()> {
    if expected_formats(kind).contains(&ClipFormat::Png) {
        copy_image(path, owner)
    } else {
        copy_file(path, owner)
    }
}

/// One fully encoded clipboard representation, ready to be published.
struct ClipboardPayload {
    /// Which representation this is (also the key of the verification contract).
    format: ClipFormat,
    /// The numeric Windows clipboard format id (`CF_*` or a registered format).
    id: u32,
    /// The bytes handed to `SetClipboardData` (copied into a `GMEM_MOVEABLE` block per attempt).
    bytes: Vec<u8>,
}

/// Builds every payload (before opening the clipboard) and writes it in `formats` order.
fn copy_with_formats(
    path: &Path,
    formats: &[ClipFormat],
    owner: Option<HWND>,
) -> anyhow::Result<()> {
    if !path.is_absolute() {
        bail!("cannot copy a relative path: {}", path.display());
    }
    if !path.is_file() {
        bail!("cannot copy a missing file: {}", path.display());
    }
    let needs_image = formats
        .iter()
        .any(|format| matches!(format, ClipFormat::Png | ClipFormat::DibV5));
    let image = if needs_image {
        Some(load_rgba(path)?)
    } else {
        None
    };

    let mut payloads: Vec<ClipboardPayload> = Vec::with_capacity(formats.len());
    for format in formats {
        let (id, bytes) = match format {
            ClipFormat::Png => {
                let png = png_bytes(image.as_ref().expect("image was decoded"))?;
                (register_png_format()?, png)
            }
            ClipFormat::DibV5 => (
                CF_DIBV5_ID,
                dibv5_bytes(image.as_ref().expect("image was decoded")),
            ),
            ClipFormat::FileDrop => (CF_HDROP_ID, hdrop_bytes(path)),
        };
        payloads.push(ClipboardPayload {
            format: *format,
            id,
            bytes,
        });
    }

    log::info!(
        "clipboard: copying {} as {:?} ({} payload(s), {} byte(s), owner {owner:?})",
        path.display(),
        formats,
        payloads.len(),
        payloads
            .iter()
            .map(|payload| payload.bytes.len())
            .sum::<usize>()
    );
    write_payloads(&payloads, owner)
}

/// Decodes one image file to RGBA, sniffing the format instead of trusting the extension.
fn load_rgba(path: &Path) -> anyhow::Result<RgbaImage> {
    let reader = image::ImageReader::open(path)
        .with_context(|| format!("cannot open image {}", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("cannot sniff image format of {}", path.display()))?;
    let decoded = reader
        .decode()
        .with_context(|| format!("cannot decode image {}", path.display()))?;
    Ok(decoded.to_rgba8())
}

/// Registers (and returns) the `"PNG"` clipboard format id.
fn register_png_format() -> anyhow::Result<u32> {
    // SAFETY: the string is a static NUL-terminated PCWSTR.
    let format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    if format == 0 {
        bail!("RegisterClipboardFormatW(\"PNG\") failed");
    }
    Ok(format)
}

/// Opens the clipboard with a few retries and holds it open until this value is dropped.
struct ClipboardGuard;

impl ClipboardGuard {
    fn open(owner: Option<HWND>) -> Result<Self, windows::core::Error> {
        let mut last_error = None;
        for attempt in 1..=OPEN_ATTEMPTS {
            // SAFETY: `owner` is either the live panel window or NULL; OpenClipboard has no
            // pointer arguments whose lifetime outlives the call.
            match unsafe { OpenClipboard(owner) } {
                Ok(()) => {
                    if attempt > 1 {
                        log::debug!("clipboard: opened on attempt {attempt}");
                    }
                    return Ok(Self);
                }
                Err(error) => {
                    last_error = Some(error);
                    if attempt < OPEN_ATTEMPTS {
                        std::thread::sleep(OPEN_RETRY_DELAY);
                    }
                }
            }
        }
        let error = last_error.expect("at least one attempt was made");
        log::debug!(
            "clipboard: OpenClipboard(owner={owner:?}) failed after {OPEN_ATTEMPTS} attempt(s)"
        );
        Err(error)
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: this guard opened the clipboard on this thread; CloseClipboard has no handles.
        if let Err(error) = unsafe { CloseClipboard() } {
            if error.code() == ERROR_CLIPBOARD_NOT_OPEN {
                // `OpenClipboard(NULL)` used to make Close fail this way even though the data was
                // published (Windows' documented NULL-owner flakiness). The writer now opens with
                // the panel window as owner, so reaching this line means the owner was unavailable
                // or the window changed; keep diagnosing it without failing a verified copy.
                log::warn!(
                    "clipboard: CloseClipboard reported ERROR_CLIPBOARD_NOT_OPEN despite the \
                     window owner"
                );
            } else {
                log::warn!("clipboard: CloseClipboard failed: {error}");
            }
        }
    }
}

/// Why one whole write sequence did not publish every expected format.
enum WriteAttemptError {
    /// `EmptyClipboard`/`SetClipboardData` failed with a Windows error.
    Windows(windows::core::Error),
    /// The calls succeeded but verification did not see all expected formats.
    Missing(Vec<ClipFormat>),
    /// Payload allocation failed before the clipboard was touched (never retryable).
    Other(anyhow::Error),
}

/// `ERROR_CLIPBOARD_NOT_OPEN` is transient; every other clipboard error fails the copy.
fn is_retryable_clipboard_error(error: &windows::core::Error) -> bool {
    error.code() == ERROR_CLIPBOARD_NOT_OPEN
}

/// Whether `attempt` (1-based) may be followed by another whole-sequence attempt.
fn should_retry(attempt: usize) -> bool {
    attempt < WRITE_ATTEMPTS
}

/// Returns the expected formats that `is_available` reports as absent, in write order.
fn missing_formats(
    expected: &[(ClipFormat, u32)],
    mut is_available: impl FnMut(u32) -> bool,
) -> Vec<ClipFormat> {
    expected
        .iter()
        .filter(|(_, id)| !is_available(*id))
        .map(|(format, _)| *format)
        .collect()
}

/// Publishes `payloads`, verifying every expected format and retrying a partial write as a whole.
fn write_payloads(payloads: &[ClipboardPayload], owner: Option<HWND>) -> anyhow::Result<()> {
    let expected: Vec<(ClipFormat, u32)> = payloads
        .iter()
        .map(|payload| (payload.format, payload.id))
        .collect();

    for attempt in 1..=WRITE_ATTEMPTS {
        match write_once(payloads, &expected, owner) {
            Ok(()) => {
                if attempt > 1 {
                    log::info!("clipboard: verified every format on attempt {attempt}");
                }
                return Ok(());
            }
            Err(WriteAttemptError::Other(error)) => return Err(error),
            Err(WriteAttemptError::Windows(error)) => {
                if !is_retryable_clipboard_error(&error) || !should_retry(attempt) {
                    return Err(anyhow::Error::new(error).context(format!(
                        "clipboard write failed on attempt {attempt} of {WRITE_ATTEMPTS}"
                    )));
                }
                log::warn!(
                    "clipboard: attempt {attempt} failed with ERROR_CLIPBOARD_NOT_OPEN; \
                     retrying the whole write"
                );
            }
            Err(WriteAttemptError::Missing(missing)) => {
                if !should_retry(attempt) {
                    bail!(
                        "clipboard: {:?} still missing after {WRITE_ATTEMPTS} complete attempt(s)",
                        missing
                    );
                }
                log::warn!(
                    "clipboard: attempt {attempt} did not publish {:?}; retrying the whole write",
                    missing
                );
            }
        }
        std::thread::sleep(WRITE_RETRY_DELAY);
    }
    unreachable!("the loop returns or bails on its last attempt")
}

/// One whole write sequence: allocate, open, empty, set, verify (the clipboard is closed on exit).
fn write_once(
    payloads: &[ClipboardPayload],
    expected: &[(ClipFormat, u32)],
    owner: Option<HWND>,
) -> Result<(), WriteAttemptError> {
    // Allocate every block first: after EmptyClipboard a late allocation failure would leave the
    // clipboard empty, which this task must avoid as far as Windows allows.
    let mut blocks = Vec::with_capacity(payloads.len());
    for payload in payloads {
        let block = OwnedHGlobal::from_bytes(&payload.bytes)
            .with_context(|| format!("cannot allocate clipboard format {:?}", payload.format))
            .map_err(WriteAttemptError::Other)?;
        blocks.push((payload.id, block));
    }

    let _clipboard = ClipboardGuard::open(owner).map_err(WriteAttemptError::Windows)?;
    // SAFETY: the clipboard is open and owned by this thread.
    unsafe { EmptyClipboard() }.map_err(WriteAttemptError::Windows)?;

    let mut first_set_error: Option<windows::core::Error> = None;
    for (id, block) in blocks {
        // SAFETY: `block` is a GMEM_MOVEABLE allocation; SetClipboardData takes ownership on
        // success, while on failure the OwnedHGlobal still frees it.
        match unsafe { SetClipboardData(id, Some(HANDLE(block.handle().0))) } {
            Ok(_) => {
                let _ = block.release();
            }
            Err(error) => {
                log::warn!("clipboard: SetClipboardData(format {id}) failed: {error}");
                if first_set_error.is_none() {
                    first_set_error = Some(error);
                }
            }
        }
    }

    // Verification catches a set that Windows reported as successful but did not publish, and a
    // set that failed with ERROR_CLIPBOARD_NOT_OPEN; both mean the write is partial.
    let missing = missing_formats(expected, |id| {
        // SAFETY: IsClipboardFormatAvailable takes a numeric format id and no pointers.
        unsafe { IsClipboardFormatAvailable(id).is_ok() }
    });
    if !missing.is_empty() {
        if let Some(error) = first_set_error {
            if !is_retryable_clipboard_error(&error) {
                return Err(WriteAttemptError::Windows(error));
            }
        }
        return Err(WriteAttemptError::Missing(missing));
    }

    log::info!(
        "clipboard: wrote {} format(s) and verified all of them",
        expected.len()
    );
    Ok(())
}

/// Bytes of the `CF_DIBV5` payload (header + pixels).
pub(crate) fn dibv5_bytes(image: &RgbaImage) -> Vec<u8> {
    let width = image.width();
    let height = image.height();
    let stride = dibv5_stride(width);
    let mut bytes = dibv5_header(width, height);
    bytes.reserve(stride.saturating_mul(height as usize));
    // Top-down rows: display order, BGRA, opaque alpha. The stride is width * 4, already
    // DWORD-aligned, so no per-row padding is needed.
    for y in 0..height {
        for x in 0..width {
            let pixel = image.get_pixel(x, y);
            bytes.push(pixel[2]);
            bytes.push(pixel[1]);
            bytes.push(pixel[0]);
            bytes.push(u8::MAX);
        }
    }
    debug_assert_eq!(bytes.len(), DIBV5_HEADER_SIZE + stride * height as usize);
    bytes
}

/// The `BITMAPV5HEADER` bytes for a `width` x `height` 32 bpp top-down bitmap.
pub(crate) fn dibv5_header(width: u32, height: u32) -> Vec<u8> {
    let stride = dibv5_stride(width) as u64;
    let image_size = stride.saturating_mul(u64::from(height)).min(u64::from(u32::MAX)) as u32;
    let signed_width = i32::try_from(width).unwrap_or(i32::MAX);
    let signed_height = i32::try_from(height).unwrap_or(i32::MAX);

    let mut bytes = Vec::with_capacity(DIBV5_HEADER_SIZE);
    push_u32(&mut bytes, DIBV5_HEADER_SIZE as u32); // bV5Size
    push_i32(&mut bytes, signed_width); // bV5Width
    push_i32(&mut bytes, -signed_height); // bV5Height (negative = top-down)
    push_u16(&mut bytes, 1); // bV5Planes
    push_u16(&mut bytes, 32); // bV5BitCount
    push_u32(&mut bytes, BI_BITFIELDS); // bV5Compression
    push_u32(&mut bytes, image_size); // bV5SizeImage
    push_i32(&mut bytes, 0); // bV5XPelsPerMeter
    push_i32(&mut bytes, 0); // bV5YPelsPerMeter
    push_u32(&mut bytes, 0); // bV5ClrUsed
    push_u32(&mut bytes, 0); // bV5ClrImportant
    push_u32(&mut bytes, 0x00FF_0000); // bV5RedMask
    push_u32(&mut bytes, 0x0000_FF00); // bV5GreenMask
    push_u32(&mut bytes, 0x0000_00FF); // bV5BlueMask
    push_u32(&mut bytes, 0xFF00_0000); // bV5AlphaMask
    push_u32(&mut bytes, LCS_SRGB); // bV5CSType
    bytes.extend_from_slice(&[0u8; 36]); // bV5Endpoints (ignored for sRGB)
    push_u32(&mut bytes, 0); // bV5GammaRed
    push_u32(&mut bytes, 0); // bV5GammaGreen
    push_u32(&mut bytes, 0); // bV5GammaBlue
    push_u32(&mut bytes, LCS_GM_IMAGES); // bV5Intent
    push_u32(&mut bytes, 0); // bV5ProfileData
    push_u32(&mut bytes, 0); // bV5ProfileSize
    push_u32(&mut bytes, 0); // bV5Reserved

    debug_assert_eq!(bytes.len(), DIBV5_HEADER_SIZE);
    bytes
}

/// Row stride in bytes of a 32 bpp DIB (`width * 4`, already DWORD aligned).
pub(crate) fn dibv5_stride(width: u32) -> usize {
    // 32 bpp rows are always a multiple of 4 bytes, so the DIB scanline padding is zero.
    (width as usize).saturating_mul(4)
}

/// Encodes `image` as PNG bytes for the registered `"PNG"` clipboard format.
pub(crate) fn png_bytes(image: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .context("PNG encoding failed")?;
    Ok(bytes)
}

/// Bytes of a one-file `CF_HDROP` payload (`DROPFILES` + UTF-16 double-NUL list).
pub(crate) fn hdrop_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    let header_size = std::mem::size_of::<DROPFILES>();
    debug_assert_eq!(header_size, 20, "DROPFILES layout changed");
    let header = DROPFILES {
        pFiles: header_size as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL::from(false),
        fWide: BOOL::from(true),
    };
    let mut bytes = Vec::new();
    // SAFETY: DROPFILES is a plain C struct with no pointers; copying its initialized bytes is
    // valid for `header_size` bytes.
    bytes.extend_from_slice(unsafe {
        std::slice::from_raw_parts(&header as *const DROPFILES as *const u8, header_size)
    });
    for unit in path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .chain(std::iter::once(0))
    {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// One `GMEM_MOVEABLE` block whose ownership is transferred to the clipboard on success.
pub(crate) struct OwnedHGlobal(HGLOBAL);

impl OwnedHGlobal {
    /// Allocates and fills a moveable global block with `bytes`.
    pub(crate) fn from_bytes(bytes: &[u8]) -> windows::core::Result<Self> {
        // SAFETY: GlobalAlloc/GlobalLock are the documented moveable-memory pair. The block stays
        // owned by this value until `release` (clipboard) or Drop (failure).
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
            let pointer = GlobalLock(handle);
            if pointer.is_null() {
                let _ = GlobalFree(Some(handle));
                return Err(E_OUTOFMEMORY.into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
            // GlobalUnlock returns FALSE when the last lock is released, which is the success
            // case; the windows wrapper turns that into an error we must ignore.
            let _ = GlobalUnlock(handle);
            Ok(Self(handle))
        }
    }

    /// The raw handle (still owned by this value).
    pub(crate) fn handle(&self) -> HGLOBAL {
        self.0
    }

    /// Transfers ownership to the caller (the clipboard, after a successful `SetClipboardData`).
    pub(crate) fn release(self) -> HGLOBAL {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
}

impl Drop for OwnedHGlobal {
    fn drop(&mut self) {
        // SAFETY: the handle came from GlobalAlloc and was not transferred.
        unsafe {
            let _ = GlobalFree(Some(self.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use image::Rgba;
    use windows::Win32::System::Memory::GlobalUnlock;

    /// `BITMAPV5HEADER::bV5Size`.
    const HEADER_SIZE: usize = 124;

    fn u16_at(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn i32_at(bytes: &[u8], offset: usize) -> i32 {
        i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn image_items_advertise_png_bitmap_and_file_drop() {
        for kind in [
            ItemKind::Capture,
            ItemKind::SystemScreenshot,
            ItemKind::Clipboard,
            ItemKind::WatchedFolder,
        ] {
            assert_eq!(
                formats_for(kind),
                vec![ClipFormat::Png, ClipFormat::DibV5, ClipFormat::FileDrop],
                "{kind:?} must advertise the PNG flavour first (spec 4.7)"
            );
        }
    }

    #[test]
    fn recording_only_ships_file_drop() {
        assert_eq!(formats_for(ItemKind::Recording), vec![ClipFormat::FileDrop]);
    }

    #[test]
    fn expected_formats_are_the_verification_contract() {
        for kind in [
            ItemKind::Capture,
            ItemKind::SystemScreenshot,
            ItemKind::Clipboard,
            ItemKind::WatchedFolder,
        ] {
            assert_eq!(
                expected_formats(kind),
                vec![ClipFormat::Png, ClipFormat::DibV5, ClipFormat::FileDrop],
                "{kind:?} must verify all three representations"
            );
        }
        assert_eq!(
            expected_formats(ItemKind::Recording),
            vec![ClipFormat::FileDrop],
            "a recording is only published as a file drop"
        );
    }

    #[test]
    fn verification_lists_each_absent_expected_format() {
        const PNG_ID: u32 = 49300;
        let expected = [
            (ClipFormat::Png, PNG_ID),
            (ClipFormat::DibV5, CF_DIBV5_ID),
            (ClipFormat::FileDrop, CF_HDROP_ID),
        ];

        assert!(missing_formats(&expected, |_| true).is_empty());
        assert_eq!(
            missing_formats(&expected, |id| id != CF_DIBV5_ID),
            vec![ClipFormat::DibV5],
            "only the absent DIB is reported"
        );
        assert_eq!(
            missing_formats(&expected, |_| false),
            vec![ClipFormat::Png, ClipFormat::DibV5, ClipFormat::FileDrop],
            "the report follows write order"
        );
    }

    #[test]
    fn a_partial_write_is_retried_only_while_attempts_remain() {
        assert!(should_retry(1), "the initial attempt may be repeated");
        assert!(should_retry(WRITE_ATTEMPTS - 1), "the last retry is allowed");
        assert!(
            !should_retry(WRITE_ATTEMPTS),
            "the final attempt must fail instead of looping"
        );
    }

    #[test]
    fn dibv5_header_describes_a_top_down_32bpp_bitmap() {
        let header = dibv5_header(3, 2);
        assert_eq!(header.len(), HEADER_SIZE, "BITMAPV5HEADER is 124 bytes");
        assert_eq!(u32_at(&header, 0), HEADER_SIZE as u32);
        assert_eq!(i32_at(&header, 4), 3, "width");
        assert_eq!(
            i32_at(&header, 8),
            -2,
            "a negative height marks the rows as top-down"
        );
        assert_eq!(u16_at(&header, 12), 1, "one colour plane");
        assert_eq!(u16_at(&header, 14), 32, "BGRA");
        assert_eq!(u32_at(&header, 16), 3, "BI_BITFIELDS");
        assert_eq!(
            u32_at(&header, 20),
            (dibv5_stride(3) * 2) as u32,
            "bV5SizeImage is stride * height"
        );
        assert_eq!(u32_at(&header, 40), 0x00FF_0000, "red mask");
        assert_eq!(u32_at(&header, 44), 0x0000_FF00, "green mask");
        assert_eq!(u32_at(&header, 48), 0x0000_00FF, "blue mask");
        assert_eq!(u32_at(&header, 52), 0xFF00_0000, "alpha mask");
        assert_eq!(u32_at(&header, 56), 0x7352_4742, "LCS_sRGB");
        assert_eq!(u32_at(&header, 108), 4, "LCS_GM_IMAGES");
    }

    #[test]
    fn dibv5_stride_is_dword_aligned_rgba() {
        assert_eq!(dibv5_stride(1), 4);
        assert_eq!(dibv5_stride(3), 12);
        assert_eq!(dibv5_stride(640), 2560);
    }

    #[test]
    fn dibv5_pixels_are_bgra_top_down_and_forced_opaque() {
        let mut image = RgbaImage::new(2, 2);
        image.put_pixel(0, 0, Rgba([255, 0, 0, 128])); // top-left, red
        image.put_pixel(1, 0, Rgba([0, 255, 0, 0])); // top-right, green
        image.put_pixel(0, 1, Rgba([0, 0, 255, 255])); // bottom-left, blue
        image.put_pixel(1, 1, Rgba([10, 20, 30, 40]));

        let bytes = dibv5_bytes(&image);
        let stride = dibv5_stride(2);
        assert_eq!(bytes.len(), HEADER_SIZE + stride * 2);

        // First row is the image's top row (top-down DIB) and every alpha byte is 255.
        assert_eq!(&bytes[HEADER_SIZE..HEADER_SIZE + 4], &[0, 0, 255, 255]);
        assert_eq!(
            &bytes[HEADER_SIZE + 4..HEADER_SIZE + 8],
            &[0, 255, 0, 255]
        );
        assert_eq!(
            &bytes[HEADER_SIZE + stride..HEADER_SIZE + stride + 4],
            &[255, 0, 0, 255]
        );
        assert_eq!(
            &bytes[HEADER_SIZE + stride + 4..HEADER_SIZE + stride + 8],
            &[30, 20, 10, 255]
        );
    }

    #[test]
    fn png_payload_round_trips_the_image() {
        let mut image = RgbaImage::new(3, 2);
        image.put_pixel(1, 1, Rgba([1, 2, 3, 4]));
        let png = png_bytes(&image).expect("encode PNG");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let decoded = image::load_from_memory(&png).expect("decode PNG");
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        let decoded = decoded.to_rgba8();
        assert_eq!(decoded.get_pixel(1, 1), &Rgba([1, 2, 3, 4]));
    }

    #[test]
    fn hdrop_payload_is_a_wide_single_file_list() {
        let path = Path::new(r"C:\Users\Example\Pictures\shot.png");
        let bytes = hdrop_bytes(path);
        assert_eq!(u32_at(&bytes, 0), 20, "pFiles points past DROPFILES");
        assert_eq!(u32_at(&bytes, 12), 0, "fNC = FALSE");
        assert_eq!(u32_at(&bytes, 16), 1, "fWide = TRUE");

        let units: Vec<u16> = bytes[20..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(&units[units.len() - 2..], &[0, 0], "double NUL terminator");
        let text: Vec<u16> = units[..units.len() - 2].to_vec();
        assert_eq!(String::from_utf16(&text).unwrap(), path.to_string_lossy());
    }

    #[test]
    fn owned_hglobal_copies_the_bytes_and_transfers_ownership() {
        let owned = OwnedHGlobal::from_bytes(b"clipboard payload").expect("allocate HGLOBAL");
        let pointer = unsafe { GlobalLock(owned.handle()) };
        assert!(!pointer.is_null(), "the block must be lockable");
        let copied = unsafe {
            std::slice::from_raw_parts(pointer.cast::<u8>(), b"clipboard payload".len())
        };
        assert_eq!(copied, b"clipboard payload");
        // SAFETY: the copy above is finished; the lock count reaches zero here.
        let _ = unsafe { GlobalUnlock(owned.handle()) };

        let raw = owned.release();
        // SAFETY: `release` transferred ownership to this test, which frees it exactly once.
        let _ = unsafe { GlobalFree(Some(raw)) };
    }
}
