/**
 * TypeScript mirror of the Rust data model (`src-tauri/src/item.rs`, `settings.rs`,
 * `app_state.rs`).
 *
 * Rust serializes every struct with `#[serde(rename_all = "camelCase")]`, so these interfaces are
 * field-for-field camelCase copies of the spec 4.5 / 3.6 shapes. Keeping them in one module means
 * the IPC layer, the Zustand store and the panel agree on the wire format.
 */

/** An axis-aligned rectangle in physical pixels (Rust `geometry::PhysicalRect`). */
export interface PhysicalRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * One monitor of the virtual desktop, in physical pixels (Rust `geometry::MonitorInfo`).
 *
 * `workArea` excludes taskbars/appbars and is what the panel is placed against; `scaleFactor`
 * is the effective DPI divided by 96 (1.0 = 100%, 1.5 = 150%).
 */
export interface MonitorInfo {
  /** GDI device name (e.g. `\\.\DISPLAY1`); stable across `get_monitors` calls. */
  id: string;
  /** Full monitor rectangle. */
  rect: PhysicalRect;
  /** Monitor rectangle minus taskbars/appbars. */
  workArea: PhysicalRect;
  /** Effective DPI scale (physical px per logical px). */
  scaleFactor: number;
  isPrimary: boolean;
}

/** Window size in physical pixels (Rust `PixelSize`, returned by `sync_panel_geometry`). */
export interface PixelSize {
  width: number;
  height: number;
}

/** Window origin in physical virtual-desktop pixels (Rust `PixelPosition`). */
export interface PixelPosition {
  x: number;
  y: number;
}

/**
 * Result of the `sync_panel_geometry` command (Task 14).
 *
 * `monitors` is the full virtual desktop, `target` is the monitor Rust chose (the one the panel
 * window is on for the command, with primary/first fallbacks) and `size`/`position` are the panel
 * window geometry it applied (or the geometry that already matched; both are `null` when the
 * window does not exist yet). `moved` is false when the window already had that geometry.
 */
export interface PanelGeometry {
  monitors: MonitorInfo[];
  target: MonitorInfo | null;
  size: PixelSize | null;
  position: PixelPosition | null;
  moved: boolean;
}

/** Provenance of a hung item (design spec 4.5). */
export type ItemKind =
  | "capture"
  | "systemScreenshot"
  | "clipboard"
  | "recording"
  | "watchedFolder";

/**
 * Side of a card for the batch take-down (`take_down_side`, Wave 1.1).
 *
 * `"before"` removes the older cards (visually left of the target), `"after"` the newer ones; the
 * target card itself stays on the line and the files are never deleted.
 */
export type TakeDownSide = "before" | "after";

/** How many cards stay on the line (spec 3.6: `"fit"` or 25/50/100). */
export type KeepOnLine = "fit" | 25 | 50 | 100;

/** Age-retention policy (design extension `keepForDays`): `"forever"` or 7/30/90 days. */
export type KeepForDays = "forever" | 7 | 30 | 90;

/** Card scale (spec 3.6 `cardSize`). */
export type CardSize = "small" | "medium" | "large";

/** Colour theme (spec 3.6 `theme`). */
export type Theme = "system" | "light" | "dark";

/**
 * One item hung on the line (design spec 4.5; Rust `Item`).
 *
 * `hash` is a 64-bit BLAKE3 truncation: it is a backend dedupe key, so the frontend must treat it
 * as opaque (values above `Number.MAX_SAFE_INTEGER` lose precision in JSON parsing).
 */
export interface Item {
  /** Opaque 32-hex-character id (backend-generated; never parse it). */
  id: string;
  /** Absolute path of the file on disk. */
  path: string;
  /**
   * P2-7 cached thumbnail path (`%LOCALAPPDATA%\Hangline\Thumbnails\<id>.png`). Recordings use
   * their P3-5 first-frame preview here; absent only for undecodable sources, in which case the
   * card falls back to the original image or the filename tile (recordings keep the play badge).
   */
  thumbPath?: string | null;
  kind: ItemKind;
  /** 64-bit content hash (backend dedupe key, opaque to the UI). */
  hash: number;
  /** Pixel width (0 until a video's first frame is decoded). */
  width: number;
  /** Pixel height (0 until a video's first frame is decoded). */
  height: number;
  /** File creation time, milliseconds since the Unix epoch. */
  createdAt: number;
  /** Time the item was hung, milliseconds since the Unix epoch. */
  addedAt: number;
  /**
   * Time Hangline last rewrote the file in place (markup save / trim), milliseconds since the Unix
   * epoch. `0`/missing for items persisted before P3; the value is the asset cache key
   * (`assetUrl(path, updatedAt)` appends `?v=`).
   */
  updatedAt?: number;
}

