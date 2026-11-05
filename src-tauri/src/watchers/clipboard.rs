//! Clipboard image watcher (Task 9): event-driven listener, privacy filter, RGBA decode, PNG save.
//!
//! Design spec 3.2 source C: an image copied to the clipboard (Win+Shift+S, a browser image, a chat
//! app) hangs on the line by itself, without Hangline ever touching the clipboard first.
//!
//! # Listening
//!
//! One dedicated thread owns a message-only window (`RegisterClassW` + `CreateWindowExW` with
//! `HWND_MESSAGE`) and registers `AddClipboardFormatListener`. `GetMessageW` delivers
//! `WM_CLIPBOARDUPDATE`; the loop handles it directly (the window procedure stays a pass-through to
//! `DefWindowProcW`), so the idle cost is zero. `WM_DESTROY`/`WM_QUIT`/`WM_CLOSE` end the loop and
//! `RemoveClipboardFormatListener` runs before the window is destroyed.
//!
//! If the message window or the listener cannot be created, the same thread falls back to polling
//! `GetClipboardSequenceNumber()` every 300 ms (design spec 3.2 C, "300 ms 轮询兜底"). Both paths
//! ignore the clipboard state that already existed at startup: only a *change* is a copy.
//!
//! A copy is read ~120 ms after its first notification ([`CLIPBOARD_SETTLE_DELAY`]): Windows posts
//! `WM_CLIPBOARDUPDATE` for every intermediate step of a multi-format copy, and reading the first
//! one races the owner (observed as a `GetClipboardData` failure) and can miss the text/HTML
//! markers that must veto the capture. The sequence number is re-read after the delay, so all the
//! queued notifications for the finished copy collapse into one read. A read that still races the
//! owner (or another clipboard user) is retried up to [`READ_ATTEMPTS`] times; each retry re-runs
//! the privacy filter against the settled format list, so a retry can only reject more, never less.
//!
//! # What is accepted (design spec 4.6 D)
//!
//! A copy is accepted only when it has at least one of `PNG`, `CF_DIBV5`, `CF_DIB` **and** no
//! format from the reject set: `CF_UNICODETEXT`, `CF_TEXT`, `CF_HDROP`,
//! `ExcludeClipboardContentFromMonitorProcessing`, `Clipboard Viewer Ignore`,
//! `org.nspasteboard.ConcealedType`, `HTML Format`, and `CanIncludeInClipboardHistory` whose DWORD
//! value is 0 (an unreadable value counts as 0 and is rejected conservatively). The pure decision
//! table is [`should_accept`]; the reader turns the history opt-out into the [`HISTORY_EXCLUDED`]
//! marker because a function over names cannot read the value.
//!
//! Consequences, documented on purpose:
//! - a browser/image-editor copy carries text and/or `HTML Format`, so it is *not* captured —
//!   exactly the "never steal a text/file copy" privacy rule of the spec;
//! - Explorer's file copy carries `CF_HDROP`, so file copies are never captured;
//! - Hangline's own `copy_item` (Task 13) writes `PNG` + `CF_DIBV5` + `CF_HDROP`, so the
//!   `CF_HDROP` reject filters the app's own writes even though the watcher has no special case;
//! - a copy that only exposes `CF_BITMAP` (e.g. WinForms `Clipboard.SetImage`) is not accepted,
//!   because the spec only whitelists `PNG`/`CF_DIBV5`/`CF_DIB`; every mainstream image producer
//!   publishes one of those.
//!
//! # Decoding
//!
//! The `PNG` flavour is preferred: it is self-describing, lossless and alpha-correct. Falling back
//! to `CF_DIBV5` and then `CF_DIB` covers producers that only publish a DIB; those payloads are
//! parsed here (`BITMAPINFOHEADER`/`BITMAPV4/V5HEADER`, 16/24/32 bpp, `BI_RGB`/`BI_BITFIELDS`,
//! bottom-up and top-down rows, 4-byte scanline padding) and converted to RGBA. A 32bpp alpha
//! channel is used only when the payload really carries one (a non-zero alpha byte or a V4/V5
//! alpha mask); GDI screenshots leave the fourth byte at zero and are forced opaque.
//!
//! Caps (design spec 4.6 D rule 4): the clipboard payload may be at most 64 MiB and the decoded
//! image at most 80 MP. The pixel cap is checked with division (`width > MAX_PIXELS / height`) so a
//! hostile header cannot overflow. `PNG` payloads are trimmed at their `IEND` chunk first because
//! `GlobalSize` can report a block-rounded size.
//!
//! # Saving and dedupe
//!
//! Accepted images are encoded once, hashed with BLAKE3-64 over those exact bytes and written to
//! `%LOCALAPPDATA%\Hangline\Screenshots\Clipboard yyyy-MM-dd HH.mm.ss.png` through a `.png.tmp`
//! file plus an atomic rename. A same-second collision gets `-2`, `-3`, ... (the spec permits an
//! overwrite, but clipboard bursts are likely and silently losing an image is worse). The hash is
//! inserted in the shared [`Deduper`] *before* the file becomes visible, so the folder watcher that
//! also watches this directory suppresses the same content instead of hanging it twice; a copy
//! that repeats within the 20-entry LRU window is skipped. A failed persist releases the hash
//! ([`Deduper::forget`]) exactly like the folder watcher. New items go through the single
//! [`crate::app_state::emit_state`] choke point, then a Rust-only `reveal-request` when
//! `revealOnNew` is on (Task 11 turns it into the frontend `reveal` event).
//!
//! # Privacy and logging
//!
//! Clipboard contents are never logged: only format names, dimensions and the content hash appear
//! in the log. A rejected copy is logged at info level with its format names only; a decode failure
//! is logged as a warning without the payload.
//!
//! # Permissions
//!
//! Windows has no POSIX `0600`: the saved PNG inherits the ACL of
//! `%LOCALAPPDATA%\Hangline\Screenshots`, which grants the signed-in user (plus SYSTEM and
//! Administrators) only. [`apply_restrictive_permissions`] documents that and applies `0600` on
//! non-Windows test targets.
//!
//! # Gating and lifecycle
//!
//! [`start`] is called by [`crate::watchers::start_all`] only when `settings.watch_clipboard` is
//! true at startup (Task 15 will add a restart API). The returned [`ClipboardWatcher`] is owned by
//! [`crate::watchers::Watchers`]; dropping it stops the loop (a `WM_CLOSE` wakes the blocking
//! `GetMessageW`) and joins the listener thread.

use std::ffi::c_void;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{bail, Context};
use image::{ImageEncoder, ImageFormat, RgbaImage};
use tauri::{AppHandle, Emitter, EventTarget, Manager};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM, ERROR_CLASS_ALREADY_EXISTS,
    SYSTEMTIME,
};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardSequenceNumber, OpenClipboard,
    RemoveClipboardFormatListener,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    PostMessageW, RegisterClassW, SetTimer, TranslateMessage, HCURSOR, HICON, HWND_MESSAGE, MSG,
    WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW, WNDCLASS_STYLES, WM_CLIPBOARDUPDATE, WM_CLOSE,
    WM_DESTROY, WM_QUIT, WM_TIMER,
};

use crate::app_state::{self, AppState};
use crate::capture;
use crate::dedupe::{self, Deduper};
use crate::item::{Item, ItemKind};
use crate::watchers::dirs::REVEAL_REASON_NEW_ITEM;

/// Marker the reader appends when `CanIncludeInClipboardHistory` is present with a DWORD value of
/// zero (or its value cannot be read): such a copy must never be captured.
pub const HISTORY_EXCLUDED: &str = "CanIncludeInClipboardHistory=0";

/// Clipboard formats that make a copy a *candidate* image (design spec 4.6 D rule 2).
const IMAGE_FORMATS: &[&str] = &["PNG", "CF_DIBV5", "CF_DIB"];

