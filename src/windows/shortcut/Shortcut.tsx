/**
 * The minimal change-shortcut window (design spec §4.2 / Task 15).
 *
 * Opened from the tray's Shortcut submenu with `?window=shortcut&target=capture|toggle`. The window
 * captures the next key combination, validates it with the same grammar as the Rust accelerator
 * parser, saves it through `set_hotkey` (persist + re-register + rollback on conflict) and closes.
 * Esc cancels; Enter saves the captured combination; a backend rejection (e.g. a conflict with
 * another application) is shown inline and the old shortcut stays registered.
 */

import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { t } from "../../lib/i18n";
import { closeShortcutWindow, setHotkey } from "../../lib/ipc";
import { initAppState, useAppStore } from "../../lib/state";
import { themeSettingFromLocation, useResolvedTheme } from "../../lib/theme";
import {
  acceleratorFromEvent,
  parseShortcutTarget,
  type CaptureFailure,
  type ShortcutTarget,
} from "./capture";
import "./Shortcut.css";

/** Which shortcut this window edits (W1.8 adds the third one). */
type Target = ShortcutTarget;



/** Human message for a capture failure (all labels come from the shared i18n table). */
function failureMessage(reason: CaptureFailure): string {
  switch (reason) {
    case "modifier":
      return t("Keep holding the modifiers and press another key");
    case "unknown-key":
      return t("That key is not supported by the shortcut parser");
    case "too-few-modifiers":
      return t("At least two modifiers (Ctrl/Alt/Shift/Win) and one key");
  }
}

export function Shortcut() {
  const query = new URLSearchParams(location.search);
  /** P2-8: capture/shortcut windows get the raw setting from their URL. */
  const theme = useResolvedTheme(themeSettingFromLocation());
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    return () => {
      delete document.documentElement.dataset.theme;
    };
  }, [theme]);
  const [target, setTarget] = useState<Target>(() => parseShortcutTarget(query.get("target")));
  const state = useAppStore((store) => store.state);
  const [captured, setCaptured] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const current =
    target === "capture"
      ? state?.settings.captureHotkey
      : target === "toggle"
        ? state?.settings.toggleHotkey
        : state?.settings.editLatestHotkey;
  const targetLabel =
    target === "capture"
      ? t("Capture")
      : target === "toggle"
        ? t("Show/hide line")
        : t("Edit latest");

  // Full-state handshake so the window can show the current accelerator and follow a language
  // switch while it is open (the store picks the i18n table before re-rendering).
  useEffect(() => {
    initAppState();
  }, []);

  // A reused window is re-targeted by the tray without being recreated.
  useEffect(() => {
    const pending = listen<string>("shortcut-target", ({ payload }) => {
      setTarget(parseShortcutTarget(payload));
      setCaptured(null);
      setError(null);
    });
    return () => {
      void pending.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  const save = useCallback(
    async (accelerator: string) => {
      if (saving) return;
      setSaving(true);
      setError(null);
      try {
        await setHotkey(target, accelerator);
        await closeShortcutWindow();
      } catch (reason) {
        setSaving(false);
        setError(reason instanceof Error ? reason.message : String(reason));
      }
    },
    [saving, target],
  );

  // Capture the key press at the window level: the backend grammar is mirrored by `capture.ts`,
  // and Escape/Enter keep their window-level meaning (cancel/save) so a combination can never
  // trap the user inside the dialog.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.code === "Escape") {
        void closeShortcutWindow().catch(() => undefined);
        return;
      }
      if (event.code === "Enter" || event.code === "NumpadEnter") {
        if (captured && !saving) void save(captured);
        return;
      }
      const result = acceleratorFromEvent(event);
      if (result.ok) {
        setCaptured(result.accelerator);
        setError(null);
      } else {
        setCaptured(null);
        setError(failureMessage(result.reason));
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [captured, save, saving]);

  return (
    <div className="shortcut" data-target={target}>
      <header className="shortcut__header">
        <h1 className="shortcut__title">{t("Change shortcut")}</h1>
        <p className="shortcut__target">
          {targetLabel}
          {current ? (
            <>
              {" · "}
              <span className="shortcut__current" data-current={current}>
                {t("Current shortcut")}: {current}
              </span>
            </>
          ) : null}
        </p>
      </header>

      <div
        className="shortcut__capture"
        data-captured={captured ?? ""}
        data-error={error ? "true" : "false"}
        aria-live="polite"
      >
        <span className="shortcut__keys">{captured ?? t("Press the new shortcut combination")}</span>
        <span className="shortcut__hint">
          {error ?? t("At least two modifiers (Ctrl/Alt/Shift/Win) and one key")}
        </span>
      </div>

      <footer className="shortcut__actions">
        <span className="shortcut__esc-hint">{t("Press Enter to save, Esc to cancel")}</span>
        <button
          type="button"
          className="shortcut__button"
          onClick={() => void closeShortcutWindow().catch(() => undefined)}
        >
          {t("Cancel")}
        </button>
        <button
          type="button"
          className="shortcut__button shortcut__button--primary"
          disabled={!captured || saving}
          onClick={() => {
            if (captured) void save(captured);
          }}
        >
          {saving ? t("Saving…") : t("Save")}
        </button>
      </footer>
    </div>
  );
}
