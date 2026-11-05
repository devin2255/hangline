/**
 * The markup editor window (`index.html?window=annotate&id=…`, P3-2, design spec §3.7).
 *
 * Rust opens this page with the item id and the raw theme; `get_editor_target` returns the item,
 * whether Hangline owns the file (watched folders are read-only) and the theme. The page renders the
 * original through a versioned asset URL (`assetUrl(path, updatedAt)`) fitted with
 * `object-fit: contain`, and overlays a `<canvas>` at exactly the displayed rectangle. Shapes are
 * stored in natural image pixels (`annotation.ts`), so:
 *
 * - drawing scales display coordinates with `toImagePoint` (window-size independent);
 * - the preview canvas has the image's natural resolution and is only CSS-scaled, which keeps
 *   stroke widths consistent with the saved output;
 * - Save paints a second, offscreen canvas at natural size (crop offset applied) and hands its
 *   PNG to Rust, which atomically replaces the file, refreshes the 432 px preview and broadcasts a
 *   new `state` (the panel re-loads the card with a fresh `?v=` URL).
 *
 * The window owns no global state: Esc closes it (a dirty editor asks first) and a failed save
 * keeps it open with an inline error. Long-press wiring is P3-4; this page is opened by the
 * `open_annotate` command.
 */

import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import {
  PEN_COLORS,
  PEN_WIDTHS,
  addShape,
  canRedo,
  canUndo,
  clampCrop,
  createAnnotationState,
  cropOutput,
  fitContain,
  moveCrop,
  redo,
  renderShapes,
  setCrop,
  textFontPx,
  toImagePoint,
  undo,
  type AnnotationState,
  type CropRect,
  type Point,
  type Shape,
  type Tool,
} from "../../lib/annotation";
import { assetUrl } from "../../lib/assetUrl";
import { editorKeys } from "../../lib/editorKeys";
import { t } from "../../lib/i18n";
import {
  closeEditor,
  getEditorTarget,
  reportAnnotateReady,
  saveAnnotation,
  setEditorDirty,
} from "../../lib/ipc";
import { fileNameOf } from "../../lib/layout";
import { initAppState, useAppStore } from "../../lib/state";
import { themeSettingFromLocation, useResolvedTheme } from "../../lib/theme";
import type { EditorTarget } from "../../lib/types";
import "./Annotate.css";

/** Padding around the image inside the stage, in CSS pixels. */
const STAGE_PADDING = 16;

/** The toolbar tools, in display order (crop is a mode and comes last). */
const TOOLS: readonly Tool[] = ["pen", "rect", "arrow", "text", "crop"];

/** Crop handles (edges + corners), with the edges each one moves. */
const CROP_HANDLES = ["nw", "n", "ne", "e", "se", "s", "sw", "w"] as const;
type CropHandle = (typeof CROP_HANDLES)[number];

/** Maximum number of sampled pen points (bounds memory for a very long stroke). */
const MAX_PEN_POINTS = 6000;

/** A text entry in progress (display position + the image-space anchor). */
interface TextEditor {
  /** Anchor in natural image pixels. */
  point: Point;
  /** Click position inside the displayed frame, in CSS pixels. */
  displayX: number;
  displayY: number;
  /** Font size in CSS pixels for the textarea preview. */
  fontSize: number;
  /** Shape width in image pixels (drives the saved font size). */
  shapeWidth: number;
  color: string;
  value: string;
}

/** The pointer gesture currently in flight. */
type DragState =
  | { mode: "shape" }
  | { mode: "crop"; start: Point }
  | { mode: "cropHandle"; handle: CropHandle }
  | { mode: "cropMove"; start: Point; origin: CropRect };

/** Monotonic shape ids (React keys; never persisted). */
let shapeSequence = 0;

function nextShapeId(): string {
  shapeSequence += 1;
  return `shape-${Date.now().toString(36)}-${shapeSequence.toString(36)}`;
}

/** The tool's i18n label. */
function toolLabel(tool: Tool): string {
  switch (tool) {
    case "pen":
      return t("Pen");
    case "rect":
      return t("Rectangle");
    case "arrow":
      return t("Arrow");
    case "text":
      return t("Text");
    case "crop":
      return t("Crop");
  }
}

/** Normalizes a rectangle dragged in any direction (image pixels). */
function normalizeDrag(rect: CropRect): CropRect {
  return {
    x: Math.min(rect.x, rect.x + rect.w),
    y: Math.min(rect.y, rect.y + rect.h),
    w: Math.abs(rect.w),
    h: Math.abs(rect.h),
  };
}

