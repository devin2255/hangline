import { describe, expect, it } from "vitest";
import { isStaleDragEnded } from "./dragSession";

describe("isStaleDragEnded", () => {
  it("accepts the first event of a new session", () => {
    expect(isStaleDragEnded(1, 0)).toBe(false);
    expect(isStaleDragEnded(4, 3)).toBe(false);
  });

  it("ignores a duplicate delivery of the session already handled", () => {
    expect(isStaleDragEnded(3, 3)).toBe(true);
  });

  it("ignores a delayed event from an older session", () => {
    expect(isStaleDragEnded(2, 5)).toBe(true);
  });

  it("accepts a payload without a session token", () => {
    expect(isStaleDragEnded(undefined, 5)).toBe(false);
  });
});
