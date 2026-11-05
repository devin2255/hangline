//! First-frame extraction and duration for screen recordings (plan Task P3-5).
//!
//! Recording cards need the same 432 px preview as images, but the source is an `mp4`/`mov` that
//! the `image` crate cannot decode. This module asks the Windows media stack for the frame at
//! t = 0 and returns its pixels plus the clip metadata; [`crate::thumbnails`] then writes the
//! preview through the existing image-thumbnail writer (atomic, id-safe) and repairs the item
//! dimensions.
//!
//! # WinRT pipeline
//!
//! `Windows.Storage.StorageFile` → `Windows.Media.Editing.MediaClip::CreateFromFileAsync` →
//! `MediaComposition::GetThumbnailAsync(TimeSpan 0, fitted width, fitted height,
//! NearestFrame)` → `ImageStream` → `DataReader` bytes → `image` decode. The blocking `.join()`
//! wait is deliberate: the call only ever runs from the folder-watcher debounce thread or the
//! startup thumbnail backfill thread, never from the Tauri main/UI thread.
//!
//! # COM / WinRT apartment
//!
//! WinRT activation and the blocking async wait require an initialized COM apartment on the
//! calling thread. [`Apartment`] calls `CoInitializeEx(COINIT_MULTITHREADED)` and
//! `RoInitialize(RO_INIT_MULTITHREADED)`; every successful call (S_OK, or S_FALSE when another
//! part of the thread already initialized the same model) is balanced by one uninitialize. A
//! thread already in an STA is rejected with a clear error instead of hanging on the async wait.
//!
//! # Failure policy
//!
//! Every failure is an `anyhow::Error` carrying a [`VideoError`] with a user-readable message:
//! missing/unreadable file, COM unavailable, unsupported codec or DRM-protected file, empty or
//! undecodable first frame, and oversized decode. Nothing here panics; the caller keeps the
//! filename/play-badge fallback when `Ok` is not returned.
//!
//! # Runtime test
//!
//! The WinRT tests are `#[ignore]` because they need a real media runtime and a video file. Generate
//! the neutral fixture with `scripts/p3/fixture-video.ps1`, then (from the repository root):
//!
//! ```powershell
//! .\scripts\p3\fixture-video.ps1
//! $env:HANG_TEST_VIDEO = "$PWD\src-tauri\target\p3-fixtures\p3-recording-fixture.mp4"
//! cargo test --manifest-path src-tauri\Cargo.toml video:: -- --ignored --nocapture
//! ```

use std::fs;
use std::io::Cursor;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use image::RgbaImage;
use windows::core::HSTRING;
use windows::Foundation::TimeSpan;
use windows::Graphics::Imaging::ImageStream;
use windows::Media::Editing::{MediaClip, MediaComposition, VideoFramePrecision};
use windows::Storage::StorageFile;
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

/// One decoded first frame plus the clip metadata the card needs.
#[derive(Clone, Debug)]
pub struct VideoInfo {
    /// First frame, longest edge at most the requested `max_edge` (never upscaled).
    pub image: RgbaImage,
    /// Natural video width in pixels (`VideoEncodingProperties.Width`, frame fallback).
    pub width: u32,
    /// Natural video height in pixels (`VideoEncodingProperties.Height`, frame fallback).
    pub height: u32,
    /// Clip duration in milliseconds (`MediaClip.OriginalDuration`); the trim window uses it to
    /// clamp the selection and [`duration_ms`] exposes the same value without decoding a frame.
    pub duration_ms: u64,
}

/// Upper bound for the thumbnail stream handed back by `GetThumbnailAsync`.
///
/// A 432 px preview is a few dozen KiB; 16 MiB leaves room for an exotic encoder while refusing a
/// malformed stream that would otherwise be buffered in memory.
const MAX_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;

/// Upper bound for a decoded preview edge, in pixels.
const MAX_DECODE_EDGE: u32 = 8192;

/// Upper bound for decoded preview pixels (guards a decompression bomb).
const MAX_DECODE_PIXELS: u64 = 64 * 1024 * 1024;