/** Resizes an existing rectangle by moving one edge/corner to `point`. */
function resizeCrop(rect: CropRect, handle: CropHandle, point: Point): CropRect {
  let left = rect.x;
  let top = rect.y;
  let right = rect.x + rect.w;
  let bottom = rect.y + rect.h;
  if (handle.includes("w")) left = point[0];
  if (handle.includes("e")) right = point[0];
  if (handle.includes("n")) top = point[1];
  if (handle.includes("s")) bottom = point[1];
  return { x: left, y: top, w: right - left, h: bottom - top };
}

/** Whether a committed shape has enough geometry to be visible (drops click-sized rect/arrow). */
function isValidShape(shape: Shape): boolean {
  if (shape.tool === "pen") return shape.points.length >= 1;
  if (shape.points.length < 2) return false;
  const [start, end] = shape.points;
  const distance = Math.hypot(end[0] - start[0], end[1] - start[1]);
  return distance >= 2;
}

/** The markup editor page. */
export function Annotate() {
  // The window is a separate webview: the URL carries the resolved language/theme so the first
  // paint is already correct, and the state subscription keeps both live if they change afterwards.
  const appState = useAppStore((store) => store.state);
  const themeSetting = appState?.settings.theme ?? themeSettingFromLocation();
  const theme = useResolvedTheme(themeSetting);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    return () => {
      delete document.documentElement.dataset.theme;
    };
  }, [theme]);

  // Fetch the full state once so `t()` follows the Rust-resolved language and item updates arrive.
  useEffect(() => {
    initAppState();
  }, []);

  const [targetId, setTargetId] = useState(
    () => new URLSearchParams(location.search).get("id") ?? "",
  );
  const [target, setTarget] = useState<EditorTarget | null>(null);
  const [targetError, setTargetError] = useState<string | null>(null);
  const [annotation, setAnnotation] = useState<AnnotationState | null>(null);
  const [imageReady, setImageReady] = useState(false);
  const [imageFailed, setImageFailed] = useState(false);
  const [tool, setTool] = useState<Tool>("pen");
  const [color, setColor] = useState<string>(PEN_COLORS[0]);
  const [penWidth, setPenWidth] = useState<number>(PEN_WIDTHS[1]);
  const [draft, setDraft] = useState<Shape | null>(null);
  const [cropDraft, setCropDraft] = useState<CropRect | null>(null);
  const [textEditor, setTextEditor] = useState<TextEditor | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [stageSize, setStageSize] = useState({ width: 0, height: 0 });

  const stageRef = useRef<HTMLDivElement | null>(null);
  const frameRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const imageRef = useRef<HTMLImageElement | null>(null);
  const textInputRef = useRef<HTMLTextAreaElement | null>(null);
  /**
   * Latest annotation state, mirrored synchronously.
   *
   * `save()` can be invoked from a keydown handler (including Ctrl+S inside the textarea) in the
   * same tick that committed the text; reading `annotation` from the render closure would miss it.
   */
  const annotationRef = useRef<AnnotationState | null>(null);
  const dirtyRef = useRef(false);
  const savingRef = useRef(false);
  /** True once the window is deliberately closing; the close-request guard must then stay quiet. */
  const closingRef = useRef(false);
  /** Latest in-flight shape (state updates are async during a pointermove). */
  const draftRef = useRef<Shape | null>(null);
  /** Latest crop rectangle being dragged/resized (image pixels, may be unnormalized). */
  const cropDraftRef = useRef<CropRect | null>(null);
  /** Latest text entry (blur + Enter can both fire for one commit). */
  const textEditorRef = useRef<TextEditor | null>(null);
  /** The pointer gesture in flight (`null` when idle). */
  const dragRef = useRef<DragState | null>(null);
  /** True while the current mounted editor still accepts state updates (delayed promises). */
  const mountedRef = useRef(true);
  /**
   * Monotonic load generation. `get_editor_target` resolves asynchronously, so a slow reply for
   * the previous id must never overwrite the item a later `annotate-target` switched to (P3-4
   * review follow-up: the same guard protects the trim page).
   */
  const loadGeneration = useRef(0);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const setTextEditorBoth = useCallback((next: TextEditor | null) => {
    textEditorRef.current = next;
    setTextEditor(next);
  }, []);

  const setDraftBoth = useCallback((next: Shape | null) => {
    draftRef.current = next;
    setDraft(next);
  }, []);

  const setCropDraftBoth = useCallback((next: CropRect | null) => {
    cropDraftRef.current = next;
    setCropDraft(next);
  }, []);

  /** Loads one item into the editor (initial open and an `annotate-target` switch). */
  const load = useCallback(
    async (id: string) => {
      const generation = ++loadGeneration.current;
      if (!id) {
        setTarget(null);
        setTargetError(t("Image failed to load"));
        return;
      }
      setTargetError(null);
      setTarget(null);
      setImageFailed(false);
      setImageReady(false);
      setAnnotation(null);
      annotationRef.current = null;
      setDraftBoth(null);
      setCropDraftBoth(null);
      setTextEditorBoth(null);
      setSaveError(null);
      try {
        const next = await getEditorTarget(id);
        // A stale reply (the window was re-targeted while this call was in flight) is dropped so
        // the late item cannot replace the one the page is now showing.
        if (!mountedRef.current || generation !== loadGeneration.current) return;
        if (next.item.id !== id) {
          setTargetError(t("Image failed to load"));
          return;
        }
        setTarget(next);
      } catch (error) {
        if (!mountedRef.current || generation !== loadGeneration.current) return;
        console.warn(`Hangline annotate: cannot load item ${id}`, error);
        setTargetError(t("Image failed to load"));
      }
    },
    [setCropDraftBoth, setDraftBoth, setTextEditorBoth],
  );

  useEffect(() => {
    void load(targetId);
  }, [load, targetId]);

  // A reused window switches items instead of navigating: the same page re-loads its target.
  // The listener is registered before the ready handshake; Rust replays a target that was emitted
  // while the page was still loading, and a dirty editor refuses a silent switch.
  const targetIdRef = useRef(targetId);
  targetIdRef.current = targetId;
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const stop = await listen<string>("annotate-target", ({ payload }) => {
        if (disposed || typeof payload !== "string" || !payload) return;
        if (dirtyRef.current) {
          console.warn(
            `Hangline annotate: ignoring a switch to ${payload} while the editor has unsaved edits`,
          );
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
        const last = await reportAnnotateReady();
        if (!disposed && last && last !== targetIdRef.current) setTargetId(last);
      } catch (error) {
        console.warn("Hangline annotate: ready handshake unavailable", error);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // The stage size decides the displayed image rectangle; observe it instead of trusting
  // `window.innerWidth` (the toolbar wraps differently at 480 px).
  useLayoutEffect(() => {
    const element = stageRef.current;
    if (!element) return;
    const update = () => setStageSize({ width: element.clientWidth, height: element.clientHeight });
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const display = useMemo(() => {
    if (!annotation) return { width: 0, height: 0 };
    return fitContain(
      annotation.width,
      annotation.height,
      Math.max(0, stageSize.width - STAGE_PADDING * 2),
      Math.max(0, stageSize.height - STAGE_PADDING * 2),
    );
  }, [annotation, stageSize]);

  const owned = target?.owned ?? false;
  /**
   * Whether the editor holds unsaved work.
   *
   * An open-but-empty text entry is not a change (clicking the text tool and pressing Esc must not
   * prompt); a non-empty one is, because blur/Enter would otherwise silently drop typed text.
   */
  const dirty =
    (annotation?.shapes.length ?? 0) > 0 ||
    (annotation?.crop ?? null) !== null ||
    (textEditor !== null && textEditor.value.trim() !== "");
  dirtyRef.current = dirty;

  // Mirror the dirty flag to Rust: `open_annotate` focuses the existing window instead of
  // discarding unsaved edits when a second card is long-pressed.
  useEffect(() => {
    void setEditorDirty(dirty).catch((error: unknown) => {
      console.warn("Hangline annotate: set_editor_dirty unavailable", error);
    });
  }, [dirty]);

  /** Applies one pure annotation transition and keeps the synchronous mirror in step. */
  const applyAnnotation = useCallback(
    (update: (current: AnnotationState) => AnnotationState) => {
      const current = annotationRef.current;
      if (!current) return;
      const next = update(current);
      if (next === current) return;
      annotationRef.current = next;
      setAnnotation(next);
    },
    [],
  );

  /** Converts an image-pixel stroke width from the toolbar's display-pixel choice. */
  const imageStrokeWidth = useCallback((): number => {
    if (!annotation || display.width <= 0) return 1;
    return Math.max(1, Math.round((penWidth * annotation.width) / display.width));
  }, [annotation, display.width, penWidth]);

  /** Pointer position → natural image pixels (the canvas fills the displayed image exactly). */
  const imagePoint = useCallback(
    (event: { clientX: number; clientY: number }): Point => {
      const canvas = canvasRef.current;
      if (!canvas || !annotation || display.width <= 0 || display.height <= 0) return [0, 0];
      const rect = canvas.getBoundingClientRect();
      return toImagePoint(
        { x: event.clientX - rect.left, y: event.clientY - rect.top },
        annotation,
        { displayWidth: display.width, displayHeight: display.height },
      );
    },
    [annotation, display],
  );

  /** Display position inside the frame (for the text textarea). */
  const framePoint = useCallback((event: { clientX: number; clientY: number }) => {
    const frame = frameRef.current;
    if (!frame) return { x: 0, y: 0 };
    const rect = frame.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }, []);

  const commitShape = useCallback(
    (shape: Shape) => {
      applyAnnotation((current) => addShape(current, shape));
    },
    [applyAnnotation],
  );

  const undoAnnotation = useCallback(() => applyAnnotation(undo), [applyAnnotation]);
  const redoAnnotation = useCallback(() => applyAnnotation(redo), [applyAnnotation]);

  // Undo/redo can clear or restore the committed crop; the crop overlay must follow it so a later
  // Apply cannot resurrect a rectangle that no longer exists in the annotation state.
  useEffect(() => {
    if (tool !== "crop" || dragRef.current) return;
    setCropDraftBoth(annotation?.crop ?? null);
  }, [annotation?.crop, setCropDraftBoth, tool]);

  const closeTextEditor = useCallback(() => {
    setTextEditorBoth(null);
  }, [setTextEditorBoth]);

  const commitText = useCallback(() => {
    const editor = textEditorRef.current;
    if (!editor) return;
    setTextEditorBoth(null);
    const text = editor.value.trim();
    if (!text) return;
    const shape: Shape = {
      id: nextShapeId(),
      tool: "text",
      color: editor.color,
      width: editor.shapeWidth,
      points: [editor.point],
      text: editor.value.replace(/[ \t]+$/gm, ""),
    };
    applyAnnotation((current) => addShape(current, shape));
  }, [applyAnnotation, setTextEditorBoth]);

  /** Switches tools, keeping the crop rectangle in sync with the committed crop. */
  const selectTool = useCallback(
    (next: Tool) => {
      setTool(next);
      closeTextEditor();
      if (next === "crop") {
        setCropDraftBoth(annotation?.crop ?? null);
      } else {
        setCropDraftBoth(null);
      }
    },
    [annotation?.crop, closeTextEditor, setCropDraftBoth],
  );

  /** Commits the crop rectangle currently being edited. */
  const applyCrop = useCallback(() => {
    const current = annotationRef.current;
    if (!current || !cropDraftRef.current) return;
    const clamped = clampCrop(cropDraftRef.current, current.width, current.height);
    if (!clamped) return;
    applyAnnotation((state) => setCrop(state, clamped));
    setCropDraftBoth(clamped);
  }, [applyAnnotation, setCropDraftBoth]);

  /** Cancels the crop editing session (a previously applied crop stays). */
  const cancelCrop = useCallback(() => {
    setCropDraftBoth(annotation?.crop ?? null);
  }, [annotation?.crop, setCropDraftBoth]);

  /** Opens the multiline text editor at a click position. */
  const openTextEditor = useCallback(
    (event: ReactPointerEvent<HTMLCanvasElement>, point: Point) => {
      if (!annotation || display.width <= 0) return;
      const position = framePoint(event);
      const shapeWidth = imageStrokeWidth();
      setTextEditorBoth({
        point,
        displayX: position.x,
        displayY: position.y,
        fontSize: Math.max(11, textFontPx(shapeWidth) * (display.width / annotation.width)),
        shapeWidth,
        color,
        value: "",
      });
    },
    [annotation, color, display.width, framePoint, imageStrokeWidth, setTextEditorBoth],
  );

  const handleCanvasPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLCanvasElement>) => {
      if (event.button !== 0 || !annotation || saving) return;
      event.preventDefault();
      try {
        frameRef.current?.setPointerCapture(event.pointerId);
      } catch {
        // The pointer may already be gone (synthetic events); drawing still works without capture.
      }
      const point = imagePoint(event);
      if (tool === "text") {
        closeTextEditor();
        openTextEditor(event, point);
        return;
      }
      if (tool === "crop") {
        closeTextEditor();
        const rect = { x: point[0], y: point[1], w: 0, h: 0 };
        setCropDraftBoth(rect);
        dragRef.current = { mode: "crop", start: point };
        return;
      }
      const width = imageStrokeWidth();
      const shape: Shape = {
        id: nextShapeId(),
        tool,
        color,
        width,
        points: tool === "pen" ? [point] : [point, point],
      };
      setDraftBoth(shape);
      dragRef.current = { mode: "shape" };
    },
    [
      annotation,
      closeTextEditor,
      color,
      imagePoint,
      imageStrokeWidth,
      openTextEditor,
      saving,
      setCropDraftBoth,
      setDraftBoth,
      tool,
    ],
  );

  const handleFramePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const drag = dragRef.current;
      if (!drag || !annotation) return;
      const point = imagePoint(event);
      if (drag.mode === "shape") {
        const current = draftRef.current;
        if (!current) return;
        if (current.tool === "pen") {
          const last = current.points[current.points.length - 1];
          if (
            last &&
            (current.points.length >= MAX_PEN_POINTS ||
              Math.hypot(point[0] - last[0], point[1] - last[1]) < 1)
          ) {
            return;
          }
          setDraftBoth({ ...current, points: [...current.points, point] });
        } else {
          setDraftBoth({ ...current, points: [current.points[0], point] });
        }
        return;
      }
      if (drag.mode === "crop") {
        setCropDraftBoth({
          x: drag.start[0],
          y: drag.start[1],
          w: point[0] - drag.start[0],
          h: point[1] - drag.start[1],
        });
        return;
      }
      if (drag.mode === "cropMove") {
        if (!cropDraftRef.current) return;
        setCropDraftBoth(
          moveCrop(drag.origin, point[0] - drag.start[0], point[1] - drag.start[1]),
        );
        return;
      }
      const current = cropDraftRef.current;
      if (!current) return;
      setCropDraftBoth(resizeCrop(normalizeDrag(current), drag.handle, point));
    },
    [annotation, imagePoint, setCropDraftBoth, setDraftBoth],
  );

  const handleFramePointerUp = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const drag = dragRef.current;
      if (!drag) return;
      dragRef.current = null;
      try {
        frameRef.current?.releasePointerCapture(event.pointerId);
      } catch {
        // No capture was taken (synthetic events): releasing is unnecessary.
      }
      // A fast drag can coalesce pointermove events, so the last observed move may lag behind the
      // release. Finalize from the pointerup position instead of trusting the last pointermove.
      const point = imagePoint(event);
      if (drag.mode === "shape") {
        const current = draftRef.current;
        setDraftBoth(null);
        if (!current) return;
        let final = current;
        if (current.tool === "pen") {
          const last = current.points[current.points.length - 1];
          if (!last || Math.hypot(point[0] - last[0], point[1] - last[1]) >= 0.5) {
            final = { ...current, points: [...current.points, point] };
          }
        } else {
          final = { ...current, points: [current.points[0], point] };
        }
        if (isValidShape(final)) commitShape(final);
        return;
      }
      if (drag.mode === "crop") {
        const rect = {
          x: drag.start[0],
          y: drag.start[1],
          w: point[0] - drag.start[0],
          h: point[1] - drag.start[1],
        };
        setCropDraftBoth(
          clampCrop(normalizeDrag(rect), annotation?.width ?? 1, annotation?.height ?? 1),
        );
        return;
      }
      if (drag.mode === "cropMove") {
        const current = cropDraftRef.current;
        if (current) {
          setCropDraftBoth(
            clampCrop(normalizeDrag(current), annotation?.width ?? 1, annotation?.height ?? 1),
          );
        }
        return;
      }
      const current = cropDraftRef.current;
      if (current) {
        setCropDraftBoth(normalizeDrag(resizeCrop(normalizeDrag(current), drag.handle, point)));
      }
    },
    [
      annotation?.height,
      annotation?.width,
      commitShape,
      imagePoint,
      setCropDraftBoth,
      setDraftBoth,
    ],
  );

  const beginCropHandle = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>, handle: CropHandle) => {
      if (event.button !== 0 || !cropDraft) return;
      event.preventDefault();
      event.stopPropagation();
      try {
        frameRef.current?.setPointerCapture(event.pointerId);
      } catch {
        // See the canvas handler: capture is an optimization for pointer tracking.
      }
      dragRef.current = { mode: "cropHandle", handle };
    },
    [cropDraft],
  );

  /** Starts dragging the whole crop rectangle (the `cursor: move` affordance). */
  const beginCropMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const current = cropDraftRef.current;
      if (event.button !== 0 || !current) return;
      event.preventDefault();
      event.stopPropagation();
      try {
        frameRef.current?.setPointerCapture(event.pointerId);
      } catch {
        // See the canvas handler: capture is an optimization for pointer tracking.
      }
      dragRef.current = {
        mode: "cropMove",
        start: imagePoint(event),
        origin: normalizeDrag(current),
      };
    },
    [imagePoint],
  );

  /** Paints the preview canvas: committed shapes + the shape being drawn. */
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !annotation) return;
    const context = canvas.getContext("2d");
    if (!context) return;
    if (canvas.width !== annotation.width || canvas.height !== annotation.height) {
      canvas.width = annotation.width;
      canvas.height = annotation.height;
    }
    context.clearRect(0, 0, annotation.width, annotation.height);
    renderShapes(context, draft ? [...annotation.shapes, draft] : annotation.shapes, 1);
  }, [annotation, draft, display.height, display.width]);

  /** Saves the composited full-resolution PNG through Rust and closes the editor. */
  const save = useCallback(async () => {
    const image = imageRef.current;
    const current = annotationRef.current;
    if (!current || !target || !owned || savingRef.current) return;
    if (!image || !image.complete || image.naturalWidth === 0) return;
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
    try {
      const output = cropOutput(current);
      const canvas = document.createElement("canvas");
      canvas.width = output.width;
      canvas.height = output.height;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("the 2D canvas context is unavailable");
      context.drawImage(image, -output.offsetX, -output.offsetY);
      context.save();
      context.translate(-output.offsetX, -output.offsetY);
      renderShapes(context, current.shapes, 1);
      context.restore();
      const dataUrl = canvas.toDataURL("image/png");
      const separator = dataUrl.indexOf(",");
      if (separator < 0) throw new Error("the canvas did not produce a data URL");
      await saveAnnotation(target.item.id, dataUrl.slice(separator + 1));
      if (!mountedRef.current) return;
      closingRef.current = true;
      await closeEditor();
    } catch (error) {
      console.warn(`Hangline annotate: save of ${target.item.path} failed`, error);
      if (!mountedRef.current) return;
      savingRef.current = false;
      setSaving(false);
      setSaveError(t("Save failed"));
    }
  }, [owned, target]);

  /** Closes the editor; a dirty image asks before discarding (Esc/titlebar X/Alt+F4). */
  const requestCancel = useCallback(() => {
    if (savingRef.current) return;
    if (dirtyRef.current && !window.confirm(t("Discard changes?"))) return;
    closingRef.current = true;
    void closeEditor().catch((error: unknown) => {
      console.warn("Hangline annotate: close_editor failed", error);
    });
  }, []);

  /**
   * Close-request guard (titlebar X, Alt+F4 and Rust `close_editor`).
   *
   * The editor used to destroy itself on Alt+F4 without asking, silently losing annotations.
   * Preventing the request lets the same dirty confirmation run, then `requestCancel` asks Rust to
   * close again with `closingRef` set so the second request goes through.
   */
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWindow()
      .onCloseRequested((event) => {
        if (disposed || closingRef.current) return;
        // A save in flight must not be interrupted; once it resolves it closes the window itself.
        if (savingRef.current) {
          event.preventDefault();
          return;
        }
        if (!dirtyRef.current) return;
        event.preventDefault();
        if (window.confirm(t("Discard changes?"))) {
          closingRef.current = true;
          void closeEditor().catch((error: unknown) => {
            console.warn("Hangline annotate: close_editor after a close request failed", error);
          });
        }
      })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error: unknown) => {
        console.warn("Hangline annotate: close-request listener unavailable", error);
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Window-level shortcuts (Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z / Ctrl+S / Esc). The textarea owns its
  // own keys (Enter commits, Esc cancels the text entry), so its events are not interpreted here.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.tagName === "TEXTAREA") return;
      const command = editorKeys(event.key, event.ctrlKey || event.metaKey, event.shiftKey);
      if (!command) return;
      if (command === "undo") {
        event.preventDefault();
        undoAnnotation();
      } else if (command === "redo") {
        event.preventDefault();
        redoAnnotation();
      } else if (command === "save") {
        event.preventDefault();
        void save();
      } else {
        event.preventDefault();
        requestCancel();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [redoAnnotation, requestCancel, save, undoAnnotation]);

  const handleImageLoad = useCallback(() => {
    const image = imageRef.current;
    if (!image) return;
    const width = image.naturalWidth;
    const height = image.naturalHeight;
    if (width < 1 || height < 1) {
      setImageReady(false);
      setImageFailed(true);
      return;
    }
    setImageReady(true);
    setImageFailed(false);
    const next = createAnnotationState(width, height);
    annotationRef.current = next;
    setAnnotation(next);
  }, []);

  // The multi-line textarea grows with its content instead of clipping after the two-line minimum.
  useLayoutEffect(() => {
    const element = textInputRef.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${element.scrollHeight}px`;
  }, [textEditor?.value]);

  const cropDisplay = useMemo(() => {
    if (!annotation || !cropDraft || display.width <= 0) return null;
    const scale = display.width / annotation.width;
    const rect = normalizeDrag(cropDraft);
    return {
      left: rect.x * scale,
      top: rect.y * scale,
      width: Math.max(1, rect.w * scale),
      height: Math.max(1, rect.h * scale),
    };
  }, [annotation, cropDraft, display.width]);

  const imageUrl = target ? assetUrl(target.item.path, target.item.updatedAt) : "";
  const fileName = target ? fileNameOf(target.item.path) : "";
  const version = target?.item.updatedAt ?? 0;

  return (
    <div
      className="annotate"
      data-window="annotate"
      data-tool={tool}
      data-owned={owned ? "true" : "false"}
      data-dirty={dirty ? "true" : "false"}
      data-saving={saving ? "true" : "false"}
      data-image-version={version}
      data-image-width={annotation?.width ?? 0}
      data-image-height={annotation?.height ?? 0}
      data-shapes={annotation?.shapes.length ?? 0}
      data-crop={annotation?.crop ? "true" : "false"}
    >
      <header className="annotate__titlebar" data-tauri-drag-region>
        <span className="annotate__title" data-tauri-drag-region>
          {t("Markup")}
          {fileName ? <span className="annotate__filename"> · {fileName}</span> : null}
        </span>
        <button
          type="button"
          className="annotate__close"
          title={t("Cancel")}
          aria-label={t("Cancel")}
          onClick={requestCancel}
          data-action="close"
        >
          ✕
        </button>
      </header>

      {target ? (
        <div className="annotate__toolbar" role="toolbar" aria-label={t("Markup")}>
          <div className="annotate__group">
            {TOOLS.map((name) => (
              <button
                key={name}
                type="button"
                className="annotate__button"
                aria-pressed={tool === name}
                data-active={tool === name ? "true" : "false"}
                data-tool={name}
                onClick={() => selectTool(name)}
                disabled={saving}
              >
                {toolLabel(name)}
              </button>
            ))}
          </div>
          <div className="annotate__group" role="group" aria-label={t("Pen")}>
            {PEN_COLORS.map((value) => (
              <button
                key={value}
                type="button"
                className="annotate__swatch"
                style={{ backgroundColor: value }}
                aria-label={value}
                aria-pressed={color === value}
                data-active={color === value ? "true" : "false"}
                data-color={value}
                onClick={() => setColor(value)}
                disabled={saving}
              />
            ))}
          </div>
          <div className="annotate__group" role="group" aria-label={t("Pen")}>
            {PEN_WIDTHS.map((value) => (
              <button
                key={value}
                type="button"
                className="annotate__width"
                aria-pressed={penWidth === value}
                data-active={penWidth === value ? "true" : "false"}
                data-width={value}
                onClick={() => setPenWidth(value)}
                disabled={saving}
              >
                <span style={{ width: value * 2, height: value * 2 }} aria-hidden="true" />
              </button>
            ))}
          </div>
          {tool === "crop" && cropDraft ? (
            <div className="annotate__group">
              <button
                type="button"
                className="annotate__button"
                onClick={applyCrop}
                disabled={saving}
                data-action="apply-crop"
              >
                {t("Apply crop")}
              </button>
              <button
                type="button"
                className="annotate__button"
                onClick={cancelCrop}
                disabled={saving}
                data-action="cancel-crop"
              >
                {t("Cancel")}
              </button>
            </div>
          ) : null}
          <span className="annotate__spacer" />
          <div className="annotate__group">
            <button
              type="button"
              className="annotate__button"
              onClick={undoAnnotation}
              disabled={!annotation || !canUndo(annotation) || saving}
              data-action="undo"
            >
              {t("Undo")}
            </button>
            <button
              type="button"
              className="annotate__button"
              onClick={redoAnnotation}
              disabled={!annotation || !canRedo(annotation) || saving}
              data-action="redo"
            >
              {t("Redo")}
            </button>
          </div>
          <div className="annotate__group">
            <button
              type="button"
              className="annotate__button"
              onClick={requestCancel}
              disabled={saving}
              data-action="cancel"
            >
              {t("Cancel")}
            </button>
            <button
              type="button"
              className="annotate__button annotate__button--primary"
              onClick={() => void save()}
              disabled={!owned || !annotation || imageFailed || saving}
              data-action="save"
            >
              {saving ? t("Saving…") : t("Save")}
            </button>
          </div>
        </div>
      ) : null}

      <div className="annotate__stage" ref={stageRef}>
        {target ? (
          <div
            className="annotate__frame"
            ref={frameRef}
            style={
              display.width > 0
                ? { width: `${display.width}px`, height: `${display.height}px` }
                : undefined
            }
            data-image-ready={imageReady ? "true" : "false"}
            onPointerMove={handleFramePointerMove}
            onPointerUp={handleFramePointerUp}
            onPointerCancel={handleFramePointerUp}
          >
            <img
              key={target.item.id}
              ref={imageRef}
              className="annotate__image"
              // CORS-clean load: the asset protocol answers with
              // `Access-Control-Allow-Origin: <window origin>`; without this attribute the canvas
              // would be tainted and `toDataURL` (the save path) throws `SecurityError`.
              crossOrigin="anonymous"
              src={imageUrl}
              alt={fileName}
              draggable={false}
              data-image-path={target.item.path}
              onLoad={handleImageLoad}
              onError={() => {
                setImageReady(false);
                setImageFailed(true);
              }}
            />
            {annotation && display.width > 0 ? (
              <canvas
                ref={canvasRef}
                className="annotate__canvas"
                style={{ width: `${display.width}px`, height: `${display.height}px` }}
                data-tool={tool}
                onPointerDown={handleCanvasPointerDown}
              />
            ) : null}
            {tool === "crop" && cropDisplay ? (
              <div
                className="annotate__crop"
                style={{
                  left: `${cropDisplay.left}px`,
                  top: `${cropDisplay.top}px`,
                  width: `${cropDisplay.width}px`,
                  height: `${cropDisplay.height}px`,
                }}
                data-crop="true"
                onPointerDown={beginCropMove}
              >
                {CROP_HANDLES.map((handle) => (
                  <div
                    key={handle}
                    className={`annotate__crop-handle annotate__crop-handle--${handle}`}
                    data-handle={handle}
                    onPointerDown={(event) => beginCropHandle(event, handle)}
                  />
                ))}
              </div>
            ) : null}
            {textEditor ? (
              <textarea
                ref={textInputRef}
                className="annotate__text-input"
                style={{
                  left: `${textEditor.displayX}px`,
                  top: `${textEditor.displayY}px`,
                  fontSize: `${textEditor.fontSize}px`,
                  color: textEditor.color,
                }}
                value={textEditor.value}
                rows={2}
                autoFocus
                spellCheck={false}
                data-action="text-input"
                onChange={(event) =>
                  setTextEditorBoth({ ...textEditor, value: event.target.value })
                }
                onKeyDown={(event) => {
                  event.stopPropagation();
                  // Ctrl+S must save even while the textarea has focus (and never trigger the
                  // browser's own save dialog); commit the text first so the saved PNG includes it.
                  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
                    event.preventDefault();
                    commitText();
                    void save();
                    return;
                  }
                  if (event.key === "Escape") {
                    event.preventDefault();
                    closeTextEditor();
                  } else if (event.key === "Enter" && !event.shiftKey) {
                    event.preventDefault();
                    commitText();
                  }
                }}
                onBlur={commitText}
                onPointerDown={(event) => event.stopPropagation()}
              />
            ) : null}
            {!imageReady && !imageFailed ? (
              <div className="annotate__overlay">{t("Loading…")}</div>
            ) : null}
            {imageFailed ? (
              <div className="annotate__overlay annotate__overlay--error">
                <span>{t("Image failed to load")}</span>
                <span className="annotate__path">{target.item.path}</span>
              </div>
            ) : null}
          </div>
        ) : null}
        {!target ? (
          <div
            className={
              targetError ? "annotate__overlay annotate__overlay--error" : "annotate__overlay"
            }
          >
            {targetError ?? t("Loading…")}
          </div>
        ) : null}
      </div>

      <footer className="annotate__status">
        {!owned && target ? (
          <span className="annotate__readonly" data-read-only="true">
            {t("Read-only (watched folder)")}
          </span>
        ) : null}
        {target && owned && imageFailed ? (
          <span className="annotate__error" data-cannot-edit="true">
            {t("Cannot edit this file")}
          </span>
        ) : null}
        {saveError ? (
          <span className="annotate__error" data-save-error="true">
            {saveError}
          </span>
        ) : null}
        <span className="annotate__hint">
          {t("Ctrl+Z undo, Ctrl+Y redo, Ctrl+S save, Esc cancel")}
        </span>
      </footer>
    </div>
  );
}