/** The 18 design-spec user settings plus the `keepForDays` age-retention extension (Rust `Settings`). */
export interface Settings {
  /** Global capture accelerator, normalized (`Ctrl+Alt+S`). */
  captureHotkey: string;
  /** Global show/hide accelerator, normalized (`Ctrl+Alt+T`). */
  toggleHotkey: string;
  /**
   * Wave 1 third global hotkey: opens the markup editor for the newest item (normalized, `""` =
   * unbound). Mirrors Rust `edit_latest_hotkey`.
   */
  editLatestHotkey: string;
  /** Wave 1.5: also put the finished capture on the clipboard (in addition to saving/hanging it). */
  captureCopyOnFinish: boolean;
  watchSystemScreenshots: boolean;
  watchClipboard: boolean;
  watchRecordings: boolean;
  /** Extra watched folders, at most 3 absolute unique paths. */
  extraFolders: string[];
  revealOnNew: boolean;
  /** Peek duration in ms (0–60000). */
  revealPeekMs: number;
  topEdgeReveal: boolean;
  showOverFullScreen: boolean;
  keepOnLine: KeepOnLine;
  /** Age retention: `"forever"` (default) or keep the last 7/30/90 days. */
  keepForDays: KeepForDays;
  cardSize: CardSize;
  sound: boolean;
  startAtLogin: boolean;
  /** UI language tag; `"system"` follows the OS. */
  language: string;
  theme: Theme;
  autoCheckUpdates: boolean;
  /** ISO 8601 timestamp of the last update check; `""` = never. */
  lastUpdateCheck: string;
}

/** Full state returned by `get_app_state` and broadcast by the `state` event (spec 4.4). */
export interface AppState {
  /** Items ordered by `addedAt` ascending (oldest left, newest right). */
  items: Item[];
  settings: Settings;
  /** Application version (Cargo/package version). */
  version: string;
  /**
   * Monotonic revision, incremented by Rust on every items/settings mutation.
   *
   * A lower revision is a stale full state that must not replace the store; equal revisions carry
   * the same state (the backend increments once per mutation). See `shouldApplyState` in
   * `state.ts`.
   */
  revision: number;
  /**
   * The language both the Rust tray and the panel must render: `"en"` or `"zh-Hans"`.
   *
   * Task 15 review fix: Rust resolves `settings.language` (including `"system"` — Windows display
   * language first, then locale, then English) exactly once and broadcasts the result. The frontend
   * must use this field instead of resolving `"system"` from `navigator.language`, otherwise a
   * mixed-locale Windows install can show a Chinese tray with an English panel.
   */
  resolvedLanguage: string;
}

/** Every valid `set_setting` key. */
export type SettingsKey = keyof Settings;

/** Why the line was revealed (Rust `reveal` event, design spec §3.4). */
export type RevealReason = "newItem" | "toggle" | "topEdge" | "secondInstance";

/** Why the line was tucked (Rust `tuck` event, design spec §3.4). */
export type TuckReason = "pointerLeave" | "toggle" | "empty" | "dismiss" | "fullscreen";

/** Payload of the Rust → panel `reveal` event (Task 11). */
export interface RevealEvent {
  /** What summoned the line. */
  reason: RevealReason;
  /** Whether a timed peek (2.5 s by default) is driving the reveal. */
  peek: boolean;
}

/** Payload of the Rust → panel `tuck` event (Task 11). */
export interface TuckEvent {
  /** What hid the line. */
  reason: TuckReason;
}

/**
 * Payload of the Rust → capture-overlay `capture-ready` event (Task 16, `src-tauri/src/capture.rs`).
 *
 * Emitted after the overlay frontend's ready handshake. `tempPath` points at the frozen frame in
 * `%LOCALAPPDATA%\Hangline` (render it with `convertFileSrc`); `virtualRect` is the captured
 * virtual-screen rectangle in physical pixels (negative origin on a left/above monitor) and
 * `scale` the overlay window's scale factor. The component uses `window.devicePixelRatio` for the
 * CSS↔physical conversion and `virtualRect` as the physical origin (never assumes 100%).
 */
export interface CaptureReady {
  /** Absolute path of the frozen frame PNG (inside the asset-protocol scope). */
  tempPath: string;
  /** Captured virtual-screen rectangle in physical pixels. */
  virtualRect: PhysicalRect;
  /** Overlay window scale factor (informational; `devicePixelRatio` is authoritative). */
  scale: number;
  /** Frozen-frame width in physical pixels. */
  width: number;
  /** Frozen-frame height in physical pixels. */
  height: number;
  /**
   * Wave 1.6 "空窗补按": the physical virtual-screen point where the hotkey's left-button press
   * happened, when the button was still held as the overlay became ready. The overlay converts it
   * to CSS pixels and starts the drag from it instead of waiting for a `pointerdown` that was
   * consumed before the window existed. `null`/absent means a normal capture.
   */
  pressedAt?: PixelPosition | null;
}