/// A clear, caller-facing first-frame/duration error.
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    /// The path is not an existing regular file.
    #[error("video file is missing or is not a regular file: {0}")]
    Missing(PathBuf),
    /// The file exists but its metadata could not be read.
    #[error("cannot read video file metadata: {0}")]
    Unreadable(String),
    /// `max_edge` was zero (the requested preview would be empty).
    #[error("invalid preview edge {0}: the longest edge must be greater than zero")]
    InvalidMaxEdge(u32),
    /// COM/WinRT could not be initialized on the calling thread.
    #[error("COM/WinRT is not available on the calling thread: {0}")]
    Apartment(String),
    /// The media stack refused the file (codec, DRM, container or access problem).
    #[error("unsupported video codec, DRM-protected or unreadable file: {0}")]
    Unsupported(String),
    /// A WinRT media object/operation failed outside the file-open step.
    #[error("Windows media operation failed: {0}")]
    Media(String),
    /// The clip produced no usable first frame.
    #[error("the video has no decodable first frame: {0}")]
    NoFrame(String),
    /// The preview bytes or decoded pixels exceed the safety limits.
    #[error("the video preview is too large to decode: {0}")]
    TooLarge(String),
}

/// Extracts the frame at t = 0, scaled to fit `max_edge` on its longest edge.
///
/// The returned `width`/`height` are the video's natural pixel dimensions (not the scaled frame),
/// and `duration_ms` comes from `MediaClip.OriginalDuration`. The caller must be a worker thread:
/// this blocks on WinRT async operations and initializes an MTA on it.
pub fn first_frame(path: &Path, max_edge: u32) -> anyhow::Result<VideoInfo> {
    if max_edge == 0 {
        return Err(VideoError::InvalidMaxEdge(max_edge).into());
    }
    ensure_readable(path)?;

    let _apartment = Apartment::init()?;
    let clip = load_clip(path)?;
    let duration_ms = clip_duration_ms(&clip)?;
    // WinRT's thumbnail service renders into the requested rectangle (the 640x360 fixture returns
    // 432x244 for a 432x243 request), so requesting the video's own aspect ratio avoids distortion.
    let natural = match natural_dimensions(&clip) {
        Ok(dimensions) => Some(dimensions),
        Err(error) => {
            log::warn!(
                "video: cannot read natural dimensions of {}: {error:#}; requesting a square frame",
                path.display()
            );
            None
        }
    };
    let (request_width, request_height) = match natural {
        Some((width, height)) => fit_within(width, height, max_edge),
        // Aspect ratio unknown: request one dimension only (height 0 = keep the source aspect).
        // Requesting a square here would render the frame distorted before `decode_preview` sees it.
        None => (max_edge, 0),
    };

    let composition = MediaComposition::new().map_err(|error| {
        VideoError::Media(format!("cannot activate MediaComposition: {error}"))
    })?;
    composition
        .Clips()
        .and_then(|clips| clips.Append(&clip))
        .map_err(|error| {
            VideoError::Media(format!("cannot add the clip to the composition: {error}"))
        })?;

    // `max_edge` is validated as a small positive number by every caller (432 for cards); the
    // clamp keeps the i32 WinRT parameter well-defined even for an absurd hand-called value.
    // A zero height is WinRT's documented "derive it from the width" request, used only for the
    // unknown-aspect fallback above; every other call passes both dimensions.
    let edge_width = i32::try_from(request_width.max(1)).unwrap_or(i32::MAX);
    let edge_height = i32::try_from(request_height).unwrap_or(i32::MAX);
    let operation = composition
        .GetThumbnailAsync(
            TimeSpan { Duration: 0 },
            edge_width,
            edge_height,
            VideoFramePrecision::NearestFrame,
        )
        .map_err(|error| {
            VideoError::Media(format!("cannot request the first frame: {error}"))
        })?;
    let stream = operation.join().map_err(|error| {
        VideoError::NoFrame(format!(
            "GetThumbnailAsync failed (empty video, unsupported codec or DRM?): {error}"
        ))
    })?;

    let bytes = read_preview_bytes(&stream)?;
    let image = decode_preview(&bytes, max_edge)?;

    // Natural dimensions drive the card aspect ratio; if the encoding properties are unavailable
    // (unusual container), the scaled frame still keeps the correct aspect ratio.
    let (width, height) = match natural {
        Some(dimensions) => dimensions,
        None => (image.width(), image.height()),
    };

    Ok(VideoInfo {
        image,
        width,
        height,
        duration_ms,
    })
}