/// Formats/markers that disqualify a copy, even when it also carries image data (spec 4.6 D rule 3).
const REJECT_FORMATS: &[&str] = &[
    // Text: a browser/Excel/chat copy carries its text representation next to the bitmap.
    "CF_UNICODETEXT",
    "CF_TEXT",
    // Files: Explorer copies and Hangline's own `copy_item` (`PNG` + `CF_DIBV5` + `CF_HDROP`).
    "CF_HDROP",
    // Windows clipboard-history/monitor opt-outs.
    "ExcludeClipboardContentFromMonitorProcessing",
    "Clipboard Viewer Ignore",
    // macOS-style concealed marker, seen in cross-platform clipboard tooling.
    "org.nspasteboard.ConcealedType",
    // A browser image copy always includes the HTML flavour.
    "HTML Format",
    // See [`HISTORY_EXCLUDED`].
    HISTORY_EXCLUDED,
];

/// Largest clipboard payload (encoded PNG or DIB bytes) this watcher reads: 64 MiB.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Largest accepted image, in pixels: 80 MP (design spec 4.6 D rule 4).
pub const MAX_PIXELS: u64 = 80_000_000;

/// The pure format decision table: accept a copy only when it carries an image format and no
/// reject marker.
///
/// `formats` are the clipboard format names the reader enumerated for one clipboard state; the
/// reader is responsible for turning a present `CanIncludeInClipboardHistory` whose DWORD is zero
/// (or unreadable) into [`HISTORY_EXCLUDED`], because a pure function over names cannot read the
/// value. Names are compared ASCII case-insensitively: `GetClipboardFormatNameW` returns whatever
/// casing the registering application used.
///
/// The documented reject set is exactly:
/// `CF_UNICODETEXT`, `CF_TEXT`, `CF_HDROP`, `ExcludeClipboardContentFromMonitorProcessing`,
/// `Clipboard Viewer Ignore`, `org.nspasteboard.ConcealedType`, `HTML Format`, and
/// [`HISTORY_EXCLUDED`]. `CF_OEMTEXT`, `CF_LOCALE` and `CF_BITMAP` are not in the set; a copy that
/// carries only those (WinForms `Clipboard.SetImage` sets just `CF_BITMAP`) is not accepted,
/// because design spec 4.6 D only whitelists `PNG`/`CF_DIBV5`/`CF_DIB`.
pub fn should_accept(formats: &[&str]) -> bool {
    let has_image = formats
        .iter()
        .any(|format| IMAGE_FORMATS.iter().any(|image| format.eq_ignore_ascii_case(image)));
    if !has_image {
        return false;
    }
    !formats.iter().any(|format| {
        REJECT_FORMATS
            .iter()
            .any(|reject| format.eq_ignore_ascii_case(reject))
    })
}

/// Enforces the payload-size cap before anything is copied or decoded.
fn check_payload_len(len: usize) -> anyhow::Result<()> {
    if len == 0 {
        bail!("clipboard payload is empty");
    }
    if len > MAX_PAYLOAD_BYTES {
        bail!("clipboard payload is {len} bytes, above the {MAX_PAYLOAD_BYTES}-byte (64 MiB) cap");
    }
    Ok(())
}

/// Enforces the pixel cap from image dimensions alone, using division so `width * height` can
/// never overflow (a hostile DIB header can claim `u32::MAX` for both fields).
fn check_pixel_cap(width: u32, height: u32) -> anyhow::Result<()> {
    if width == 0 || height == 0 {
        bail!("image dimensions must be non-zero, got {width}x{height}");
    }
    if u64::from(width) > MAX_PIXELS / u64::from(height) {
        bail!("image dimensions {width}x{height} exceed the {MAX_PIXELS}-pixel (80 MP) cap");
    }
    Ok(())
}

/// Trims a block-rounded clipboard `PNG` payload at the end of its `IEND` chunk.
///
/// `GetClipboardData` hands out a global-memory block, and `GlobalSize` reports the whole block,
/// which can be rounded past the bytes the owner actually wrote. The PNG decoder would see those
/// trailing bytes; walking the chunk headers and cutting after `IEND` removes them. A payload that
/// is not a well-formed PNG is returned unchanged so the decoder reports the real error.
fn trim_png(bytes: &[u8]) -> &[u8] {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < SIGNATURE.len() || bytes[..SIGNATURE.len()] != SIGNATURE {
        return bytes;
    }
    let mut offset = SIGNATURE.len();
    // A chunk is 4 bytes of length, 4 bytes of type, `length` bytes of data and 4 bytes of CRC.
    while offset + 12 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let chunk_type = &bytes[offset + 4..offset + 8];
        let Some(end) = offset
            .checked_add(12)
            .and_then(|end| end.checked_add(length))
        else {
            return bytes;
        };
        if end > bytes.len() {
            return bytes;
        }
        offset = end;
        if chunk_type == b"IEND" {
            return &bytes[..offset];
        }
    }
    bytes
}

/// Decodes a clipboard `PNG` payload (a self-describing PNG stream) into RGBA pixels.
fn decode_png(bytes: &[u8]) -> anyhow::Result<RgbaImage> {
    check_payload_len(bytes.len())?;
    let trimmed = trim_png(bytes);
    let reader = image::ImageReader::new(std::io::Cursor::new(trimmed))
        .with_guessed_format()
        .map_err(|error| anyhow::anyhow!("cannot read the clipboard PNG header: {error}"))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| anyhow::anyhow!("cannot read the clipboard PNG dimensions: {error}"))?;
    check_pixel_cap(width, height)?;
    let image = image::load_from_memory_with_format(trimmed, ImageFormat::Png)
        .map_err(|error| anyhow::anyhow!("cannot decode the clipboard PNG: {error}"))?;
    Ok(image.into_rgba8())
}

/// One parsed clipboard DIB header plus everything needed to walk its scanlines.
#[derive(Clone, Copy, Debug)]
struct DibHeader {
    width: u32,
    height: u32,
    /// Stored top-down (`biHeight < 0`) instead of the usual bottom-up.
    top_down: bool,
    bit_count: u16,
    /// `BI_RGB` (0) or `BI_BITFIELDS` (3).
    compression: u32,
    /// Index of the first pixel byte inside the payload.
    pixel_offset: usize,
    /// Row length in bytes, including the 4-byte alignment padding.
    stride: usize,
    /// `[red, green, blue, alpha]` masks for `BI_BITFIELDS` (`alpha == 0`: no alpha channel).
    masks: Option<[u32; 4]>,
}

/// Reads a little-endian `u16`, or `None` when the payload is too short.
fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

/// Reads a little-endian `u32`, or `None` when the payload is too short.
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Reads a little-endian `i32`, or `None` when the payload is too short.
fn read_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    read_u32(bytes, offset).map(|value| value as i32)
}

