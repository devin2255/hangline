/**
 * Zustand mirror of the Rust application state (design spec 4.1: Rust is the only data source).
 *
 * The store holds the last full `AppState` received from `get_app_state` / the `state` event and
 * exposes `refresh()` for an explicit re-fetch. Components read it through `useAppStore`; they
 * never write to it directly — mutations go through `setSetting` and come back as a new snapshot.
 *
 * Rust tags every full state with a monotonic `revision`. Two folder watchers plus the IPC thread
 * can broadcast concurrently, so a snapshot can arrive (or a `get_app_state` reply can resolve)
 * after a newer one; [`shouldApplyState`] drops the stale one instead of letting the whole store
 * regress.
 */

import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { setResolvedLanguage } from "./i18n";
import { getAppState } from "./ipc";
import type { AppState } from "./types";

/** Shape of the Zustand store. */
export interface AppStore {
  /** Last full state from Rust; `null` until the first successful fetch. */
  state: AppState | null;
  /** Last IPC error (e.g. no Tauri runtime under plain `vite`); cleared on success. */
  error: string | null;
  /** Re-fetches the full state from Rust (startup and manual resync). */
  refresh: () => Promise<void>;
}

/** The one in-flight `state` event subscription, so initialization is idempotent. */
let subscription: Promise<() => void> | null = null;

/** Revision of the last snapshot applied to the store; `-1` until the first one. */
let lastRevision = -1;

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Returns whether a snapshot tagged `incomingRevision` may replace the mirrored state.
 *
 * Rust increments `revision` on every mutation and serializes snapshot + broadcast, so a lower
 * revision is always stale (it was taken before a state the store already has). Equal revisions
 * carry the same full state (nothing was mutated in between), so applying one is harmless — that
 * keeps the initial `get_app_state` usable when the first `state` event raced it.
 */
export function shouldApplyState(
  currentRevision: number,
  incomingRevision: number,
): boolean {
  return incomingRevision >= currentRevision;
}

/**
 * Applies one full snapshot unless it is stale.
 *
 * @returns whether the store was updated.
 */
function applyState(snapshot: AppState): boolean {
  if (!shouldApplyState(lastRevision, snapshot.revision)) {
    console.debug(
      `Hangline state: ignoring stale snapshot revision ${snapshot.revision} ` +
        `(last ${lastRevision})`,
    );
    return false;
  }
  lastRevision = snapshot.revision;
  // Task 15: Rust resolves `settings.language` (including "system" against the Windows display
  // language) once and broadcasts the tag; select that table before publishing the snapshot so
  // the panel matches the tray exactly. Never resolve "system" from `navigator.language` here.
  setResolvedLanguage(snapshot.resolvedLanguage ?? "en");
  useAppStore.setState({ state: snapshot, error: null });
  return true;
}

/**
 * Frontend mirror of `AppState` (the Zustand store).
 *
 * `refresh()` never throws: failures are logged and exposed as `error`, matching the panel's
 * P0 behavior of rendering without IPC.
 */
export const useAppStore = create<AppStore>((set) => ({
  state: null,
  error: null,
  refresh: async () => {
    try {
      const snapshot = await getAppState();
      // An event may have arrived while the fetch was in flight; the revision check keeps the
      // newer snapshot (Task 7's fetch-vs-event race).
      if (!applyState(snapshot)) return;
      console.log(`Hangline state loaded: ${snapshot.items.length} item(s)`);
    } catch (error) {
      // Plain `vite` (outside Tauri) has no IPC; the panel still renders for styling work.
      console.warn("Hangline state: get_app_state unavailable", error);
      set({ error: messageOf(error) });
    }
  },
}));

/**
 * Starts the `state` event subscription and the initial `get_app_state` fetch exactly once.
 *
 * Called from the panel on mount. The listener is registered *before* the fetch so no broadcast is
 * missed; a mutation that happens during the fetch then arrives as an event and the stale reply is
 * dropped by [`shouldApplyState`]. A window opened after a state change still gets the current
 * snapshot from the fetch, and later changes arrive through the event. If the listener cannot be
 * registered (not running under Tauri) a later call retries instead of being disabled forever.
 */
export function initAppState(): void {
  if (subscription) return;
  const pending = listen<AppState>("state", ({ payload }) => {
    applyState(payload);
  });
  subscription = pending;
  pending.then(
    () => {
      void useAppStore.getState().refresh();
    },
    (error) => {
      if (subscription === pending) subscription = null;
      console.warn("Hangline state: state event listener unavailable", error);
      // Still try the fetch so the caller sees the IPC error/state under plain `vite`.
      void useAppStore.getState().refresh();
    },
  );
}