/** Result of the `finish_capture` command (Wave 1.5 return contract). */
export interface CaptureFinishResult {
  /** True when the capture was saved to disk and hung on the line. */
  saved: boolean;
  /**
   * True when saving failed but the image was copied to the clipboard instead. The overlay closes
   * in both cases; this only distinguishes the fallback for diagnostics (no extra UI).
   */
  copiedFallback: boolean;
}

/**
 * Payload of the Rust → panel `drag-ended` event (Task 13, `src-tauri/src/dragout.rs`).
 *
 * Rust emits it once per drag session, for drops, cancels and failures alike. `effect` is the
 * final OLE `DROPEFFECT`; the panel calls `after_drag` with it for every ending and Rust's
 * effect-driven policy decides whether the card leaves the line (a `move` whose file is still
 * present is re-checked for a slow shell move).
 */
export interface DragEndedEvent {
  /** Monotonic Rust drag-session token; absent in payloads from older builds. */
  sessionId?: number;
  /** False when the drag failed, panicked, or was aborted by the Rust watchdog. */
  ok?: boolean;
  effect: "copy" | "move" | "link" | "none";
  /** True when OLE reported a completed drop (not a cancel). */
  dropped?: boolean;
  /** Present when the drag could not run or the watchdog broke a stuck OLE loop. */
  error?: string;
}

/** Which overlay animation to play (`fly` = new card in, `fall` = taken-down card out). */
export type FlyKind = "fly" | "fall";

/**
 * One active fly/fall overlay animation (Rust `fly.rs::FlyAnimation`, design spec §4.4 / P2-5).
 *
 * `from`/`to` are virtual-desktop physical pixels; the overlay page converts them to CSS pixels
 * with `cssScale = innerWidth / virtualRect.w`. `tiltDeg` is the panel's deterministic card tilt
 * (the overlay interpolates towards it) and `durationMs` comes from the animation kind (650/550).
 */
export interface FlyAnimation {
  /** Opaque item id: the `fly-landed` payload key and the overlay's `fly_done` argument. */
  id: string;
  kind: FlyKind;
  /** Absolute path of the original image the overlay renders. */
  imagePath: string;
  /**
   * `Item.updatedAt` of the file (P3): the overlay builds `assetUrl(imagePath, imageVersion)` so an
   * edited card never animates with pre-edit bytes from the WebView cache.
   */
  imageVersion: number;
  /** Start rectangle on the virtual screen (physical px). */
  from: PhysicalRect;
  /** End rectangle on the virtual screen (physical px). */
  to: PhysicalRect;
  /** Final card tilt in degrees. */
  tiltDeg: number;
  /** Animation length in ms (650 fly / 550 fall). */
  durationMs: number;
  /**
   * DPI scale of the monitor the card lands on (1.0 = 100 %, 1.5 = 150 %).
   *
   * Physical positions/sizes are exact already; the overlay derives its chrome dp scale from
   * `targetScale / devicePixelRatio` so arc/inset/radius/pin/fall distance stay physically correct
   * on mixed-DPI desktops.
   */
  targetScale: number;
}

/** Payload of the Rust → fly-overlay `fly-animation` event. */
export interface FlyAnimationEvent {
  animations: FlyAnimation[];
}

/** Payload of the Rust → panel `fly-landed` event: the card may become visible again. */
export interface FlyLandedEvent {
  id: string;
}

/** Resolved visual theme the fly overlay renders with (`settings.theme`, `"system"` resolved). */
export type FlyTheme = "light" | "dark";

/** Raw theme setting returned by the overlay ready handshake (the page resolves `"system"`). */
export type FlyThemeSetting = "system" | "light" | "dark";

/**
 * Result of the fly overlay ready handshake (`report_fly_ready`).
 *
 * The page uses `virtualRect` for `cssScale = innerWidth / virtualRect.w` and `theme` for the
 * glass material; `theme` is the raw setting so the page can follow `prefers-color-scheme` when it
 * is `"system"` (the same resolution the panel uses).
 */
export interface FlyReady {
  virtualRect: PhysicalRect;
  theme: FlyThemeSetting;
}

