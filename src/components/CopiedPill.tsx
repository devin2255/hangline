/**
 * The `Copied` pill shown on a card for 1.2 s after a successful copy (design spec §3.1).
 *
 * Tendedero draws it as a glass capsule under the card (`PeggedView.swift:102-115`); the glass
 * material lives in `Panel.css`, the lifetime in `lib/pillTimer.ts` so it can be unit-tested. The
 * component itself is pure presentation and announced politely.
 */

import { t } from "../lib/i18n";

export interface CopiedPillProps {
  /** Pill text; defaults to the localized `Copied`. */
  label?: string;
}

export function CopiedPill({ label = t("Copied") }: CopiedPillProps) {
  return (
    <span className="copied-pill" role="status" data-testid="copied-pill">
      <svg
        className="copied-pill__check"
        viewBox="0 0 12 12"
        aria-hidden="true"
        focusable="false"
      >
        <path d="M2 6.2 4.7 9 10 3.2" />
      </svg>
      <span className="copied-pill__label">{label}</span>
    </span>
  );
}