/// Parses a clipboard `CF_DIB`/`CF_DIBV5` payload and validates it against the caps.
///
/// Accepted headers are `BITMAPINFOHEADER` (40 bytes, the `CF_DIB` case) and the `BITMAPV4/V5`
/// extensions (108/124 bytes, `CF_DIBV5`, which carry the alpha mask). Palette entries are skipped
/// using `biClrUsed` (or the implied 2^bpp palette); the image itself must be 16, 24 or 32 bpp with
/// `BI_RGB` or `BI_BITFIELDS` compression. Everything else is rejected with a logged reason.
fn parse_dib(bytes: &[u8]) -> anyhow::Result<DibHeader> {
    check_payload_len(bytes.len())?;
    if bytes.len() < 40 {
        bail!(
            "clipboard DIB is {} bytes, shorter than a BITMAPINFOHEADER",
            bytes.len()
        );
    }
    let header_size = read_u32(bytes, 0).ok_or_else(|| anyhow::anyhow!("truncated DIB header"))?;
    if header_size < 40 {
        bail!("unsupported DIB header size {header_size}");
    }
    if header_size as usize > bytes.len() {
        bail!(
            "DIB header claims {header_size} bytes but the payload has {}",
            bytes.len()
        );
    }
    let raw_width = read_i32(bytes, 4).ok_or_else(|| anyhow::anyhow!("truncated DIB width"))?;
    let raw_height = read_i32(bytes, 8).ok_or_else(|| anyhow::anyhow!("truncated DIB height"))?;
    if raw_width <= 0 {
        bail!("DIB width must be positive, got {raw_width}");
    }
    if raw_height == 0 {
        bail!("DIB height must be non-zero");
    }
    let planes = read_u16(bytes, 12).ok_or_else(|| anyhow::anyhow!("truncated DIB planes"))?;
    if planes != 1 {
        bail!("DIB has {planes} planes; only one is supported");
    }
    let bit_count = read_u16(bytes, 14).ok_or_else(|| anyhow::anyhow!("truncated DIB bpp"))?;
    if !matches!(bit_count, 16 | 24 | 32) {
        bail!("unsupported DIB bit count {bit_count} (only 16/24/32 bpp are decoded)");
    }
    let compression =
        read_u32(bytes, 16).ok_or_else(|| anyhow::anyhow!("truncated DIB compression"))?;
    let clr_used = read_u32(bytes, 32).unwrap_or(0);

    let width = raw_width as u32;
    let height = raw_height.unsigned_abs();
    check_pixel_cap(width, height)?;

    // BI_BITFIELDS masks live inside a V4/V5 header; a 40-byte header is followed by three masks
    // instead (there is no alpha mask in that variant).
    let (masks, masks_len) = match compression {
        0 => (None, 0usize), // BI_RGB
        3 => {
            if header_size >= 52 {
                let mut masks = [0u32; 4];
                for (index, mask) in masks.iter_mut().enumerate() {
                    *mask = read_u32(bytes, 40 + index * 4)
                        .ok_or_else(|| anyhow::anyhow!("truncated DIB channel masks"))?;
                }
                (Some(masks), 0)
            } else {
                if bytes.len() < 52 {
                    bail!("BI_BITFIELDS DIB is missing its channel masks");
                }
                let mut masks = [0u32; 4];
                for (index, mask) in masks.iter_mut().take(3).enumerate() {
                    *mask = read_u32(bytes, 40 + index * 4)
                        .ok_or_else(|| anyhow::anyhow!("truncated DIB channel masks"))?;
                }
                (Some(masks), 12)
            }
        }
        other => bail!("unsupported DIB compression {other} (only BI_RGB and BI_BITFIELDS)"),
    };

    let palette_entries = if clr_used > 0 {
        clr_used as usize
    } else {
        0 // 16/24/32 bpp never have an implied palette
    };
    let pixel_offset = (header_size as usize)
        .checked_add(masks_len)
        .and_then(|offset| offset.checked_add(palette_entries.checked_mul(4)?))
        .ok_or_else(|| anyhow::anyhow!("DIB palette is too large"))?;

    // ((width * bpp + 31) / 32) * 4 bytes per scanline.
    let stride = ((u64::from(width) * u64::from(bit_count) + 31) / 32 * 4) as usize;
    let pixels = u64::from(width) * u64::from(height);
    let required = (height as usize)
        .checked_mul(stride)
        .and_then(|row_bytes| pixel_offset.checked_add(row_bytes))
        .ok_or_else(|| anyhow::anyhow!("DIB dimensions are too large"))?;
    if bytes.len() < required {
        bail!(
            "DIB is truncated: {} bytes, needs {required} for {width}x{height} at {bit_count} bpp",
            bytes.len()
        );
    }
    let _ = pixels;

    Ok(DibHeader {
        width,
        height,
        top_down: raw_height < 0,
        bit_count,
        compression,
        pixel_offset,
        stride,
        masks,
    })
}

/// Extracts one channel from a packed pixel value and scales it to 0..=255.
fn channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    let raw = (value & mask) >> shift;
    ((u64::from(raw) * 255) / u64::from(max)) as u8
}

/// Decodes a clipboard `CF_DIBV5`/`CF_DIB` payload (a header plus pixel data, no file header).
////// Handles bottom-up (`biHeight > 0`) and top-down (`biHeight < 0`) 16/24/32 bpp images, including
/// the 4-byte scanline padding. A 32bpp alpha channel is honoured only when the payload really
/// carries alpha (a non-zero alpha byte, or a V4/V5 alpha mask): GDI screenshots and most `CF_DIB`
/// producers leave the fourth byte at zero, and honouring it blindly would make every screenshot
/// fully transparent.
fn decode_dib(bytes: &[u8]) -> anyhow::Result<RgbaImage> {
    let header = parse_dib(bytes)?;
    let width = header.width as usize;
    let height = header.height as usize;
    let bytes_per_pixel = usize::from(header.bit_count / 8);
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| anyhow::anyhow!("DIB dimensions are too large"))?;
    let byte_len = pixels
        .checked_mul(4)
        .ok_or_else(|| anyhow::anyhow!("DIB dimensions are too large"))?;
    let mut rgba = vec![0u8; byte_len];

    // A BI_RGB 32bpp DIB carries an alpha byte per pixel, but most producers leave it at zero.
    // Only a payload with at least one non-zero alpha byte is treated as alpha-bearing.
    let alpha_present = match (header.bit_count, header.compression) {
        (32, 0) => {
            let row_len = width * 4;
            (0..height).any(|row| {
                let start = header.pixel_offset + row * header.stride;
                bytes[start..start + row_len]
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] != 0)
            })
        }
        (32, 3) => header.masks.is_some_and(|masks| masks[3] != 0),
        _ => false,
    };

    for row in 0..height {
        let source_row = if header.top_down { row } else { height - 1 - row };
        let source_start = header.pixel_offset + source_row * header.stride;
        let source = &bytes[source_start..source_start + width * bytes_per_pixel];
        let target = &mut rgba[row * width * 4..(row + 1) * width * 4];
        match header.bit_count {
            32 => {
                for (index, pixel) in source.chunks_exact(4).enumerate() {
                    let out = &mut target[index * 4..index * 4 + 4];
                    match header.masks {
                        Some(masks) => {
                            let value =
                                u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
                            out[0] = channel(value, masks[0]);
                            out[1] = channel(value, masks[1]);
                            out[2] = channel(value, masks[2]);
                            out[3] = if masks[3] == 0 {
                                255
                            } else {
                                channel(value, masks[3])
                            };
                        }
                        None => {
                            out[0] = pixel[2];
                            out[1] = pixel[1];
                            out[2] = pixel[0];
                            out[3] = if alpha_present { pixel[3] } else { 255 };
                        }
                    }
                }
            }
            24 => {
                for (index, pixel) in source.chunks_exact(3).enumerate() {
                    let out = &mut target[index * 4..index * 4 + 4];
                    out[0] = pixel[2];
                    out[1] = pixel[1];
                    out[2] = pixel[0];
                    out[3] = 255;
                }
            }
            16 => {
                for (index, pixel) in source.chunks_exact(2).enumerate() {
                    let value = u16::from_le_bytes([pixel[0], pixel[1]]) as u32;
                    let out = &mut target[index * 4..index * 4 + 4];
                    match header.masks {
                        Some(masks) => {
                            out[0] = channel(value, masks[0]);
                            out[1] = channel(value, masks[1]);
                            out[2] = channel(value, masks[2]);
                            out[3] = if masks[3] == 0 {
                                255
                            } else {
                                channel(value, masks[3])
                            };
                        }
                        None => {
                            // BI_RGB at 16 bpp is RGB555.
                            out[0] = ((value >> 10) & 0x1F) as u8;
                            out[1] = ((value >> 5) & 0x1F) as u8;
                            out[2] = (value & 0x1F) as u8;
                            out[0] = (out[0] << 3) | (out[0] >> 2);
                            out[1] = (out[1] << 3) | (out[1] >> 2);
                            out[2] = (out[2] << 3) | (out[2] >> 2);
                            out[3] = 255;
                        }
                    }
                }
            }
            other => bail!("unsupported DIB bit count {other}"),
        }
    }

    RgbaImage::from_raw(header.width, header.height, rgba)
        .ok_or_else(|| anyhow::anyhow!("failed to build an image from the clipboard DIB"))
}