/** Payload of the Rust → fly-overlay `fly-geometry` event (display change / new flight start). */
export interface FlyGeometryEvent {
  virtualRect: PhysicalRect;
  theme: FlyThemeSetting;
}


/** Payload of the Rust → frontend `panel-geometry` event (P2-6 review). */
export interface PanelGeometryEvent {
  /** Panel window physical origin x on the virtual screen. */
  x: number;
  /** Panel window physical origin y on the virtual screen. */
  y: number;
  /** Target monitor scale factor (informational). */
  scale: number;
}

/**
 * Result of `get_editor_target` (P3, Rust `annotate::EditorTarget`).
 *
 * `owned` is true only for files inside `%LOCALAPPDATA%\Hangline\Screenshots`; watched-folder items
 * open read-only (Save disabled in the UI, and the backend rejects the write again). `theme` is the
 * raw setting (`system`/`light`/`dark`) carried in the window URL.
 */
export interface EditorTarget {
  item: Item;
  owned: boolean;
  theme: "system" | "light" | "dark";
}

/**
 * Payload of the Rust → panel `editor-state` event (P3-4).
 *
 * `open` is true while the single markup editor window exists. The panel reports this as busy so
 * the reveal controller never tucks the line while the user is editing.
 */
export interface EditorStateEvent {
  open: boolean;
}

/**
 * Result of `get_trim_target` (P3-6, Rust `trim::TrimTarget`).
 *
 * `durationMs` comes from `MediaClip::OriginalDuration` (authoritative for the backend's range
 * validation), so the page uses it for the slider maximum instead of the `<video>` metadata. The
 * `max*` fields mirror the backend caps; when `sizeBytes`/`durationMs` exceed them the page
 * disables Save and explains why (the command rejects the export again).
 */
export interface TrimTarget {
  item: Item;
  /** Current file size in bytes. */
  sizeBytes: number;
  /** Clip duration in milliseconds from the media stack. */
  durationMs: number;
  /** Backend file-size limit (2 GiB). */
  maxBytes: number;
  /** Backend duration limit (60 min). */
  maxDurationMs: number;
  /** Raw `settings.theme` (`system`/`light`/`dark`); the page resolves `system` itself. */
  theme: Theme;
}

/**
 * Payload of the Rust → trim page `trim-progress` event (P3-6/P3-7).
 *
 * Coarse milestones only: 0.05 at export start, 0.6 once the render produced the temp file, 1.0
 * after the original was atomically replaced. WinRT's render progress handler is not granular
 * enough across codecs, so the backend emits these guaranteed steps instead.
 */
export interface TrimProgressEvent {
  /** Item being exported (ignore events for another id). */
  id: string;
  /** Milestone in `[0, 1]`. */
  progress: number;
}

/** Payload of the Rust → panel `trim-state` event (P3-6 busy model). */
export interface TrimStateEvent {
  /** True while the single trim window exists. */
  open: boolean;
}

/**
 * Result of `get_editor_state` (P3-4 review follow-up).
 *
 * A panel webview that reloaded while an editor was already open never received the creation
 * event; it reconciles the shared busy flag from these live window-registry values on mount.
 */
export interface EditorStateSnapshot {
  annotateOpen: boolean;
  trimOpen: boolean;
}

/**
 * Result of `check_updates` and payload of the Rust → panel `update-available` event (P5-2).
 *
 * `manual` records how the check was triggered; `pubDate` is the manifest's `pub_date` verbatim
 * (ISO 8601, `null` when the release omits it). The app never downloads on its own: an
 * announcement only enables the panel's explicit `Install update` action.
 */
export interface UpdateInfo {
  /** Announced version (e.g. `0.4.1`). */
  version: string;
  /** Release notes from the manifest, when present. */
  notes: string | null;
  /** Manifest `pub_date` (ISO 8601), when present. */
  pubDate: string | null;
  /** True when a manual check produced this announcement. */
  manual: boolean;
}

/**
 * Payload of the Rust → panel `update-progress` event (P5-2).
 *
 * `chunk` is cumulative bytes downloaded so far (not the size of one network chunk) and `total`
 * is the server-declared content length when it provided one.
 */
export interface UpdateProgressEvent {
  chunk: number;
  total: number | null;
}

/** Payload of the Rust → panel `update-check-started` event (manual checks only). */
export interface UpdateCheckStartedEvent {
  manual: boolean;
}

/** Payload of the Rust → panel `update-check-finished` event (manual check found no update). */
export interface UpdateCheckFinishedEvent {
  available: boolean;
}

/** Payload of the Rust → panel `update-check-failed` event (manual checks only). */
export interface UpdateCheckFailedEvent {
  /** Diagnostic detail; the panel shows a translated generic message. */
  message: string;
}
