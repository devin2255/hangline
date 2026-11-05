/**
 * P3-6 trim window: `<video>` preview, dual start/end sliders and the WinRT export trigger.
 *
 * The page is opened by `open_trim` (`index.html?window=trim&id=…&theme=…&lang=…`) and receives
 * its metadata from `get_trim_target`: the item, its file size, the clip duration from the Windows
 * media stack and both backend limits (2 GiB / 60 min). The duration is authoritative — it is the
 * same value `trim_video` validates against — so the slider maximum comes from it rather than the
 * `<video>` metadata (which is only a fallback when the backend reported 0).
 *
 * Save is explicit: no dirty confirmation is needed, because closing the window discards nothing
 * until the user presses Trim. The export listens for the coarse `trim-progress` milestones and
 * closes the window only after `trim_video` resolved; a failure stays on the page with the error
 * and a "Open in default player" fallback so the user can still watch the untouched original.
 */

import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ChangeEvent } from "react";
import { assetUrl } from "../../lib/assetUrl";
import { t } from "../../lib/i18n";
import { closeTrim, getTrimTarget, openItem, reportTrimReady, trimVideo } from "../../lib/ipc";
import { fileNameOf } from "../../lib/layout";
import { initAppState, useAppStore } from "../../lib/state";
import { themeSettingFromLocation, useResolvedTheme } from "../../lib/theme";
import {
  canSaveTrim,
  clampEnd,
  clampStart,
  formatTrimTime,
  selectionMs,
  shouldCloseAfterSave,
  snapToStep,
  trimRefusal,
} from "../../lib/trim";
import type { TrimProgressEvent, TrimTarget } from "../../lib/types";
import "./Trim.css";

/** Coarse milestone the backend emits when the export starts (module docs in `trim.rs`). */
const PROGRESS_START = 0.05;