/// Returns the clip duration in milliseconds without decoding a frame (P3-6 trim reuses this).
pub fn duration_ms(path: &Path) -> anyhow::Result<u64> {
    ensure_readable(path)?;
    let _apartment = Apartment::init()?;
    let clip = load_clip(path)?;
    clip_duration_ms(&clip)
}

/// Fits `(width, height)` inside a `max_edge` square, preserving aspect ratio and never upscaling.
///
/// Returns `(0, 0)` for a zero/unknown dimension or a zero edge. The longest edge of a non-empty
/// result is exactly `min(max_edge, longest source edge)`; each axis is at least 1 px so a very
/// thin source cannot round away.
pub(crate) fn fit_within(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    if width == 0 || height == 0 || max_edge == 0 {
        return (0, 0);
    }
    let longest = width.max(height);
    if longest <= max_edge {
        return (width, height);
    }
    let scale = f64::from(max_edge) / f64::from(longest);
    let scaled_width = ((f64::from(width) * scale).round() as u32)
        .max(1)
        .min(max_edge);
    let scaled_height = ((f64::from(height) * scale).round() as u32)
        .max(1)
        .min(max_edge);
    (scaled_width, scaled_height)
}

/// Rejects a missing/unreadable file before any WinRT work, so the common error path is cheap.
fn ensure_readable(path: &Path) -> anyhow::Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(VideoError::Missing(path.to_path_buf()).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(VideoError::Missing(path.to_path_buf()).into())
        }
        Err(error) => Err(VideoError::Unreadable(format!("{}: {error}", path.display())).into()),
    }
}

/// RAII COM/WinRT apartment for one worker-thread call (shared with the trim exporter).
///
/// Both initializers are idempotent per thread: S_FALSE ("already initialized") still needs one
/// matching uninitialize, while `RPC_E_CHANGED_MODE` means another model owns the thread and is
/// reported instead of waiting on an async operation that can never complete without a message
/// pump.
pub(crate) struct Apartment {
    /// `CoInitializeEx` succeeded (S_OK or S_FALSE) and must be balanced.
    co_owned: bool,
    /// `RoInitialize` succeeded (S_OK or S_FALSE) and must be balanced.
    ro_owned: bool,
}

impl Apartment {
    /// Initializes an MTA and the Windows Runtime on the current thread.
    pub(crate) fn init() -> anyhow::Result<Self> {
        // SAFETY: CoInitializeEx takes no pointers and only changes the current thread's COM state.
        let co = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if co == RPC_E_CHANGED_MODE {
            return Err(VideoError::Apartment(
                "the calling thread is already in a single-threaded COM apartment; first-frame \
                 extraction needs a multi-threaded worker thread"
                    .to_string(),
            )
            .into());
        }
        if co.is_err() {
            return Err(VideoError::Apartment(format!(
                "CoInitializeEx(COINIT_MULTITHREADED) failed: {}",
                co.message()
            ))
            .into());
        }

        // SAFETY: RoInitialize has no pointers and initializes the Windows Runtime on this thread.
        if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            // Undo the COM initialization before reporting: the caller will not get a guard.
            // SAFETY: the successful CoInitializeEx above is balanced here exactly once.
            unsafe { CoUninitialize() };
            return Err(VideoError::Apartment(format!(
                "RoInitialize(RO_INIT_MULTITHREADED) failed: {}",
                error.message()
            ))
            .into());
        }

        Ok(Self {
            co_owned: true,
            ro_owned: true,
        })
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.ro_owned {
            // SAFETY: balanced against the successful RoInitialize in `init`.
            unsafe { RoUninitialize() };
        }
        if self.co_owned {
            // SAFETY: balanced against the successful CoInitializeEx in `init` (including S_FALSE).
            unsafe { CoUninitialize() };
        }
    }
}

