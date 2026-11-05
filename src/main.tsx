import ReactDOM from "react-dom/client";
import "./styles/base.css";
import { setResolvedLanguage } from "./lib/i18n";
import { themeFromLocation } from "./lib/theme";
import { Panel } from "./windows/panel/Panel";
import { Capture } from "./windows/capture/Capture";
import { Fly } from "./windows/fly/Fly";
import { Shortcut } from "./windows/shortcut/Shortcut";
import { Annotate } from "./windows/annotate/Annotate";
import { Trim } from "./windows/trim/Trim";

const win = new URLSearchParams(location.search).get("window") ?? "panel";

// Set before the first render so window-specific CSS (e.g. the panel's transparent
// backdrop) applies on the very first paint instead of flashing the base surface.
document.documentElement.dataset.window = win;

// P2-8/P3: capture/shortcut/annotate/trim are separate webviews whose theme comes from the URL.
// Resolve it before the first paint too, so a visible light window never flashes the dark base
// surface.
if (win === "capture" || win === "shortcut" || win === "annotate" || win === "trim") {
  document.documentElement.dataset.theme = themeFromLocation();
}

// P3-4/P3-6: the modal editors receive the Rust-resolved language in their URL, so their chrome is
// localized on the first paint instead of switching from English after `get_app_state` resolves.
if (win === "annotate" || win === "trim") {
  const language = new URLSearchParams(location.search).get("lang");
  if (language) setResolvedLanguage(language);
}

function Root() {
  if (win === "panel") return <Panel />;
  // Task 15: the minimal change-shortcut window (`?window=shortcut&target=…`).
  if (win === "shortcut") return <Shortcut />;
  // Task 16: the freeze-frame region-selection overlay (`?window=capture`).
  if (win === "capture") return <Capture />;
  // P2-5: the pre-warmed screen-wide fly-in / fall overlay (`?window=fly`).
  if (win === "fly") return <Fly />;
  // P3-2: the markup editor (`?window=annotate&id=…`), opened by `open_annotate`.
  if (win === "annotate") return <Annotate />;
  // P3-6: the trim window (`?window=trim&id=…`), opened by `open_trim`.
  if (win === "trim") return <Trim />;
  return <div data-window={win}>Hangline: {win}</div>;
}

ReactDOM.createRoot(document.getElementById("root")!).render(<Root />);