/** The trim page. */
export function Trim() {
  // Separate webview: the URL carries theme/language for a correct first paint; the state
  // subscription keeps them live if the settings change while the window is open.
  const appState = useAppStore((store) => store.state);
  const themeSetting = appState?.settings.theme ?? themeSettingFromLocation();
  const theme = useResolvedTheme(themeSetting);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    return () => {
      delete document.documentElement.dataset.theme;
    };
  }, [theme]);

  useEffect(() => {
    initAppState();
  }, []);

  const [targetId, setTargetId] = useState(
    () => new URLSearchParams(location.search).get("id") ?? "",
  );
  const [target, setTarget] = useState<TrimTarget | null>(null);
  const [targetError, setTargetError] = useState<string | null>(null);
  const [durationMs, setDurationMs] = useState(0);
  const [startMs, setStartMs] = useState(0);
  const [endMs, setEndMs] = useState(0);
  const [videoReady, setVideoReady] = useState(false);
  const [videoFailed, setVideoFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [progress, setProgress] = useState(0);
  /** A rejection of the export itself (the error panel + player fallback). */
  const [saveError, setSaveError] = useState<string | null>(null);
  /** Detail of the last export failure (hover title; full text is also logged). */
  const [saveErrorDetail, setSaveErrorDetail] = useState("");

  const videoRef = useRef<HTMLVideoElement | null>(null);
  /** Latest target id, so the progress listener can ignore events for another item. */
  const targetIdRef = useRef(targetId);
  targetIdRef.current = targetId;
  /** Latest duration, read by the `<video>` metadata fallback without a stale closure. */
  const durationRef = useRef(durationMs);
  durationRef.current = durationMs;
  const mountedRef = useRef(true);
  const savingRef = useRef(false);
  /**
   * Monotonic load generation. `get_trim_target` resolves asynchronously, so a slow reply for the
   * previous id must never overwrite the item a later `trim-target` switched to (P3-4 review
   * follow-up: the same guard now also protects the markup editor).
   */
  const loadGeneration = useRef(0);
  /** True once the page requested a close; avoids a duplicate `close_trim`. */
  const closingRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  /** Loads one item into the trim page (initial open and a `trim-target` switch). */
  const load = useCallback((id: string) => {
    const generation = ++loadGeneration.current;
    if (!id) {
      setTarget(null);
      setTargetError(t("Recording failed to load"));
      return;
    }
    setTarget(null);
    setTargetError(null);
    setDurationMs(0);
    setStartMs(0);
    setEndMs(0);
    setVideoReady(false);
    setVideoFailed(false);
    setSaving(false);
    savingRef.current = false;
    setProgress(0);
    setSaveError(null);
    setSaveErrorDetail("");
    void getTrimTarget(id).then(
      (next) => {
        // A stale reply (the window was re-targeted while this call was in flight) is dropped.
        if (!mountedRef.current || generation !== loadGeneration.current) return;
        if (next.item.id !== id) {
          setTargetError(t("Recording failed to load"));
          return;
        }
        setTarget(next);
        // The 10 ms grid can only express whole steps; the backend duration stays authoritative.
        const duration = snapToStep(Math.max(0, next.durationMs));
        setDurationMs(duration);
        setStartMs(0);
        setEndMs(duration);
      },
      (error: unknown) => {
        if (!mountedRef.current || generation !== loadGeneration.current) return;
        console.warn(`Hangline trim: cannot load item ${id}`, error);
        setTargetError(t("Recording failed to load"));
      },
    );
  }, []);

  useEffect(() => {
    load(targetId);
  }, [load, targetId]);

  // A reused window switches items instead of navigating. The listener is registered before the
  // ready handshake; Rust replays a target emitted while the page was still loading.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const stop = await listen<string>("trim-target", ({ payload }) => {
        if (disposed || typeof payload !== "string" || !payload) return;
        // A retarget while `trim_video` runs would change the item the save closes over (and let a
        // second export start); Rust refuses it too, this is the page-side half of the contract.
        if (savingRef.current) {
          console.warn(`Hangline trim: ignoring trim-target ${payload} during an export`);
          return;
        }
        setTargetId(payload);
      });
      if (disposed) {
        stop();
        return;
      }
      unlisten = stop;
      try {
        const last = await reportTrimReady();
        if (!disposed && last && last !== targetIdRef.current) {
          if (savingRef.current) {
            console.warn(`Hangline trim: ignoring replayed trim-target ${last} during an export`);
          } else {
            setTargetId(last);
          }
        }
      } catch (error) {
        console.warn("Hangline trim: ready handshake unavailable", error);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Coarse export progress (0.05 start / 0.6 rendered / 1.0 replaced); only the open item counts.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<TrimProgressEvent>("trim-progress", ({ payload }) => {
      if (disposed || !payload || payload.id !== targetIdRef.current) return;
      if (typeof payload.progress !== "number" || !Number.isFinite(payload.progress)) return;
      setProgress(Math.min(1, Math.max(0, payload.progress)));
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error: unknown) => {
        console.warn("Hangline trim: progress listener unavailable", error);
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** Seeks the preview so dragging a slider shows the frame the selection boundary lands on. */
  const seekPreview = useCallback((milliseconds: number) => {
    const video = videoRef.current;
    if (!video || !Number.isFinite(video.duration) || video.duration <= 0) return;
    const seconds = Math.min(Math.max(0, milliseconds / 1000), video.duration);
    try {
      video.currentTime = seconds;
    } catch {
      // A metadata-less video ignores the seek; the slider labels are still correct.
    }
  }, []);

  const handleStartChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const next = clampStart(Number(event.currentTarget.value), endMs, durationMs);
      setStartMs(next);
      seekPreview(next);
    },
    [durationMs, endMs, seekPreview],
  );

  const handleEndChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const next = clampEnd(Number(event.currentTarget.value), startMs, durationMs);
      setEndMs(next);
      seekPreview(next);
    },
    [durationMs, seekPreview, startMs],
  );

  const handleLoadedMetadata = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (!(video.duration > 0)) {
      setVideoReady(false);
      setVideoFailed(true);
      return;
    }
    setVideoReady(true);
    setVideoFailed(false);
    // The backend duration is authoritative; metadata is only a fallback when it reported 0.
    if (!(durationRef.current > 0)) {
      const duration = snapToStep(video.duration * 1000);
      setDurationMs(duration);
      setStartMs(0);
      setEndMs(duration);
    }
  }, []);

  /** Closes the window; a running export must never be cancelled mid-render. */
  const requestCancel = useCallback(() => {
    if (savingRef.current || closingRef.current) return;
    closingRef.current = true;
    void closeTrim().catch((error: unknown) => {
      closingRef.current = false;
      console.warn("Hangline trim: close_trim failed", error);
    });
  }, []);

  /** Exports the selected range and closes the window on success. */
  const save = useCallback(async () => {
    if (!target || savingRef.current) return;
    const currentDuration = durationMs;
    if (!canSaveTrim(startMs, endMs, currentDuration)) return;
    savingRef.current = true;
    setSaving(true);
    setProgress(PROGRESS_START);
    setSaveError(null);
    setSaveErrorDetail("");
    try {
      await trimVideo(target.item.id, startMs, endMs);
      if (!mountedRef.current) return;
      if (!shouldCloseAfterSave(target.item.id, targetIdRef.current)) {
        // A retarget raced the export: the window now shows another item, so closing it would hide
        // that item instead of returning to the line. The exported item still refreshed in place.
        console.warn(
          `Hangline trim: ${target.item.id} was trimmed, but the window now shows ` +
            `${targetIdRef.current}; leaving it open`,
        );
        savingRef.current = false;
        setSaving(false);
        setProgress(0);
        return;
      }
      closingRef.current = true;
      await closeTrim();
    } catch (error) {
      if (!mountedRef.current) return;
      console.warn(`Hangline trim: export of ${target.item.path} failed`, error);
      savingRef.current = false;
      setSaving(false);
      setSaveError(t("Trim failed"));
      setSaveErrorDetail(typeof error === "string" ? error : String(error));
    }
  }, [durationMs, endMs, startMs, target]);

  // Esc cancels the window (there is no dirty state to confirm).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      requestCancel();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [requestCancel]);

  const refusal = target
    ? trimRefusal(target.sizeBytes, target.durationMs, target.maxBytes, target.maxDurationMs)
    : null;
  const selection = selectionMs(startMs, endMs);
  const rangeValid = canSaveTrim(startMs, endMs, durationMs);
  const savable = Boolean(target) && !refusal && rangeValid && !saving;
  /**
   * Both sliders span the whole timeline, so their thumbs line up with the selection bar. The
   * handlers clamp the values: a start dragged past `end − 110 ms` stops at the minimum, an end
   * dragged below `start + 110 ms` stops there instead (Rust rejects `end − start ≤ 100`).
   */
  const slidersDisabled = !target || saving || durationMs <= 0;
  const imageUrl = target ? assetUrl(target.item.path, target.item.updatedAt) : "";
  const fileName = target ? fileNameOf(target.item.path) : "";
  const refusalMessage =
    refusal === "tooLarge"
      ? t("File is too large to trim")
      : refusal === "tooLong"
        ? t("Recording is too long to trim")
        : null;

  return (
    <div
      className="trim"
      data-window="trim"
      data-target-id={target?.item.id ?? ""}
      data-duration-ms={durationMs}
      data-start-ms={startMs}
      data-end-ms={endMs}
      data-selection-ms={selection}
      data-video-ready={videoReady ? "true" : "false"}
      data-video-failed={videoFailed ? "true" : "false"}
      data-saving={saving ? "true" : "false"}
      data-progress={String(progress)}
      data-refused={refusal ?? "false"}
      data-range-valid={rangeValid ? "true" : "false"}
      data-error={saveError ? "true" : "false"}
    >
      <header className="trim__titlebar" data-tauri-drag-region>
        <span className="trim__title" data-tauri-drag-region>
          {t("Trim recording")}
          {fileName ? <span className="trim__filename"> · {fileName}</span> : null}
        </span>
        <button
          type="button"
          className="trim__close"
          title={saving ? t("Trimming…") : t("Cancel")}
          aria-label={t("Cancel")}
          onClick={requestCancel}
          disabled={saving}
          data-action="close"
        >
          ✕
        </button>
      </header>

      <div className="trim__stage">
        {target ? (
          <>
            <video
              key={target.item.id}
              ref={videoRef}
              className="trim__video"
              src={imageUrl}
              preload="metadata"
              playsInline
              data-video-path={target.item.path}
              onLoadedMetadata={handleLoadedMetadata}
              onError={() => {
                setVideoReady(false);
                setVideoFailed(true);
              }}
            />
            {!videoReady && !videoFailed ? (
              <div className="trim__overlay">{t("Loading…")}</div>
            ) : null}
            {videoFailed ? (
              <div className="trim__overlay trim__overlay--error">
                <span>{t("Recording failed to load")}</span>
                <span className="trim__path">{target.item.path}</span>
              </div>
            ) : null}
          </>
        ) : (
          <div
            className={
              targetError ? "trim__overlay trim__overlay--error" : "trim__overlay"
            }
          >
            {targetError ?? t("Loading…")}
          </div>
        )}
      </div>

      <div className="trim__controls">
        <div className="trim__timeline" aria-hidden="true">
          <div
            className="trim__selection-bar"
            style={{
              left: durationMs > 0 ? `${(startMs / durationMs) * 100}%` : "0%",
              width: durationMs > 0 ? `${(selection / durationMs) * 100}%` : "0%",
            }}
          />
        </div>

        <label className="trim__slider-row">
          <span className="trim__slider-label">{t("Start")}</span>
          <input
            className="trim__slider"
            type="range"
            min={0}
            max={Math.max(durationMs, 0)}
            step={10}
            value={startMs}
            disabled={slidersDisabled}
            onChange={handleStartChange}
            aria-label={t("Start")}
            data-slider="start"
          />
          <output className="trim__time" data-start-label>
            {formatTrimTime(startMs)}
          </output>
        </label>

        <label className="trim__slider-row">
          <span className="trim__slider-label">{t("End")}</span>
          <input
            className="trim__slider"
            type="range"
            min={0}
            max={Math.max(durationMs, 0)}
            step={10}
            value={endMs}
            disabled={slidersDisabled}
            onChange={handleEndChange}
            aria-label={t("End")}
            data-slider="end"
          />
          <output className="trim__time" data-end-label>
            {formatTrimTime(endMs)}
          </output>
        </label>

        <div className="trim__summary">
          <span className="trim__summary-item">
            {t("Duration")}: <strong data-selection-label>{formatTrimTime(selection)}</strong>
          </span>
          <span className="trim__summary-item trim__summary-item--muted">
            {formatTrimTime(0)} – {formatTrimTime(durationMs)}
          </span>
        </div>

        {refusalMessage ? (
          <p className="trim__refusal" data-refusal={refusal ?? undefined}>
            {refusalMessage}
          </p>
        ) : null}
        {!refusalMessage && target && !rangeValid ? (
          <p className="trim__refusal" data-refusal="range">
            {t("Invalid trim range")}
          </p>
        ) : null}
        {saveError ? (
          <div className="trim__error" data-save-error="true" title={saveErrorDetail}>
            <span>{saveError}</span>
            <button
              type="button"
              className="trim__button"
              onClick={() => {
                if (!target) return;
                void openItem(target.item.id).catch((error: unknown) => {
                  console.warn("Hangline trim: cannot open the original in its default app", error);
                });
              }}
              data-action="open-player"
            >
              {t("Open in default player")}
            </button>
          </div>
        ) : null}

        {saving ? (
          <div className="trim__progress" data-progress-bar>
            <div
              className="trim__progress-fill"
              style={{ width: `${Math.round(progress * 100)}%` }}
            />
            <span className="trim__progress-label">
              {t("Trimming…")} {Math.round(progress * 100)}%
            </span>
          </div>
        ) : null}
      </div>

      <footer className="trim__footer">
        <span className="trim__hint">{t("Select the part to keep")}</span>
        <button
          type="button"
          className="trim__button"
          onClick={requestCancel}
          disabled={saving}
          data-action="cancel"
        >
          {t("Cancel")}
        </button>
        <button
          type="button"
          className="trim__button trim__button--primary"
          onClick={() => void save()}
          disabled={!savable}
          data-action="save"
        >
          {saving ? t("Trimming…") : t("Trim")}
        </button>
      </footer>
    </div>
  );
}