/// Resolves `path` to an absolute WinRT `StorageFile` and creates its `MediaClip`.
pub(crate) fn load_clip(path: &Path) -> anyhow::Result<MediaClip> {
    let hstring = wide_path(path)?;
    let file = StorageFile::GetFileFromPathAsync(&hstring)
        .map_err(|error| {
            VideoError::Unsupported(format!("cannot open {}: {error}", path.display()))
        })?
        .join()
        .map_err(|error| {
            VideoError::Unsupported(format!("cannot read {}: {error}", path.display()))
        })?;
    let clip = MediaClip::CreateFromFileAsync(&file)
        .map_err(|error| {
            VideoError::Unsupported(format!(
                "cannot create a media clip for {}: {error}",
                path.display()
            ))
        })?
        .join()
        .map_err(|error| {
            VideoError::Unsupported(format!(
                "the media runtime refused {} (unsupported codec or DRM?): {error}",
                path.display()
            ))
        })?;
    Ok(clip)
}

/// Encodes a Windows path as a NUL-free UTF-16 `HSTRING` (lossless for non-UTF-8 names).
pub(crate) fn wide_path(path: &Path) -> anyhow::Result<HSTRING> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) {
        return Err(VideoError::Unreadable(format!("invalid path: {}", path.display())).into());
    }
    Ok(HSTRING::from_wide(&wide))
}

/// Reads the whole `ImageStream` returned by `GetThumbnailAsync`, bounded by [`MAX_PREVIEW_BYTES`].
fn read_preview_bytes(stream: &ImageStream) -> anyhow::Result<Vec<u8>> {
    let size = stream.Size().map_err(|error| {
        VideoError::Media(format!("cannot size the first-frame stream: {error}"))
    })?;
    if size == 0 {
        return Err(VideoError::NoFrame("the first-frame stream is empty".to_string()).into());
    }
    if size > MAX_PREVIEW_BYTES {
        return Err(VideoError::TooLarge(format!(
            "{size} bytes from GetThumbnailAsync (limit {MAX_PREVIEW_BYTES})"
        ))
        .into());
    }

    let input = stream.GetInputStreamAt(0).map_err(|error| {
        VideoError::Media(format!("cannot open the first-frame stream: {error}"))
    })?;
    let reader = DataReader::CreateDataReader(&input).map_err(|error| {
        VideoError::Media(format!("cannot create a data reader for the frame: {error}"))
    })?;

    let mut bytes = Vec::with_capacity(size as usize);
    let mut remaining = size;
    while remaining > 0 {
        let chunk = remaining.min(u64::from(u32::MAX)) as u32;
        let loaded = reader
            .LoadAsync(chunk)
            .and_then(|operation| operation.join())
            .map_err(|error| {
                VideoError::Media(format!("cannot read the first-frame stream: {error}"))
            })?;
        if loaded == 0 {
            break;
        }
        let start = bytes.len();
        let end = start + loaded as usize;
        bytes.resize(end, 0);
        reader.ReadBytes(&mut bytes[start..end]).map_err(|error| {
            VideoError::Media(format!("cannot copy the first-frame bytes: {error}"))
        })?;
        remaining = remaining.saturating_sub(u64::from(loaded));
    }

    if bytes.is_empty() {
        return Err(
            VideoError::NoFrame("the first-frame stream returned no bytes".to_string()).into(),
        );
    }
    if bytes.len() < size as usize {
        return Err(VideoError::NoFrame(format!(
            "the first-frame stream ended early ({} of {size} bytes)",
            bytes.len()
        ))
        .into());
    }
    Ok(bytes)
}

