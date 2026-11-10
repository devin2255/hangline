/**
 * Wave 2.4 hover preview: one large original-image overlay inside the panel window.
 *
 * The panel renders a single instance (`Panel` owns the state) instead of every card decoding its
 * own original: the hovered card asks the panel after the 300 ms dwell, the panel computes the
 * geometry (`lib/panel.ts`) and mounts this component. The element is `pointer-events: none` and
 * is not part of the click-through hit rects, so the hovered card and its action capsule stay
 * fully interactive while the preview peeks out behind them (the hovered slot is raised above the
 * preview in `Panel.css`).
 *
 * The original image is requested only when this component mounts (i.e. after the dwell); a decode
 * failure falls back to the cached 432 px preview once, then hides the overlay.
 */

import { useEffect, useState } from "react";
import { assetUrl } from "../lib/assetUrl";
import type { PreviewGeometry } from "../lib/panel";
import type { Item } from "../lib/types";
import "./CardPreview.css";

export interface CardPreviewProps {
  /** The hovered item; recordings never reach this component. */
  item: Item;
  /** Resolved on-screen box in panel-client CSS pixels (`previewGeometryFor`). */
  geometry: PreviewGeometry;
}

export function CardPreview({ item, geometry }: CardPreviewProps) {
  const version = item.updatedAt ?? 0;
  const thumbSrc = item.thumbPath ? assetUrl(item.thumbPath, version) : null;
  /** Load the original first; on decode failure try the cached preview once, then hide. */
  const [source, setSource] = useState<"original" | "thumb" | "failed">("original");

  useEffect(() => {
    setSource("original");
  }, [item.id, item.updatedAt]);

  if (source === "failed" || (source === "thumb" && !thumbSrc)) return null;
  const src = source === "thumb" && thumbSrc ? thumbSrc : assetUrl(item.path, version);

  return (
    <div
      className="panel__card-preview"
      style={{
        left: `${geometry.x}px`,
        top: `${geometry.y}px`,
        width: `${geometry.width}px`,
        height: `${geometry.height}px`,
      }}
      data-item-id={item.id}
      aria-hidden="true"
    >
      <img
        className="panel__card-preview__image"
        src={src}
        alt=""
        draggable={false}
        onError={() =>
          setSource((current) => (current === "original" && thumbSrc ? "thumb" : "failed"))
        }
      />
    </div>
  );
}
