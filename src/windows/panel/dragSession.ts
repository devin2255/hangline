/** How long the panel waits for Rust's `drag-ended` event before clearing the pressed card. */
export const DRAG_ENDED_TIMEOUT_MS = 30_000;

/**
 * Returns true when a `drag-ended` event belongs to a session the panel already handled.
 *
 * Rust stamps every drag with a monotonically increasing `sessionId` (see
 * `src-tauri/src/dragout.rs`) and the panel remembers the highest id it has seen. A delayed event
 * from an older session — or a duplicate delivery of the current one — must not clear the state
 * of a newer drag. Payloads without a token (an older build) are always handled: clearing the
 * pressed state is idempotent.
 */
export function isStaleDragEnded(sessionId: number | undefined, lastHandled: number): boolean {
  if (sessionId === undefined) return false;
  return sessionId > 0 && sessionId <= lastHandled;
}