/// Decodes the PNG/JPEG bytes from the media stream, capped and fitted to `max_edge`.
fn decode_preview(bytes: &[u8], max_edge: u32) -> anyhow::Result<RgbaImage> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| {
            VideoError::NoFrame(format!(
                "cannot identify the first-frame image format: {error}"
            ))
        })?;
    let (encoded_width, encoded_height) = reader.into_dimensions().map_err(|error| {
        VideoError::NoFrame(format!(
            "cannot read the first-frame image dimensions: {error}"
        ))
    })?;
    if encoded_width == 0 || encoded_height == 0 {
        return Err(VideoError::NoFrame(format!(
            "the first-frame image is {encoded_width}x{encoded_height}"
        ))
        .into());
    }
    if encoded_width > MAX_DECODE_EDGE
        || encoded_height > MAX_DECODE_EDGE
        || u64::from(encoded_width) * u64::from(encoded_height) > MAX_DECODE_PIXELS
    {
        return Err(VideoError::TooLarge(format!(
            "{encoded_width}x{encoded_height} (limit {MAX_DECODE_EDGE} px edge / \
             {MAX_DECODE_PIXELS} pixels)"
        ))
        .into());
    }

    let decoded = image::load_from_memory(bytes)
        .map_err(|error| {
            VideoError::NoFrame(format!("cannot decode the first-frame image: {error}"))
        })?
        .to_rgba8();
    let (width, height) = fit_within(decoded.width(), decoded.height(), max_edge);
    if width == 0 || height == 0 {
        return Err(VideoError::NoFrame(format!(
            "the first-frame image is {}x{}",
            decoded.width(),
            decoded.height()
        ))
        .into());
    }
    if (width, height) != (decoded.width(), decoded.height()) {
        return Ok(image::imageops::thumbnail(&decoded, width, height));
    }
    Ok(decoded)
}

/// Natural video pixel dimensions from the clip's video encoding properties.
fn natural_dimensions(clip: &MediaClip) -> anyhow::Result<(u32, u32)> {
    let properties = clip.GetVideoEncodingProperties().map_err(|error| {
        VideoError::Media(format!("cannot read the video encoding properties: {error}"))
    })?;
    let width = properties.Width().map_err(|error| {
        VideoError::Media(format!("cannot read the video width: {error}"))
    })?;
    let height = properties.Height().map_err(|error| {
        VideoError::Media(format!("cannot read the video height: {error}"))
    })?;
    if width == 0 || height == 0 {
        return Err(VideoError::NoFrame(format!(
            "the video reports {width}x{height} pixels"
        ))
        .into());
    }
    Ok((width, height))
}