/// Builds the file stem for one clipboard save: `Clipboard yyyy-MM-dd HH.mm.ss` in local time.
fn clipboard_stem(local: SYSTEMTIME) -> String {
    format!(
        "Clipboard {:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

/// Picks a free `Clipboard ...png` path, numbering same-second collisions (`-2`, `-3`, ...).
///
/// Design spec 4.6 permits a same-second overwrite, but clipboard bursts (two copies within the
/// same second) are likely and silently losing the first image is worse than a suffix. The rule is
/// shared with the interactive capture saver (`capture::unique_png_path`, Task 16), so both own
/// save paths behave identically; all 99 numbered names taken falls back to a millisecond suffix.
fn unique_path(dir: &Path, stem: &str) -> PathBuf {
    crate::capture::unique_png_path(dir, stem)
}

/// Encodes an image as PNG bytes; the same bytes are hashed and then written unchanged.
fn encode_png(image: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("failed to encode the clipboard image as PNG")?;
    Ok(png)
}

/// Writes `png` to `<name>.png.tmp`, syncs it, then renames it into place atomically.
fn write_png_atomic(path: &Path, png: &[u8]) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("clipboard save path has no parent: {}", path.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let tmp = path.with_extension("png.tmp");
    let written = (|| -> anyhow::Result<()> {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(png)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", tmp.display()))?;
        drop(file);
        fs::rename(&tmp, path)
            .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))
    })();
    if written.is_err() {
        // Best effort: a stale .tmp must never masquerade as a saved image.
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Applies the most restrictive file permissions available on the platform (best effort).
///
/// Windows has no POSIX mode bit that means "0600": access is governed by the ACL inherited from
/// `%LOCALAPPDATA%\Hangline\Screenshots`, which grants the signed-in user (plus SYSTEM and
/// Administrators) and nothing else. Rewriting that ACL would need privileges beyond P0, so on
/// Windows this is a documented no-op instead of a pretend `0600`. On non-Windows targets (unit
/// tests only; the app is Windows-only) the POSIX owner-only mode is applied.
fn apply_restrictive_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            log::debug!(
                "clipboard: cannot restrict the permissions of {}: {error}",
                path.display()
            );
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Encodes, dedupes and atomically saves one clipboard image into `dir`.
///
/// Returns `Ok(None)` when the content is already in the dedupe LRU or the chosen path is already
/// hung on the line; returns the saved path and its BLAKE3-64 hash otherwise. The hash is computed
/// over the exact bytes that are written, so it equals `dedupe::hash_file` on the saved file (the
/// folder watcher of the same directory will compute exactly that hash and suppress the duplicate).
/// A failed write releases the dedupe entry so the same image can be copied again.
fn ingest_image(
    dir: &Path,
    image: &RgbaImage,
    deduper: &Deduper,
    is_hung: &dyn Fn(&Path) -> bool,
) -> anyhow::Result<Option<(PathBuf, u64)>> {
    let png = encode_png(image)?;
    let hash = dedupe::hash_bytes(&png);
    let path = unique_path(dir, &clipboard_stem(unsafe { GetLocalTime() }));

    if is_hung(&path) {
        log::info!(
            "clipboard: ignored the save to {} (already hung on the line)",
            path.display()
        );
        return Ok(None);
    }
    if !deduper.check_or_insert(hash) {
        log::info!("clipboard: dedupe hit (hash {hash:016x}); the same image is already hung");
        return Ok(None);
    }

    if let Err(error) = write_png_atomic(&path, &png) {
        // The hash was claimed before the write; a retry of the same image must not be swallowed.
        deduper.forget(hash);
        return Err(error);
    }
    apply_restrictive_permissions(&path);
    Ok(Some((path, hash)))
}

// ---------------------------------------------------------------------------------------------
// Win32 listener: a message-only window, with a 300 ms sequence-number poll as the fallback
// ---------------------------------------------------------------------------------------------

/// Poll interval of the `GetClipboardSequenceNumber` fallback (design spec 3.2 C).
pub const POLL_INTERVAL_MS: u32 = 300;

/// How long to let a clipboard owner finish publishing all formats before reading the copy.
///
/// The first notification of a multi-format copy can arrive while the owner is still writing (and
/// while it still holds the clipboard), so reading immediately both fails and can miss the text/
/// HTML markers that must veto the capture. See [`handle_if_changed`].
const CLIPBOARD_SETTLE_DELAY: Duration = Duration::from_millis(120);

/// Sleep slice that makes the window-less polling fallback stop promptly.
const POLL_SLICE_MS: u32 = 50;

/// Timer id used by the windowed polling fallback (`WM_TIMER`).
const POLL_TIMER_ID: usize = 1;

/// Number of `OpenClipboard` attempts: another process may briefly hold the clipboard open.
const OPEN_ATTEMPTS: usize = 20;

/// Delay between `OpenClipboard` attempts.
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);

/// Upper bound on enumerated clipboard formats (defensive against a hostile enumerator).
const MAX_FORMATS: usize = 1024;

/// Registered window class of the message-only listener window.
const WINDOW_CLASS: PCWSTR = w!("HanglineClipboardListener");

/// A running clipboard watcher; dropping it stops the listener thread and joins it.
pub struct ClipboardWatcher {
    /// Set on shutdown; read between messages and by the window-less polling loop.
    stop: Arc<AtomicBool>,
    /// HWND of the message-only window, stored as `isize` because `HWND` is not `Send`.
    window: Arc<AtomicIsize>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        // The stop flag covers the window-less polling fallback; the posted WM_CLOSE wakes the
        // blocking GetMessageW of the message-window path. If the window is already gone, posting
        // fails harmlessly and the thread is exiting anyway, so the join below cannot hang.
        self.stop.store(true, Ordering::SeqCst);
        let window = self.window.load(Ordering::SeqCst);
        if window != 0 {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(window as *mut c_void)),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::warn!("clipboard: the listener thread panicked");
            }
        }
        log::info!("clipboard: watcher stopped");
    }
}

/// Starts the clipboard listener on its own thread.
///
/// Only called when `settings.watchClipboard` is on (design spec 4.6 C). The thread reports setup
/// failures by logging and falls back to polling; the watcher never fails application startup. The
/// returned watcher keeps the thread alive for the application's lifetime and stops it on drop.
pub fn start(app: &AppHandle) -> anyhow::Result<ClipboardWatcher> {
    let app = app.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let window = Arc::new(AtomicIsize::new(0));
    let thread_stop = Arc::clone(&stop);
    let thread_window = Arc::clone(&window);
    let thread = thread::Builder::new()
        .name("hangline-clipboard".to_string())
        .spawn(move || run_listener(app, thread_stop, thread_window))
        .map_err(|error| {
            anyhow::anyhow!("failed to spawn the clipboard listener thread: {error}")
        })?;
    log::info!("clipboard: watching the clipboard (listener thread started)");
    Ok(ClipboardWatcher {
        stop,
        window,
        thread: Some(thread),
    })
}

/// Entry point of the listener thread: message window first, polling fallback otherwise.
fn run_listener(app: AppHandle, stop: Arc<AtomicBool>, window: Arc<AtomicIsize>) {
    match create_message_window() {
        Ok(hwnd) => {
            window.store(hwnd.0 as isize, Ordering::SeqCst);
            if stop.load(Ordering::SeqCst) {
                // Shutdown raced startup: do not enter the blocking message loop.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                window.store(0, Ordering::SeqCst);
                return;
            }

            let listener = unsafe { AddClipboardFormatListener(hwnd) };
            let polling = listener.is_err();
            match &listener {
                Ok(()) => log::info!(
                    "clipboard: listening for WM_CLIPBOARDUPDATE in a message-only window"
                ),
                Err(error) => {
                    log::warn!(
                        "clipboard: AddClipboardFormatListener failed ({error}); \
                         polling every {POLL_INTERVAL_MS} ms instead"
                    );
                    if unsafe { SetTimer(Some(hwnd), POLL_TIMER_ID, POLL_INTERVAL_MS, None) } == 0
                    {
                        log::warn!(
                            "clipboard: SetTimer failed; no clipboard updates will be observed"
                        );
                    }
                }
            }

            let mut last_sequence = unsafe { GetClipboardSequenceNumber() };
            message_loop(&app, listener.is_ok(), &mut last_sequence, &stop);

            unsafe {
                if listener.is_ok() {
                    let _ = RemoveClipboardFormatListener(hwnd);
                }
                if polling {
                    let _ = KillTimer(Some(hwnd), POLL_TIMER_ID);
                }
                let _ = DestroyWindow(hwnd);
            }
            window.store(0, Ordering::SeqCst);
            log::info!("clipboard: message window closed");
        }
        Err(error) => {
            log::warn!(
                "clipboard: message window unavailable ({error:#}); \
                 polling every {POLL_INTERVAL_MS} ms instead"
            );
            poll_forever(&app, &stop);
        }
    }
}

/// Creates the message-only window that receives `WM_CLIPBOARDUPDATE`.
fn create_message_window() -> anyhow::Result<HWND> {
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| anyhow::anyhow!("GetModuleHandleW failed: {error}"))?;
    let instance = HINSTANCE(module.0);
    let class = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON(std::ptr::null_mut()),
        hCursor: HCURSOR(std::ptr::null_mut()),
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: WINDOW_CLASS,
    };

    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        // A single-instance process can only hit this when the class is already registered in this
        // same process (e.g. a retried start); anything else is a real failure.
        let error = unsafe { GetLastError() };
        if error != ERROR_CLASS_ALREADY_EXISTS {
            bail!(
                "RegisterClassW failed: {}",
                std::io::Error::from_raw_os_error(error.0 as i32)
            );
        }
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("Hangline clipboard listener"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow::anyhow!("CreateWindowExW(HWND_MESSAGE) failed: {error}"))?;
    Ok(hwnd)
}

/// Pass-through window procedure: the message loop handles every message that matters.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Runs the `GetMessageW` loop until `WM_CLOSE`/`WM_DESTROY`/`WM_QUIT` or a shutdown.
fn message_loop(
    app: &AppHandle,
    listener: bool,
    last_sequence: &mut u32,
    stop: &AtomicBool,
) {
    let mut message = MSG::default();
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 == 0 || result.0 == -1 {
            // 0 = WM_QUIT was posted, -1 = GetMessageW failed; either way the loop is over.
            break;
        }
        match message.message {
            WM_CLIPBOARDUPDATE if listener => handle_if_changed(app, last_sequence),
            WM_TIMER if !listener => poll_once(app, last_sequence),
            WM_TIMER => {} // a stale timer from a previous mode: nothing to poll
            WM_CLOSE | WM_DESTROY | WM_QUIT => break,
            _ => {
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
    // Drain nothing: the clipboard is only read on a sequence change, never on shutdown.
}

/// Polls the clipboard sequence number once and processes a real change.
fn poll_once(app: &AppHandle, last_sequence: &mut u32) {
    let sequence = unsafe { GetClipboardSequenceNumber() };
    if sequence == 0 || sequence == *last_sequence {
        return;
    }
    *last_sequence = sequence;
    handle_clipboard(app);
}

/// Processes one clipboard notification after the owner has finished publishing it.
///
/// Windows posts `WM_CLIPBOARDUPDATE` for **every** clipboard change, including the intermediate
/// states an owner creates while it populates several formats (a browser writes `PNG` first, then
/// `HTML Format`/text). Reading the first notification would both race the owner (an observed
/// `GetClipboardData` failure while the owner was still publishing) and could see a format list
/// without the text/HTML markers that must veto the capture. A short settle delay makes the read
/// happen once the copy is complete; the sequence number taken *after* the delay collapses all the
/// queued notifications for that same final state into one read.
fn handle_if_changed(app: &AppHandle, last_sequence: &mut u32) {
    let before = unsafe { GetClipboardSequenceNumber() };
    if before != 0 && before == *last_sequence {
        return;
    }
    thread::sleep(CLIPBOARD_SETTLE_DELAY);
    let after = unsafe { GetClipboardSequenceNumber() };
    if after != 0 {
        // Updated before reading: a read failure must not make every later message retry it.
        *last_sequence = after;
    }
    handle_clipboard(app);
}

/// Polls the clipboard sequence number every [`POLL_INTERVAL_MS`] until shutdown.
///
/// Used only when no message window could be created at all. The loop sleeps in [`POLL_SLICE_MS`]
/// slices so dropping the watcher does not wait a whole interval.
fn poll_forever(app: &AppHandle, stop: &AtomicBool) {
    let mut last_sequence = unsafe { GetClipboardSequenceNumber() };
    while !stop.load(Ordering::SeqCst) {
        let mut waited = 0u32;
        while waited < POLL_INTERVAL_MS {
            if stop.load(Ordering::SeqCst) {
                log::info!("clipboard: polling stopped");
                return;
            }
            thread::sleep(Duration::from_millis(u64::from(POLL_SLICE_MS)));
            waited += POLL_SLICE_MS;
        }
        poll_once(app, &mut last_sequence);
    }
    log::info!("clipboard: polling stopped");
}

// ---------------------------------------------------------------------------------------------
// Reading one clipboard state
// ---------------------------------------------------------------------------------------------

/// One accepted clipboard image: the format names for logging plus the encoded image bytes.
struct ClipboardImage {
    /// Every format name seen in the clipboard state (names only, never contents).
    formats: Vec<String>,
    /// Which flavour was read: `PNG`, `CF_DIBV5` or `CF_DIB`.
    format: &'static str,
    /// The encoded image bytes.
    payload: Payload,
}

/// The encoded image bytes of the accepted flavour.
enum Payload {
    /// A self-describing PNG stream from the `PNG` registered format.
    Png(Vec<u8>),
    /// A `BITMAPINFO(HEADER|V4/V5HEADER)` payload from `CF_DIBV5`/`CF_DIB`.
    Dib(Vec<u8>),
}

impl ClipboardImage {
    /// Decodes the accepted flavour into RGBA pixels, enforcing the caps.
    fn decode(&self) -> anyhow::Result<RgbaImage> {
        match &self.payload {
            Payload::Png(bytes) => decode_png(bytes),
            Payload::Dib(bytes) => decode_dib(bytes),
        }
    }
}

/// Number of complete read attempts for one copy (open → filter → read the image bytes).
///
/// A read can lose the race with the clipboard owner or with another clipboard user:
/// `GetClipboardData` then fails with `ERROR_CLIPBOARD_NOT_OPEN` even though this thread had just
/// opened the clipboard successfully. Retrying the whole read (rather than a single call) also
/// re-runs the privacy filter against the settled format list, which is strictly safer.
const READ_ATTEMPTS: usize = 3;

/// Delay between complete clipboard read attempts.
const READ_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Outcome of one complete clipboard read attempt.
enum ClipboardRead {
    /// The copy passed the filter and its image bytes were read.
    Accepted(ClipboardImage),
    /// The privacy filter rejected the copy (final; no retry).
    Rejected {
        /// Format names for the log.
        formats: Vec<String>,
    },
    /// An image was advertised but its bytes could not be read (retryable).
    Incomplete {
        /// Format names for the log.
        formats: Vec<String>,
        /// Why the bytes could not be read (never clipboard contents).
        reason: String,
    },
}

/// An open clipboard, closed again on every path (including an early return).
struct ClipboardSession;

impl ClipboardSession {
    /// Opens the clipboard, retrying while another process holds it (copy operations are brief).
    fn open() -> anyhow::Result<Self> {
        let mut last_error = None;
        for attempt in 0..OPEN_ATTEMPTS {
            match unsafe { OpenClipboard(None) } {
                Ok(()) => return Ok(Self),
                Err(error) => {
                    last_error = Some(error);
                    if attempt + 1 < OPEN_ATTEMPTS {
                        thread::sleep(OPEN_RETRY_DELAY);
                    }
                }
            }
        }
        let error = last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown error".to_string());
        bail!("cannot open the clipboard after {OPEN_ATTEMPTS} attempts: {error}");
    }
}

impl Drop for ClipboardSession {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Reads the current clipboard and returns it when the privacy filter accepts it.
///
/// Everything happens inside one clipboard session: the format list, the history opt-out value and
/// the image bytes are all read while the clipboard is open, then the session is closed before the
/// image is decoded or saved (never hold the clipboard during I/O). Rejections are logged with the
/// format names only. A read that an advertised flavour cannot satisfy is retried a few times, so a
/// transient `ERROR_CLIPBOARD_NOT_OPEN` race does not silently drop a copy.
fn read_clipboard_image() -> anyhow::Result<Option<ClipboardImage>> {
    let mut last_incomplete = None;
    for attempt in 0..READ_ATTEMPTS {
        match read_clipboard_once()? {
            ClipboardRead::Accepted(image) => return Ok(Some(image)),
            ClipboardRead::Rejected { formats } => {
                if !formats.is_empty() {
                    log::info!("clipboard: ignored a copy (formats: {})", formats.join(", "));
                }
                return Ok(None);
            }
            ClipboardRead::Incomplete { formats, reason } => {
                log::debug!(
                    "clipboard: incomplete clipboard read (attempt {}/{}): {reason}",
                    attempt + 1,
                    READ_ATTEMPTS
                );
                last_incomplete = Some((formats, reason));
                if attempt + 1 < READ_ATTEMPTS {
                    thread::sleep(READ_RETRY_DELAY);
                }
            }
        }
    }
    if let Some((formats, reason)) = last_incomplete {
        log::warn!(
            "clipboard: the copy advertises an image but no readable flavour after \
             {READ_ATTEMPTS} attempts ({reason}; formats: {})",
            formats.join(", ")
        );
    }
    Ok(None)
}

/// One complete clipboard read attempt: open, enumerate, filter, read the image bytes.
fn read_clipboard_once() -> anyhow::Result<ClipboardRead> {
    let session = ClipboardSession::open()?;
    let entries: Vec<(u32, String)> = enumerate_formats()
        .into_iter()
        .filter_map(|format| format_name(format).map(|name| (format, name)))
        .collect();

    // Log the owner's real formats; the history opt-out is a derived decision marker, not a format
    // the owner published, so it must not leak into the human-readable list.
    let formats: Vec<String> = entries.iter().map(|(_, name)| name.clone()).collect();
    let mut decisions = formats.clone();
    if history_excluded(&entries) {
        decisions.push(HISTORY_EXCLUDED.to_string());
    }
    let decision_refs: Vec<&str> = decisions.iter().map(String::as_str).collect();
    if !should_accept(&decision_refs) {
        return Ok(ClipboardRead::Rejected { formats });
    }

    match read_image_payload(&entries) {
        Ok(Some((payload, format))) => {
            // Close the clipboard before decoding/encoding: another app may want it right away.
            drop(session);
            Ok(ClipboardRead::Accepted(ClipboardImage {
                formats,
                format,
                payload,
            }))
        }
        Ok(None) => Ok(ClipboardRead::Incomplete {
            formats,
            reason: "no image flavour is present".to_string(),
        }),
        Err(error) => Ok(ClipboardRead::Incomplete {
            formats,
            reason: format!("{error:#}"),
        }),
    }
}

/// Reads the preferred image flavour: `PNG`, then `CF_DIBV5`, then `CF_DIB`.
fn read_image_payload(
    entries: &[(u32, String)],
) -> anyhow::Result<Option<(Payload, &'static str)>> {
    const CANDIDATES: [(&str, &str); 3] = [
        ("PNG", "PNG"),
        ("CF_DIBV5", "CF_DIBV5"),
        ("CF_DIB", "CF_DIB"),
    ];
    let mut last_error = None;
    for (name, label) in CANDIDATES {
        let Some((id, _)) = entries
            .iter()
            .find(|(_, entry)| entry.eq_ignore_ascii_case(name))
        else {
            continue;
        };
        match read_format_bytes(*id) {
            Ok(bytes) => {
                let payload = if name == "PNG" {
                    Payload::Png(bytes)
                } else {
                    Payload::Dib(bytes)
                };
                return Ok(Some((payload, label)));
            }
            Err(error) => {
                log::debug!("clipboard: cannot read the {label} flavour: {error:#}");
                last_error = Some(error);
            }
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

/// Enumerates the clipboard format ids in the currently open clipboard.
fn enumerate_formats() -> Vec<u32> {
    let mut formats = Vec::new();
    let mut format = 0u32;
    loop {
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        formats.push(format);
        if formats.len() >= MAX_FORMATS {
            log::warn!("clipboard: more than {MAX_FORMATS} formats; ignoring the rest");
            break;
        }
    }
    formats
}

/// Maps a clipboard format id to its name (the predefined names, else the registered string).
fn format_name(format: u32) -> Option<String> {
    let predefined = match format {
        1 => Some("CF_TEXT"),
        2 => Some("CF_BITMAP"),
        3 => Some("CF_METAFILEPICT"),
        4 => Some("CF_SYLK"),
        5 => Some("CF_DIF"),
        6 => Some("CF_TIFF"),
        7 => Some("CF_OEMTEXT"),
        8 => Some("CF_DIB"),
        9 => Some("CF_PALETTE"),
        10 => Some("CF_PENDATA"),
        11 => Some("CF_RIFF"),
        12 => Some("CF_WAVE"),
        13 => Some("CF_UNICODETEXT"),
        14 => Some("CF_ENHMETAFILE"),
        15 => Some("CF_HDROP"),
        16 => Some("CF_LOCALE"),
        17 => Some("CF_DIBV5"),
        _ => None,
    };
    if let Some(name) = predefined {
        return Some(name.to_string());
    }
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClipboardFormatNameW(format, &mut buffer) };
    if length <= 0 {
        return None;
    }
    String::from_utf16(&buffer[..length as usize]).ok()
}

/// Copies one clipboard format's bytes out of its `HGLOBAL` (capped, never freed by us).
fn read_format_bytes(format: u32) -> anyhow::Result<Vec<u8>> {
    let handle = unsafe { GetClipboardData(format) }
        .map_err(|error| anyhow::anyhow!("GetClipboardData({format}) failed: {error}"))?;
    let hglobal = HGLOBAL(handle.0);
    let length = unsafe { GlobalSize(hglobal) };
    check_payload_len(length)?;
    let pointer = unsafe { GlobalLock(hglobal) };
    if pointer.is_null() {
        bail!("GlobalLock returned null for clipboard format {format}");
    }
    let mut bytes = vec![0u8; length];
    // SAFETY: the clipboard owns `length` initialized bytes at `pointer` (GlobalSize) and we copy
    // them before unlocking; the handle itself is never freed.
    unsafe {
        std::ptr::copy_nonoverlapping(pointer.cast::<u8>(), bytes.as_mut_ptr(), length);
        let _ = GlobalUnlock(hglobal);
    }
    Ok(bytes)
}

/// Reads a `DWORD`-valued clipboard format (used for `CanIncludeInClipboardHistory`).
fn read_format_dword(format: u32) -> Option<u32> {
    let handle = unsafe { GetClipboardData(format) }.ok()?;
    let hglobal = HGLOBAL(handle.0);
    if unsafe { GlobalSize(hglobal) } < 4 {
        return None;
    }
    let pointer = unsafe { GlobalLock(hglobal) };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the block is at least four bytes and global memory is suitably aligned.
    let value = unsafe { std::ptr::read_unaligned(pointer.cast::<u32>()) };
    unsafe {
        let _ = GlobalUnlock(hglobal);
    }
    Some(value)
}

/// Whether the copy opted out of clipboard history (present with DWORD 0, or unreadable).
fn history_excluded(entries: &[(u32, String)]) -> bool {
    let Some((id, _)) = entries
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case("CanIncludeInClipboardHistory"))
    else {
        return false;
    };
    match read_format_dword(*id) {
        Some(0) => {
            log::info!("clipboard: the copy opted out of clipboard history; not capturing it");
            true
        }
        Some(_) => false,
        None => {
            log::warn!(
                "clipboard: cannot read CanIncludeInClipboardHistory; rejecting conservatively"
            );
            true
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Production pipeline: decode, save, dedupe, add the item and broadcast
// ---------------------------------------------------------------------------------------------

/// Handles one clipboard change: read, filter, decode, save, dedupe, hang on the line.
fn handle_clipboard(app: &AppHandle) {
    let clipboard = match read_clipboard_image() {
        Ok(Some(clipboard)) => clipboard,
        Ok(None) => return,
        Err(error) => {
            log::warn!("clipboard: cannot read the clipboard: {error:#}");
            return;
        }
    };
    let image = match clipboard.decode() {
        Ok(image) => image,
        Err(error) => {
            log::warn!(
                "clipboard: cannot decode the copied image ({} flavour): {error:#}",
                clipboard.format
            );
            return;
        }
    };
    let (width, height) = (image.width(), image.height());
    log::info!(
        "clipboard: accepted a copy ({} flavour, {width}x{height}, formats: {})",
        clipboard.format,
        clipboard.formats.join(", ")
    );

    let dir = match capture::screenshots_dir() {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!("clipboard: the screenshot folder is unavailable: {error:#}");
            return;
        }
    };
    let state = app.state::<AppState>();
    let deduper = app.state::<Deduper>();
    let saved = ingest_image(&dir, &image, &deduper, &|path| state.contains_path(path));
    let (path, hash) = match saved {
        Ok(Some(saved)) => saved,
        Ok(None) => return,
        Err(error) => {
            log::warn!("clipboard: cannot save the copied image: {error:#}");
            return;
        }
    };

    let mut item = Item::new(path.clone(), ItemKind::Clipboard, hash, width, height);
    // P2-7: generate the preview before the first snapshot; a failure keeps the original path.
    crate::thumbnails::attach(&mut item);
    match state.add_item(item) {
        Ok(_) => {
            log::info!(
                "clipboard: hung {} (Clipboard) {width}x{height} hash {hash:016x}",
                path.display()
            );
            // The single serialized snapshot+broadcast choke point.
            let Some(snapshot) = app_state::emit_state(app) else {
                return;
            };
            // Design spec 3.4: a new item peeks the line when `revealOnNew` is on. This is a
            // Rust-only request to the reveal controller (Task 11); the controller emits the
            // frontend-facing `reveal { reason, peek }` event.
            if snapshot.settings.reveal_on_new {
                if let Err(error) = app.emit_to(
                    EventTarget::App,
                    "reveal-request",
                    serde_json::json!({ "reason": REVEAL_REASON_NEW_ITEM }),
                ) {
                    log::warn!("clipboard: failed to emit the reveal request: {error}");
                }
            }
        }
        Err(error) => {
            log::error!(
                "clipboard: failed to persist the new item for {}: {error:#}",
                path.display()
            );
            // `add_item` mutated memory before the persist failure: release the claimed hash so the
            // same image can be copied again, then republish the in-memory state.
            deduper.forget(hash);
            app_state::emit_state(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_pure_images_only() {
        // At least one image format (design spec 4.6 D rule 2).
        assert!(should_accept(&["PNG"]));
        assert!(should_accept(&["CF_DIBV5"]));
        assert!(should_accept(&["CF_DIB"]));
        assert!(should_accept(&["PNG", "CF_DIBV5", "CF_DIB"]));

        // Text or files anywhere in the payload (design spec 4.6 D rule 3).
        assert!(!should_accept(&["PNG", "CF_UNICODETEXT"]));
        assert!(!should_accept(&["PNG", "CF_TEXT"]));
        assert!(!should_accept(&["PNG", "CF_HDROP"]));
        assert!(!should_accept(&["CF_DIB", "CF_HDROP"]));

        // Monitor/history opt-outs.
        assert!(!should_accept(&[
            "PNG",
            "ExcludeClipboardContentFromMonitorProcessing"
        ]));
        assert!(!should_accept(&["PNG", "Clipboard Viewer Ignore"]));
        assert!(!should_accept(&["PNG", "org.nspasteboard.ConcealedType"]));

        // Browser copies always carry HTML Format next to the bitmap.
        assert!(!should_accept(&["PNG", "HTML Format"]));

        // Presence of `CanIncludeInClipboardHistory` alone is fine; DWORD 0 is not.
        assert!(should_accept(&["PNG", "CanIncludeInClipboardHistory"]));
        assert!(!should_accept(&["PNG", HISTORY_EXCLUDED]));

        // Nothing to capture.
        assert!(!should_accept(&[]));
        assert!(!should_accept(&["CF_BITMAP"]));
        assert!(!should_accept(&["CF_UNICODETEXT", "CF_TEXT"]));

        // GetClipboardFormatNameW is verbatim; compare ASCII case-insensitively.
        assert!(should_accept(&["png"]));
        assert!(!should_accept(&["PNG", "html format"]));
    }

    // ---------------------------------------------------------------------------------------
    // Synthetic DIB fixtures: a clipboard DIB is a BITMAPINFO(HEADER|V5HEADER) followed by
    // scanlines, with no BITMAPFILEHEADER. Rows are bottom-up unless the height is negative.
    // ---------------------------------------------------------------------------------------

    /// `BITMAPINFOHEADER` (40 bytes) with the fields the decoder reads.
    fn bitmap_info_header(
        width: i32,
        height: i32,
        bit_count: u16,
        compression: u32,
        clr_used: u32,
    ) -> Vec<u8> {
        let mut header = vec![0u8; 40];
        header[0..4].copy_from_slice(&40u32.to_le_bytes());
        header[4..8].copy_from_slice(&width.to_le_bytes());
        header[8..12].copy_from_slice(&height.to_le_bytes());
        header[12..14].copy_from_slice(&1u16.to_le_bytes()); // biPlanes
        header[14..16].copy_from_slice(&bit_count.to_le_bytes());
        header[16..20].copy_from_slice(&compression.to_le_bytes());
        header[32..36].copy_from_slice(&clr_used.to_le_bytes());
        header
    }

    /// `BITMAPV5HEADER` (124 bytes) with optional channel masks.
    fn bitmap_v5_header(
        width: i32,
        height: i32,
        bit_count: u16,
        compression: u32,
        masks: [u32; 4],
    ) -> Vec<u8> {
        let mut header = vec![0u8; 124];
        header[0..4].copy_from_slice(&124u32.to_le_bytes());
        header[4..8].copy_from_slice(&width.to_le_bytes());
        header[8..12].copy_from_slice(&height.to_le_bytes());
        header[12..14].copy_from_slice(&1u16.to_le_bytes());
        header[14..16].copy_from_slice(&bit_count.to_le_bytes());
        header[16..20].copy_from_slice(&compression.to_le_bytes());
        for (index, mask) in masks.iter().enumerate() {
            let start = 40 + index * 4;
            header[start..start + 4].copy_from_slice(&mask.to_le_bytes());
        }
        header
    }

    /// Rounds a scanline up to the DIB 4-byte stride.
    fn padded_row(row: &[u8]) -> Vec<u8> {
        let stride = (row.len() + 3) / 4 * 4;
        let mut padded = row.to_vec();
        padded.resize(stride, 0);
        padded
    }

    #[test]
    fn decodes_bottom_up_32bpp_rgb_forcing_opaque_alpha() {
        let mut dib = bitmap_info_header(2, 2, 32, 0, 0);
        // Stored bottom row first; GDI screenshots leave the alpha byte at 0.
        dib.extend_from_slice(&[1, 2, 3, 0, 4, 5, 6, 0]);
        dib.extend_from_slice(&[7, 8, 9, 0, 10, 11, 12, 0]);

        let image = decode_dib(&dib).expect("decode the 32bpp DIB");
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(
            image.as_raw(),
            &vec![
                9, 8, 7, 255, 12, 11, 10, 255, // top row (stored second)
                3, 2, 1, 255, 6, 5, 4, 255, // bottom row (stored first)
            ],
            "BGRA is swapped to RGBA, rows are flipped and a zero alpha becomes opaque"
        );
    }

    #[test]
    fn keeps_a_present_32bpp_alpha_channel() {
        // A negative height means the rows are stored top-down.
        let mut dib = bitmap_info_header(2, -1, 32, 0, 0);
        dib.extend_from_slice(&[10, 20, 30, 128, 1, 2, 3, 0]);

        let image = decode_dib(&dib).expect("decode the top-down 32bpp DIB");
        assert_eq!(
            image.as_raw(),
            &vec![30, 20, 10, 128, 3, 2, 1, 0],
            "a non-zero alpha channel is honoured pixel by pixel"
        );
    }

    #[test]
    fn decodes_24bpp_rows_with_stride_padding() {
        let mut dib = bitmap_info_header(3, 2, 24, 0, 0);
        // 3 pixels = 9 bytes, rounded to a 12-byte stride; padding must not become pixels.
        dib.extend_from_slice(&padded_row(&[1, 2, 3, 4, 5, 6, 7, 8, 9]));
        dib.extend_from_slice(&padded_row(&[10, 11, 12, 13, 14, 15, 16, 17, 18]));

        let image = decode_dib(&dib).expect("decode the 24bpp DIB");
        assert_eq!((image.width(), image.height()), (3, 2));
        assert_eq!(
            image.as_raw(),
            &vec![
                12, 11, 10, 255, 15, 14, 13, 255, 18, 17, 16, 255, // top row
                3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, // bottom row
            ]
        );
    }

    #[test]
    fn decodes_16bpp_rgb555_without_masks() {
        let mut dib = bitmap_info_header(2, 1, 16, 0, 0);
        dib.extend_from_slice(&0x7C00u16.to_le_bytes()); // 5-5-5 red
        dib.extend_from_slice(&0x001Fu16.to_le_bytes()); // 5-5-5 blue

        let image = decode_dib(&dib).expect("decode the 16bpp RGB555 DIB");
        assert_eq!(image.as_raw(), &vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn decodes_16bpp_565_bitfields() {
        // BITMAPINFOHEADER + BI_BITFIELDS carries its three masks right after the header.
        let mut dib = bitmap_info_header(2, 1, 16, 3, 0);
        for mask in [0xF800u32, 0x07E0, 0x001F] {
            dib.extend_from_slice(&mask.to_le_bytes());
        }
        dib.extend_from_slice(&0xF800u16.to_le_bytes()); // red
        dib.extend_from_slice(&0x07E0u16.to_le_bytes()); // green

        let image = decode_dib(&dib).expect("decode the 16bpp 565 DIB");
        assert_eq!(image.as_raw(), &vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn decodes_a_top_down_bitmapv5_with_masks() {
        // 0xAARRGGBB in memory, with an alpha mask; height < 0 = top-down.
        let dib = {
            let mut dib = bitmap_v5_header(
                2,
                -1,
                32,
                3,
                [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000],
            );
            dib.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF]); // A=FF R=00 G=00 B=FF (blue)
            dib.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // white
            dib
        };

        let image = decode_dib(&dib).expect("decode the BITMAPV5HEADER DIB");
        assert_eq!(
            image.as_raw(),
            &vec![0, 0, 255, 255, 255, 255, 255, 255],
            "BITMAPV5HEADER masks select the BGRA channels"
        );
    }

    #[test]
    fn rejects_unsupported_or_truncated_dibs() {
        assert!(
            decode_dib(&[]).is_err(),
            "an empty payload cannot be a DIB"
        );
        assert!(
            decode_dib(&bitmap_info_header(2, 2, 24, 0, 0)[..12]).is_err(),
            "a truncated header is rejected"
        );
        assert!(
            decode_dib(&bitmap_info_header(2, 2, 8, 0, 0)).is_err(),
            "8bpp paletted DIBs are not supported (spec whitelists images, not palettes)"
        );
        assert!(
            decode_dib(&bitmap_info_header(2, 2, 24, 0, 0)).is_err(),
            "a header without pixel data is rejected"
        );
        let mut core_header = bitmap_info_header(1, 1, 24, 0, 0);
        core_header[0..4].copy_from_slice(&12u32.to_le_bytes());
        assert!(
            decode_dib(&core_header).is_err(),
            "BITMAPCOREHEADER (12 bytes) is not a supported clipboard DIB"
        );
    }

    #[test]
    fn caps_pixels_at_eighty_megapixels_without_overflow() {
        assert!(check_pixel_cap(10_000, 8_000).is_ok(), "80 MP exactly is fine");
        assert!(check_pixel_cap(8_000, 10_000).is_ok());
        assert!(check_pixel_cap(10_000, 8_001).is_err(), "above 80 MP");
        assert!(
            check_pixel_cap(u32::MAX, u32::MAX).is_err(),
            "the cap check must not overflow a u32 product"
        );
        assert!(check_pixel_cap(1, 0).is_err(), "zero height");
        assert!(check_pixel_cap(0, 0).is_err(), "zero size");

        let huge = bitmap_info_header(100_000, 100_000, 32, 0, 0);
        assert!(
            decode_dib(&huge).is_err(),
            "a huge DIB header is rejected before any pixel allocation"
        );
    }

    #[test]
    fn caps_the_payload_at_sixty_four_mebibytes() {
        assert!(check_payload_len(0).is_err(), "an empty payload is useless");
        assert!(check_payload_len(MAX_PAYLOAD_BYTES).is_ok());
        assert!(check_payload_len(MAX_PAYLOAD_BYTES + 1).is_err());
    }

    #[test]
    fn png_decode_trims_block_rounded_trailing_bytes() {
        use base64::prelude::*;

        let png = BASE64_STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==")
            .expect("valid base64 PNG fixture");
        // Clipboard HGLOBALs are block-rounded, so the payload can carry trailing zero padding.
        let mut padded = png.clone();
        padded.extend_from_slice(&[0u8; 13]);

        assert_eq!(trim_png(&padded), &png[..], "trim at the IEND chunk");
        let image = decode_png(&padded).expect("decode the padded PNG");
        assert_eq!((image.width(), image.height()), (1, 1));
    }

    // ---------------------------------------------------------------------------------------
    // Save / dedupe pipeline
    // ---------------------------------------------------------------------------------------

    /// Scratch directory under `target/` (git-ignored); never touches the real data directory.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("clipboard-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the clipboard test dir");
        dir
    }

    #[test]
    fn save_name_is_local_time_to_the_second() {
        let local = SYSTEMTIME {
            wYear: 2026,
            wMonth: 10,
            wDay: 29,
            wHour: 16,
            wMinute: 5,
            wSecond: 3,
            wMilliseconds: 7,
            wDayOfWeek: 4,
        };
        assert_eq!(clipboard_stem(local), "Clipboard 2026-10-29 16.05.03");
    }

    #[test]
    fn same_second_saves_get_numbered_suffixes_instead_of_overwriting() {
        let dir = test_dir("names");
        let stem = "Clipboard 2026-10-29 16.05.03";
        assert_eq!(unique_path(&dir, stem), dir.join(format!("{stem}.png")));

        std::fs::write(dir.join(format!("{stem}.png")), b"first").expect("seed the first name");
        assert_eq!(
            unique_path(&dir, stem),
            dir.join(format!("{stem}-2.png")),
            "a second copy in the same second must not overwrite the first"
        );

        std::fs::write(dir.join(format!("{stem}-2.png")), b"second").expect("seed the second name");
        assert_eq!(unique_path(&dir, stem), dir.join(format!("{stem}-3.png")));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saves_once_per_content_and_skips_paths_that_are_already_hung() {
        let dir = test_dir("ingest");
        let deduper = Deduper::default();
        let image = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));

        let (path, hash) = ingest_image(&dir, &image, &deduper, &|_| false)
            .expect("save the first copy")
            .expect("the first copy is new");
        assert!(path.exists(), "the PNG must exist after a successful ingest");
        assert!(
            path.to_string_lossy()
                .starts_with(&dir.join("Clipboard ").to_string_lossy().to_string()),
            "the file uses the Clipboard <time>.png name: {}",
            path.display()
        );
        let saved = image::open(&path).expect("read the saved PNG").to_rgba8();
        assert_eq!((saved.width(), saved.height()), (3, 2));
        assert_eq!(saved.as_raw(), image.as_raw(), "the PNG round-trips exactly");
        assert_eq!(
            dedupe::hash_file(&path).expect("hash the saved file"),
            hash,
            "the reported hash is the BLAKE3-64 of the exact saved PNG bytes"
        );

        assert!(
            ingest_image(&dir, &image, &deduper, &|_| false)
                .expect("the duplicate is not an error")
                .is_none(),
            "the same content must not be saved twice"
        );
        assert_eq!(
            std::fs::read_dir(&dir).expect("list the test dir").count(),
            1,
            "a duplicate must not leave a second PNG behind"
        );

        let other = RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 4]));
        let (other_path, other_hash) = ingest_image(&dir, &other, &deduper, &|_| false)
            .expect("save the second image")
            .expect("different content is new");
        assert_ne!(other_path, path, "different content keeps its own file");
        assert_ne!(other_hash, hash);
        assert_eq!(
            std::fs::read_dir(&dir).expect("list the test dir").count(),
            2
        );

        // A path that another source already hung is skipped before the dedupe LRU is touched, so
        // the content is still ingestible through the clipboard once the path is gone.
        let untouched = Deduper::default();
        assert!(
            ingest_image(&dir, &image, &untouched, &|_| true)
                .expect("a hung path is not an error")
                .is_none()
        );
        assert!(
            ingest_image(&dir, &image, &untouched, &|_| false)
                .expect("the same content is still ingestible")
                .is_some(),
            "skipping an already-hung path must not poison the dedupe LRU"
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