/// Clip duration in milliseconds (WinRT `TimeSpan` is 100 ns ticks).
pub(crate) fn clip_duration_ms(clip: &MediaClip) -> anyhow::Result<u64> {
    let span = clip.OriginalDuration().map_err(|error| {
        VideoError::Media(format!("cannot read the clip duration: {error}"))
    })?;
    if span.Duration <= 0 {
        return Ok(0);
    }
    Ok((span.Duration / 10_000) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    /// Scratch directory under `target/` (git-ignored); never touches user folders.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("video-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the video test dir");
        dir
    }

    #[test]
    fn fit_within_preserves_aspect_and_never_upscales() {
        assert_eq!(fit_within(4000, 3000, 432), (432, 324));
        assert_eq!(fit_within(3000, 4000, 432), (324, 432));
        assert_eq!(fit_within(640, 360, 432), (432, 243));
        assert_eq!(fit_within(320, 200, 432), (320, 200));
        assert_eq!(fit_within(432, 432, 432), (432, 432));
    }

    #[test]
    fn fit_within_keeps_thin_sources_at_least_one_pixel_wide() {
        assert_eq!(fit_within(1, 1000, 432), (1, 432));
        assert_eq!(fit_within(1000, 1, 432), (432, 1));
        assert_eq!(fit_within(3, 2, 1), (1, 1));
    }

    #[test]
    fn fit_within_handles_degenerate_inputs() {
        assert_eq!(fit_within(0, 100, 432), (0, 0));
        assert_eq!(fit_within(100, 0, 432), (0, 0));
        assert_eq!(fit_within(100, 100, 0), (0, 0));
        assert_eq!(fit_within(0, 0, 0), (0, 0));
    }

    #[test]
    fn missing_video_is_a_clear_error() {
        let dir = test_dir("missing");
        let path = dir.join("does-not-exist.mp4");
        let error = first_frame(&path, 432).expect_err("a missing file must be an error");
        assert!(
            error.to_string().contains("missing"),
            "the error must name the missing file: {error}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_video_duration_is_a_clear_error() {
        let dir = test_dir("duration-missing");
        let path = dir.join("does-not-exist.mp4");
        let error = duration_ms(&path).expect_err("a missing file must be an error");
        assert!(
            error.to_string().contains("missing"),
            "the error must name the missing file: {error}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_max_edge_is_rejected_before_touching_the_media_stack() {
        let dir = test_dir("zero-edge");
        let path = dir.join("does-not-exist.mp4");
        let error = first_frame(&path, 0).expect_err("a zero edge must be rejected");
        assert!(
            error.to_string().contains("invalid preview edge"),
            "the error must explain the edge: {error}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn decode_preview_fits_a_valid_frame_to_the_edge() {
        let image = RgbaImage::from_pixel(640, 360, image::Rgba([30, 111, 115, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode the test frame");
        let decoded = decode_preview(&png, 432).expect("decode the test frame");
        assert_eq!((decoded.width(), decoded.height()), (432, 243));
    }

    #[test]
    fn decode_preview_rejects_oversized_headers_before_decoding() {
        // A 9000 px edge exceeds MAX_DECODE_EDGE; the header check must refuse it before the
        // decoder allocates the full pixel buffer.
        let image = RgbaImage::from_pixel(9000, 1, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode the oversized frame");
        let error = decode_preview(&png, 432).expect_err("an oversized frame must be rejected");
        assert!(
            error.to_string().contains("too large"),
            "the error must explain the size limit: {error}"
        );
    }

    #[test]
    fn decode_preview_rejects_non_image_bytes() {
        let error = decode_preview(b"not a png", 432).expect_err("garbage must not decode");
        assert!(
            error.to_string().contains("no decodable first frame"),
            "the error must classify the decode failure: {error}"
        );
    }

    #[test]
    #[ignore = "needs HANG_TEST_VIDEO=<a real mp4>; see the module docs"]
    fn first_frame_decodes_a_real_video() {
        let Some(raw) = std::env::var_os("HANG_TEST_VIDEO") else {
            panic!(
                "set HANG_TEST_VIDEO to a real 3 s 640x360 mp4 (see scripts/p3/fixture-video.ps1)"
            );
        };
        let info = first_frame(Path::new(&raw), 432).expect("decode the first frame");
        assert!(
            info.width > 0 && info.height > 0,
            "natural dimensions must be known"
        );
        assert!(
            (2850..=3150).contains(&info.duration_ms),
            "the 3 s fixture must report ≈3000 ms (container rounding), got {}",
            info.duration_ms
        );
        assert!(
            info.image.width().max(info.image.height()) <= 432,
            "the preview must fit 432 px: {}x{}",
            info.image.width(),
            info.image.height()
        );
        // Aspect ratio is preserved within rounding (432x243 for 640x360).
        let frame_aspect = f64::from(info.image.width()) / f64::from(info.image.height());
        let video_aspect = f64::from(info.width) / f64::from(info.height);
        assert!(
            (frame_aspect - video_aspect).abs() < 0.01,
            "frame {frame_aspect:.4} vs video {video_aspect:.4}"
        );
        // The duration helper must agree with the frame call (same clip metadata).
        assert_eq!(
            duration_ms(Path::new(&raw)).expect("duration"),
            info.duration_ms
        );
    }

    #[test]
    #[ignore = "needs the Windows media runtime; a corrupt file must not panic"]
    fn corrupt_video_returns_a_clear_error() {
        let dir = test_dir("corrupt");
        let path = dir.join("corrupt.mp4");
        fs::write(&path, b"not really an mp4").expect("write the corrupt file");
        let error = first_frame(&path, 432).expect_err("a corrupt file must be an error");
        let message = error.to_string();
        assert!(
            message.contains("unsupported") || message.contains("no decodable first frame"),
            "the error must classify the media failure: {message}"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
