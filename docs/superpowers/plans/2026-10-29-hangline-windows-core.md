# Hangline Windows V1 Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付 Hangline（晾图绳）Windows 版的可日用里程碑：P0 六项技术验证通过 + P1 核心闭环（四路截图来源 → 顶栏上卡 → 展开/收起 → 复制/打开/拖出/删除 → 托盘与持久化）。

**Architecture:** Tauri 2 单进程。Rust 侧负责 Win32/WinRT 原生能力（窗口、截屏、剪贴板、拖出、回收站、托盘、热键、目录监听、几何与持久化），React/TS 侧负责顶栏面板、玻璃卡片、动画与交互，两者通过 20 个 command / 10 个 event 通信。Rust 的 `AppState` 是唯一数据源，前端只做投影与动画。

**Tech Stack:** Tauri 2（2.12+）、Rust stable（1.90+，edition 2021，MSVC 工具链）、`windows` crate 0.62、React 19 + TypeScript 5 + Vite 7、pnpm 10、Vitest、`image` / `notify` / `blake3` / `serde`。

**Spec:** `docs/superpowers/specs/2026-10-28-hangline-windows-design.md`（实现时必须同时阅读 spec；本计划不重复 spec 中的产品规则，只把它们落成代码与测试。）

---

## Global Constraints

以下约束适用于**每一个任务**，逐条来自 spec，执行时不得偏离：

- **平台**：Windows 10 22H2 / Windows 11，x64；不实现 arm64（V1）。
- **产品名**：英文 `Hangline`，中文「晾图绳」；图标必须原创，禁止复制 Tendedero 图标/名称。
- **技术版本**：Tauri 2（>=2.12）、React 19、TypeScript 5、Vite 7、pnpm 10、Rust edition 2021、`tauri.conf.json` 与 `package.json` 版本号必须一致。
- **安装包**：NSIS + portable，目标 ≤ 15 MB；空闲内存 ≤ 150 MB；空闲 CPU < 1%。
- **默认热键**：截图 `Ctrl+Alt+S`，显示/隐藏线 `Ctrl+Alt+T`；注册失败必须回滚旧键并提示。
- **零网络**（除自动更新）：不引入任何分析/遥测/云 SDK；更新是唯一出网行为。
- **数据目录**：`%LOCALAPPDATA%\Hangline\`（`settings.json`、`state.json`、`logs\`、`Screenshots\`）。
- **面板几何**（逻辑 dp，渲染乘 scaleFactor）：尺寸档 small/medium/large → `cardWidth = 150*scale`（120/150/202）、`photoMaxHeight = 104*scale`（83/104/140）、面板高 = `photoMaxHeight + 106`（189/210/246）；`spacing = cardWidth + 24`；`fits = max(3, min(12, floor((panelWidth-200)/spacing)))`。
- **动画参数**：展开 `stiffness 224 / damping 25`、收起 `easeIn 220ms`；微风 `stiffness 38 / damping 2.4`，振幅 1.6–3.4°，阵风间隔 7–16s；入场 16°/`stiffness 46 / damping 2.6`；长按 0.45s；拖拽阈值 4dp；离开收起 0.5s；新截图 peek 2.5s。
- **i18n**：key = 英文原文，`src/i18n/en.json` 与 `src/i18n/zh-Hans.json` V1 必须齐全，缺失 key 由 `scripts/check-i18n.mjs` 报错。
- **质量门**：每个任务结束前 `pnpm check`（lint + typecheck + vitest + i18n + version）与 `cargo test` 全绿，并产生一个独立 commit。
- **命名**：Rust 文件 snake_case、React 组件 PascalCase、IPC command snake_case、事件名 kebab-case。
- **不做**：自研录屏编码器、OCR、云同步、账号、马赛克/滚动截图（见 spec §1.4）。

**里程碑定义**：本计划完成后，应用可日用：任意方式截图 → 上栏 → 用热键/自动展开 → 复制/拖出/删除 → 退出重启状态正确。P2 视觉灵魂、P3 标注/录屏、P4 设置完善、P5 发布各自另立计划。

---

## File Structure（本计划涉及的全部文件）

```
hangline/
├── package.json                       # 前端依赖、scripts（dev/build/check/tauri:*）
├── pnpm-lock.yaml
├── vite.config.ts                     # React + 多窗口入口 + vitest 配置
├── tsconfig.json / tsconfig.node.json
├── index.html                         # 单入口 ?window=panel|fly|capture...
├── .gitignore
├── .prettierrc / eslint.config.js
├── scripts/
│   ├── check-i18n.mjs                 # i18n key 集合校验
│   └── version.mjs                    # package.json ↔ tauri.conf.json 版本一致性
├── src/
│   ├── main.tsx                       # 按 ?window= 选择窗口根组件
│   ├── windows/panel/Panel.tsx        # 顶栏根组件
│   ├── windows/panel/Panel.css
│   ├── components/Rope.tsx            # 绳（SVG path）
│   ├── components/GlassCard.tsx       # 玻璃卡片 + 夹子 + 悬浮操作
│   ├── components/ClothesPin.tsx
│   ├── components/CopiedPill.tsx
│   ├── lib/layout.ts                  # 几何/容量/位置（纯函数，可测）
│   ├── lib/layout.test.ts
│   ├── lib/spring.ts                  # rAF 弹簧积分器（Task 11 落地）
│   ├── lib/spring.test.ts
│   ├── lib/state.ts                   # Zustand store，订阅 Rust 事件
│   ├── lib/ipc.ts                     # invoke 封装 + 类型
│   ├── lib/types.ts                   # 与 Rust serde 类型一一对应
│   ├── lib/i18n.ts                    # t(key) + 语言切换
│   ├── i18n/en.json / zh-Hans.json
│   └── styles/base.css
├── src-tauri/
│   ├── Cargo.toml
│   ├── build.rs
│   ├── tauri.conf.json
│   ├── icons/                         # 原创图标（占位也必须是自有图）
│   └── src/
│       ├── main.rs                    # 唯一入口，调用 lib::run()
│       ├── lib.rs                     # Builder、插件、setup、命令注册
│       ├── app_state.rs               # AppState + 事件广播
│       ├── settings.rs                # 默认值/校验/原子持久化
│       ├── store.rs                   # items 持久化 + 启动校验 + prune
│       ├── item.rs                    # Item 模型与分类
│       ├── geometry.rs                # 显示器/DPI/工作区/布局/命中（纯函数）
│       ├── hittest.rs                 # 30Hz 鼠标轮询 → click-through
│       ├── panel.rs                   # 面板窗口创建/定位/显隐
│       ├── capture.rs                 # GDI 截屏 + 区域裁剪 + PNG 写盘
│       ├── watchers/dirs.rs           # notify 目录监听
│       ├── watchers/clipboard.rs      # 剪贴板监听（消息窗 + 轮询兜底）
│       ├── watchers/mod.rs
│       ├── dedupe.rs                  # BLAKE3 哈希 + LRU 去重
│       ├── clipboard_io.rs            # 写 CF_DIBV5 / "PNG" / CF_HDROP
│       ├── trash.rs                   # IFileOperation 移入回收站
│       ├── dragout.rs                 # 原生文件拖出
│       ├── tray.rs                    # 托盘图标 + 菜单
│       ├── hotkeys.rs                 # 注册/改键/冲突回滚
│       ├── reveal.rs                  # 展开/收起状态机 + 指针/点击检测
│       ├── ipc.rs                     # command/event 定义
│       └── tests_util.rs              # 临时目录等测试辅助（cfg(test)）
├── docs/
│   ├── superpowers/specs/2026-10-28-hangline-windows-design.md
│   ├── superpowers/plans/2026-10-29-hangline-windows-core.md   # 本文件
│   ├── experiments/P0-E1E2.md / P0-E3.md / P0-E4.md / P0-E5.md
│   └── experiments/P0-report.md         # P0 门禁报告
└── .github/workflows/ci.yml             # windows-latest：check + cargo test + tauri build
```

---

## Task 1: 工程脚手架与质量闸门

**Files:**
- Create: `package.json`, `vite.config.ts`, `tsconfig.json`, `tsconfig.node.json`, `index.html`, `src/main.tsx`, `src/lib/i18n.ts`, `src/styles/base.css`, `scripts/check-i18n.mjs`, `scripts/version.mjs`, `src/i18n/en.json`, `src/i18n/zh-Hans.json`, `.gitignore`, `.prettierrc`, `eslint.config.js`, `README.md`, `.github/workflows/ci.yml`
- Create: `src-tauri/`（`pnpm tauri init --ci` 生成）+ `Cargo.toml` 依赖
- Test: `scripts/version.mjs` 与 `check-i18n.mjs` 自身的失败用例（手工命令验证）

**Interfaces:**
- Produces: `pnpm dev`（Vite :5173）、`pnpm tauri dev`、`pnpm check`、`pnpm build`、`src-tauri/Cargo.toml` 中的依赖集合、`index.html?window=` 路由约定。

- [ ] **Step 1: 写前端工程文件**

`package.json`：
```json
{
  "name": "hangline",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc --noEmit && vite build",
    "preview": "vite preview",
    "tauri": "tauri",
    "tauri:dev": "tauri dev",
    "tauri:build": "tauri build",
    "lint": "eslint .",
    "typecheck": "tsc --noEmit",
    "test": "vitest run --passWithNoTests",
    "check": "pnpm lint && pnpm typecheck && pnpm test && node scripts/check-i18n.mjs && node scripts/version.mjs"
  },
  "dependencies": {
    "@tauri-apps/api": "^2",
    "react": "^19",
    "react-dom": "^19",
    "zustand": "^5"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2",
    "@types/react": "^19",
    "@types/react-dom": "^19",
    "@vitejs/plugin-react": "^5",
    "@eslint/js": "^9",
    "eslint": "^9",
    "eslint-plugin-react-hooks": "^5",
    "typescript-eslint": "^8",
    "prettier": "^3",
    "typescript": "^5",
    "vite": "^7",
    "vitest": "^3"
  }
}
```

`vite.config.ts`：
```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "chrome105" },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
```

`index.html`：
```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Hangline</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

`src/main.tsx` 先做窗口路由占位（Task 2 起填充各窗口）：
```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import "./styles/base.css";

const win = new URLSearchParams(location.search).get("window") ?? "panel";

function Root() {
  return <div data-window={win}>Hangline: {win}</div>;
}

ReactDOM.createRoot(document.getElementById("root")!).render(<Root />);
```

`src/i18n/en.json`（V1 只放本计划用到的 key，后续任务按需增加）：
```json
{
  "Show line": "Show line",
  "Hide line": "Hide line",
  "Take everything down": "Take everything down",
  "Copied": "Copied"
}
```
`src/i18n/zh-Hans.json`：
```json
{
  "Show line": "显示线",
  "Hide line": "隐藏线",
  "Take everything down": "全部取下线",
  "Copied": "已复制"
}
```

`src/lib/i18n.ts`（V1 最小实现：内置 JSON 表，按系统语言选择，缺失回退英文）：
```ts
import en from "../i18n/en.json";
import zhHans from "../i18n/zh-Hans.json";

const tables: Record<string, Record<string, string>> = { en, "zh-Hans": zhHans };
const lang = navigator.language.toLowerCase().startsWith("zh") ? "zh-Hans" : "en";
const table = tables[lang] ?? en;

export function t(key: string): string {
  return table[key] ?? en[key] ?? key;
}
```

`.prettierrc`：
```json
{ "semi": true, "singleQuote": false, "printWidth": 100 }
```

`eslint.config.js`（TypeScript + React hooks 最小配置）：
```js
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";

export default tseslint.config(
  { ignores: ["dist", "src-tauri/target"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  { plugins: { "react-hooks": reactHooks }, rules: { ...reactHooks.configs.recommended.rules } },
);
```

`.github/workflows/ci.yml`：
```yaml
name: ci
on: [push, pull_request]
jobs:
  windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
        with: { version: 10 }
      - uses: actions/setup-node@v4
        with: { node-version: 22, cache: pnpm }
      - uses: dtolnay/rust-toolchain@stable
      - run: pnpm install --frozen-lockfile
      - run: pnpm check
      - run: cargo test --manifest-path src-tauri/Cargo.toml
      - run: pnpm tauri build --no-bundle
```

- [ ] **Step 2: 写质量脚本**

`scripts/version.mjs`：
```js
import { readFileSync } from "node:fs";
const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url)));
const conf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url)));
const a = pkg.version, b = conf.version;
if (a !== b) {
  console.error(`version mismatch: package.json=${a} tauri.conf.json=${b}`);
  process.exit(1);
}
console.log(`version ok: ${a}`);
```

`scripts/check-i18n.mjs`：
```js
import { readFileSync, readdirSync } from "node:fs";
const dir = new URL("../src/i18n/", import.meta.url);
const en = JSON.parse(readFileSync(new URL("en.json", dir)));
const langs = readdirSync(dir).filter((f) => f.endsWith(".json") && f !== "en.json");
let bad = 0;
for (const f of langs) {
  const t = JSON.parse(readFileSync(new URL(f, dir)));
  for (const k of Object.keys(en)) if (!(k in t)) { console.error(`${f}: missing "${k}"`); bad++; }
  for (const k of Object.keys(t)) if (!(k in en)) { console.error(`${f}: unused "${k}"`); bad++; }
}
if (bad) process.exit(1);
console.log(`i18n ok: ${Object.keys(en).length} keys, ${langs.length} languages`);
```

- [ ] **Step 3: 安装依赖并初始化 Tauri**

```powershell
cd %USERPROFILE%\Documents\deepseek-harness\default-workspace\hangline
pnpm install
pnpm exec tauri init --ci --app-name Hangline --window-title Hangline --frontend-dist ../dist --dev-url http://localhost:5173 --before-dev-command "pnpm dev" --before-build-command "pnpm build"
```
Expected：生成 `src-tauri/{Cargo.toml,build.rs,tauri.conf.json,src/main.rs,src/lib.rs,icons/}`。

- [ ] **Step 4: 补齐 Rust 依赖与插件**

```powershell
cd src-tauri
cargo add tauri@2 --features tray-icon,protocol-asset
cargo add tauri-plugin-log@2 tauri-plugin-single-instance@2 tauri-plugin-autostart@2 tauri-plugin-global-shortcut@2
cargo add serde --features derive
cargo add serde_json anyhow thiserror image notify blake3 base64
cargo add windows@0.62 --features Win32_Foundation,Win32_UI_WindowsAndMessaging,Win32_UI_Shell,Win32_System_Com,Win32_System_Threading,Win32_Graphics_Gdi,Win32_Graphics_Dwm,Win32_Storage_FileSystem,Win32_System_LibraryLoader,Win32_System_Variant,Win32_System_DataExchange,Win32_System_Memory,Win32_System_Ole,Media_Editing,Media_Transcoding
```
Expected：`Cargo.toml` 出现上述依赖；`cargo check` 通过。

- [ ] **Step 5: 写 Tauri 配置与最小 lib.rs**

`src-tauri/tauri.conf.json` 关键字段：
```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Hangline",
  "version": "0.1.0",
  "identifier": "app.hangline.Hangline",
  "build": {
    "beforeDevCommand": "pnpm dev",
    "beforeBuildCommand": "pnpm build",
    "devUrl": "http://localhost:5173",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [],
    "security": {
      "csp": "default-src 'self'; img-src 'self' asset: http://asset.localhost data:; style-src 'self' 'unsafe-inline'",
      "assetProtocol": { "enable": true, "scope": ["$LOCALDATA/Hangline/**", "$PICTURE/Screenshots/**", "$VIDEO/Captures/**"] }
    }
  },
  "bundle": {
    "active": true,
    "targets": ["nsis"],
    "icon": ["icons/icon.ico"],
    "windows": { "webviewInstallMode": { "type": "embedBootstrapper" } }
  }
}
```

`src-tauri/src/lib.rs`（最小可运行版本，后续任务在此追加）：
```rust
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // single-instance 必须最先注册：第二个实例要在其余插件 setup 之前被检测并退出
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let _ = app.emit("second-instance", ());
        }))
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .run(tauri::generate_context!())
        .expect("error while running Hangline");
}
```
`src-tauri/src/main.rs`：
```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() { hangline_lib::run(); }
```
（`Cargo.toml` 中 `[lib] name = "hangline_lib"`，`main.rs` 调用它。）

- [ ] **Step 6: 验证质量闸门**

```powershell
cd %USERPROFILE%\Documents\deepseek-harness\default-workspace\hangline
pnpm check
cd src-tauri; cargo check
```
Expected：`pnpm check` 输出 `version ok: 0.1.0` / `i18n ok: 4 keys, 1 languages`；`cargo check` 成功。

- [ ] **Step 7: 提交**

```powershell
cd %USERPROFILE%\Documents\deepseek-harness\default-workspace\hangline
git add -A; git commit -m "chore: scaffold Tauri 2 + React/TS project with quality gates"
```

---

## Task 2: P0-A 透明置顶面板 + 点击穿透（E1/E2）

**Files:**
- Create: `src-tauri/src/panel.rs`, `src-tauri/src/hittest.rs`（先纯函数版）, `src-tauri/src/geometry.rs`（先只放 `PhysicalRect`/`LogicalRect`/`any_hit`）
- Modify: `src-tauri/src/lib.rs`, `src/main.tsx`, `src/windows/panel/Panel.tsx`, `src/styles/base.css`
- Create: `docs/experiments/P0-E1E2.md`
- Test: `src-tauri/src/geometry.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1 的工程与 `index.html?window=` 约定。
- Produces: `geometry::PhysicalRect { x: i32, y: i32, w: i32, h: i32 }`、`geometry::any_hit(cursor: (i32,i32), rects: &[PhysicalRect]) -> bool`；`panel::create(app) -> tauri::Result<WebviewWindow>`；IPC command `set_hit_rects(rects: Vec<PhysicalRect>)`；前端每帧布局后上报。

- [ ] **Step 1: 写失败的几何单测**

`src-tauri/src/geometry.rs`：
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhysicalRect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

pub fn any_hit(cursor: (i32, i32), rects: &[PhysicalRect], pad: i32) -> bool {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hit_inside_and_outside() {
        let r = [PhysicalRect { x: 100, y: 20, w: 150, h: 120 }];
        assert!(any_hit((150, 80), &r, 0));
        assert!(!any_hit((99, 80), &r, 0));
        assert!(any_hit((98, 80), &r, 4));      // 4dp 外扩
        assert!(!any_hit((95, 80), &r, 4));
        assert!(!any_hit((150, 200), &r, 4));
    }
}
```

- [ ] **Step 2: 运行单测确认失败**

```powershell
cd src-tauri; cargo test geometry::tests::hit_inside_and_outside
```
Expected：FAIL（`not yet implemented`）。

- [ ] **Step 3: 实现 hit-test 与面板窗口**

`geometry.rs` 实现：
```rust
pub fn any_hit(cursor: (i32, i32), rects: &[PhysicalRect], pad: i32) -> bool {
    rects.iter().any(|r| {
        cursor.0 >= r.x - pad && cursor.0 < r.x + r.w + pad
            && cursor.1 >= r.y - pad && cursor.1 < r.y + r.h + pad
    })
}
```

`panel.rs`：
```rust
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let win = WebviewWindowBuilder::new(app, "panel", WebviewUrl::App("index.html?window=panel".into()))
        .title("Hangline")
        .inner_size(1440.0, 210.0)
        .position(0.0, 0.0)
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .focused(false)
        .visible(true)
        .build()?;
    win.set_ignore_cursor_events(true).ok();
    Ok(win)
}
```

`hittest.rs`（30Hz 轮询，状态变化才调用 API）：
```rust
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use tauri::{AppHandle, Manager};
use crate::geometry::{any_hit, PhysicalRect};

pub fn spawn(app: AppHandle, rects: Arc<std::sync::Mutex<Vec<PhysicalRect>>>) {
    std::thread::spawn(move || {
        let mut last_ignore = true;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(33));
            let Some(panel) = app.get_webview_window("panel") else { continue };
            let Ok(cursor) = panel.cursor_position() else { continue };
            let Ok(pos) = panel.outer_position() else { continue };
            let local = (cursor.x as i32 - pos.x, cursor.y as i32 - pos.y);
            let hit = any_hit(local, &rects.lock().unwrap(), 4);
            if hit == last_ignore {
                last_ignore = !hit;
                let _ = panel.set_ignore_cursor_events(!hit);
            }
        }
    });
}
```

`lib.rs` 的 setup：创建面板、注册 `set_hit_rects` command、启动 hittest。

- [ ] **Step 4: 前端面板骨架**

`src/windows/panel/Panel.tsx`：渲染一个 150×104 的占位卡片，用 `ResizeObserver`/`useLayoutEffect` 把卡片 `getBoundingClientRect()` 换算成物理像素后 `invoke("set_hit_rects")`：
```tsx
const scale = await getScaleFactor(); // invoke("get_scale_factor")
```
（Task 2 先由 Rust 提供 `get_scale_factor` command，返回 `panel.scale_factor()`。）

- [ ] **Step 5: 运行并执行 E1/E2 手动验证**

```powershell
pnpm tauri dev
```
按 `docs/experiments/P0-E1E2.md` 的清单逐项验证并记录（无黑底/白底、卡片可点、其余区域点击穿透到桌面、进出卡片无可感闪烁、隐藏时无残留窗口）。

- [ ] **Step 6: 提交**

```powershell
git add -A; git commit -m "feat(p0): transparent top panel with dynamic click-through (E1/E2)"
```

---

## Task 3: P0-B 全局热键 + 托盘 + 单实例（E3）

**Files:**
- Create: `src-tauri/src/hotkeys.rs`, `src-tauri/src/tray.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/panel.rs`（显隐函数）
- Test: `src-tauri/src/hotkeys.rs` 内 `#[cfg(test)]`（快捷键字符串归一化）
- Create: `docs/experiments/P0-E3.md`

**Interfaces:**
- Consumes: Task 2 的 `panel::create`。
- Produces: `hotkeys::normalize(&str) -> Result<String>`（接受 `ctrl+alt+s`/`Ctrl+Alt+S`，输出 `Ctrl+Alt+S`）；`hotkeys::register(app, "capture"|"toggle", accel) -> Result<()>`；托盘菜单项 id 常量（`tray::ID_SHOW`, `tray::ID_QUIT`, ...）。

- [ ] **Step 1: 写失败的归一化单测**

```rust
#[test]
fn normalizes_case_and_order() {
    assert_eq!(normalize("ctrl+alt+s").unwrap(), "Ctrl+Alt+S");
    assert_eq!(normalize("ALT+CTRL+T").unwrap(), "Ctrl+Alt+T");
    assert!(normalize("S").is_err());
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cd src-tauri; cargo test hotkeys::tests::normalizes_case_and_order
```
Expected：FAIL。

- [ ] **Step 3: 实现热键/托盘/单实例**

`hotkeys.rs`：解析修饰键集合 {Ctrl, Alt, Shift, Win}，要求至少 2 个修饰键 + 1 个主键；使用 `tauri_plugin_global_shortcut::GlobalShortcutExt`：
```rust
pub fn register(app: &tauri::AppHandle, target: Target, accel: &str) -> anyhow::Result<()> {
    let shortcut = normalize(accel)?;
    app.global_shortcut().register(shortcut.as_str())?;
    Ok(())
}
```
（`Target` 枚举区分 `Capture`/`Toggle`；handler 中按 target 调用 `capture::start` 或 `panel::toggle`。）

`tray.rs`：`TrayIconBuilder` + 最小菜单（Show/Hide、Quit），`on_menu_event` 处理；菜单文字随 `panel.is_visible()` 更新。

`lib.rs`：`.plugin(single_instance)` 回调中 `panel::toggle(&app)`（第二次启动唤起）；`setup` 里创建托盘、注册两个热键、`start_hit_test`。

- [ ] **Step 4: 执行 E3 手动验证**

清单：热键占用时（先开一个占用 `Ctrl+Alt+S` 的小工具或临时注册同键）注册失败返回错误、回滚旧键并弹托盘气泡；不抢焦点；二级启动唤起已有实例；托盘菜单可显示/隐藏线。

- [ ] **Step 5: 写实验报告并提交**

`docs/experiments/P0-E3.md` 记录设备、系统版本、结果与截图路径。

```powershell
git add -A; git commit -m "feat(p0): global hotkeys, tray and single instance (E3)"
```

---

## Task 4: P0-C 原生文件拖出（E4）

> **实现修订（2026-10-29，P0 实测）**：Tauri 2.12 的 `WebviewWindow::start_dragging()` 语义是「拖动窗口」，**没有向外拖文件的 API**（tauri/tauri-runtime/wry 中不存在 CF_HDROP/IDropSource/IDataObject/DoDragDrop；`DragDropEvent` 只处理入向拖放）。E4 主路径改为**自研 OLE 拖出**（`windows` crate 已是项目依赖，不新增第三方包）：实现 `IDataObject`（CF_HDROP + DROPFILES）、`IDropSource`（QueryContinueDrag 检测左键/Esc）、`DoDragDrop` 模态循环；前端在 pointermove 时 `preventDefault` 阻止 WebView2 自己的 HTML5 拖放。Step 1 下方的 `window.start_dragging(...)` 示例作废。若 OLE 在 STA/消息循环上有无法解决的阻塞，才允许报告 BLOCKED 交由 controller 决定。

**Files:**
- Create: `src-tauri/src/dragout.rs`; `docs/experiments/P0-E4.md`
- Modify: `src-tauri/src/lib.rs`（注册 command）、`src/windows/panel/Panel.tsx`（占位卡片可拖）
- Test: 手动矩阵（资源管理器/微信/桌面/Figma/取消）

**Interfaces:**
- Consumes: Task 2 的占位卡片；Task 1 的 `windows` crate。
- Produces: `#[tauri::command] fn start_drag(window: WebviewWindow, path: String) -> Result<(), String>`；前端 `onPointerDown` → 移动 4dp → `invoke("start_drag", { path })`。

- [ ] **Step 1: 实现自研 OLE 拖出（Tauri 无文件拖出 API，示例代码作废）**

```rust
// dragout.rs —— 用 windows crate 实现 OLE 拖出源
pub fn drag_file(hwnd: HWND, path: &Path) -> anyhow::Result<DragEffect> {
    // 必须在 STA 线程且 OleInitialize 之后调用（DoDragDrop 自带模态消息循环）
    let data = FileDataObject::new(path)?;   // unsafe { implement IDataObject }：CF_HDROP（DROPFILES + UTF-16 双 \0）
    let source = FileDropSource::new()?;     // unsafe { implement IDropSource }：QueryContinueDrag 检测左键/Esc
    let mut effect = DROPEFFECT_COPY;
    let hr = unsafe {
        DoDragDrop(&data.to_interface(), &source.to_interface(),
                   DROPEFFECT_COPY | DROPEFFECT_MOVE, &mut effect)
    };
    if hr == DRAGDROP_S_DROP { Ok(effect.into()) } else { Ok(DragEffect::Cancelled) }
}
```
要点：`GlobalAlloc(GMEM_MOVEABLE)` 分配 DROPFILES 数据；`GetData` 必须 `ReleaseStgMedium` 兼容；`Dragging` 原子标志覆盖整个 DoDragDrop 期间（hittest 读它）；前端 `pointermove` 里 `preventDefault()` 防止 WebView2 触发原生 HTML5 拖放。
（以 Tauri 2.12 的 `start_dragging` 实际签名为准；若签名不同，按 `cargo doc` 修正——本任务允许因 API 差异调整调用方式，但行为契约不变。）

- [ ] **Step 2: 前端拖拽启动**

鼠标按下记录坐标，`pointermove` 超过 4dp 时调用 `start_drag`；拖拽期间设置全局 `dragging=true` 通知 Rust 暂停穿透切换（避免拖拽中卡片变为穿透）。

- [ ] **Step 3: 执行 E4 手动矩阵**

资源管理器、桌面、微信、Figma、Chrome 下载区、以及「拖到无人接收处松手」；记录每种目标的粘贴结果（副本/移动/取消）。

- [ ] **Step 4: 若主路径不满足判据，记录回退需求**

在报告中明确：Tauri `start_dragging` 是否支持拖拽图像、是否阻塞到拖拽结束、是否支持 `CF_HDROP`。若任一关键项不满足，报告标注「需在 Task 13 实现 OLE fallback」，并把具体 API 差异写入报告（不要在本任务写 fallback 代码）。

- [ ] **Step 5: 提交**

```powershell
git add -A; git commit -m "feat(p0): native file drag-out (E4)"
```

---

## Task 5: P0-D 多屏 GDI 截屏 + DPI 几何（E5）

**Files:**
- Create: `src-tauri/src/capture.rs`; `docs/experiments/P0-E5.md`
- Modify: `src-tauri/src/geometry.rs`（加显示器换算纯函数与测试）、`src-tauri/src/lib.rs`
- Test: `geometry` 单测（DPI 换算、跨屏负坐标、裁剪矩形）；手动 E5

**Interfaces:**
- Consumes: Task 1 依赖（`image`、`windows`）。
- Produces: `capture::grab_virtual_screen() -> anyhow::Result<image::RgbaImage>`；`capture::crop(img, PhysicalRect) -> RgbaImage`；`capture::save_png(dir, img) -> Result<PathBuf>`；`geometry::monitor_at(point) -> MonitorInfo`、`geometry::to_logical(rect, scale) -> LogicalRect`。

- [ ] **Step 1: 写失败的几何单测**

```rust
#[test]
fn converts_physical_to_logical_with_scale() {
    let r = PhysicalRect { x: 100, y: 50, w: 300, h: 200 };
    assert_eq!(to_logical(r, 1.5), LogicalRect { x: 66.67, y: 33.33, w: 200.0, h: 133.33 });
}
```
（浮点断言用 `(a - b).abs() < 0.01`。）

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test geometry::tests::converts_physical_to_logical_with_scale
```
Expected：FAIL。

- [ ] **Step 3: 实现 GDI 截屏**

```rust
pub fn grab_virtual_screen() -> anyhow::Result<image::RgbaImage> {
    // SM_XVIRTUALSCREEN/Y/CX/CY 取虚拟屏幕；GetDC(NULL) + CreateCompatibleDC + CreateCompatibleBitmap
    // + BitBlt(SRCCOPY) + GetDIBits(32bpp 自顶向下) → RgbaImage::from_raw
    // 所有 HWND/HDC/GDI 对象用 RAII guard 释放
    todo!()
}
```
（实现时按上述 API 顺序落地；本步骤只允许 `todo!()` 出现在计划文档，实际提交必须是完整实现。）

- [ ] **Step 4: 实现保存与裁剪**

`crop` 用 `image::imageops::crop_imm`；`save_png` 文件名 `Hangline yyyy-MM-dd HH.mm.ss.png`（本地时区），目录 `%LOCALAPPDATA%\Hangline\Screenshots`，原子写（先写 `.tmp` 再 `rename`）。

- [ ] **Step 5: E5 手动验证**

双屏（100%+150%）各截一张；跨屏框选；确认 PNG 尺寸/颜色正确、无黑边；记录到 `docs/experiments/P0-E5.md`。

- [ ] **Step 6: 提交**

```powershell
git add -A; git commit -m "feat(p0): multi-monitor GDI capture and DPI geometry (E5)"
```

---

## Task 6: P0 门禁报告

**Files:**
- Create: `docs/experiments/P0-report.md`
- Modify: `docs/superpowers/specs/2026-10-28-hangline-windows-design.md` 仅当需要记录「已验证」结论（不改需求）

**Interfaces:**
- Consumes: Task 2–5 的实验记录。
- Produces: 一份明确 go/no-go 的报告；`go` 才允许进入 Task 7。

- [ ] **Step 1: 汇总六项实验结论**

逐项写：环境（Windows 版本、GPU、缩放、显示器）、操作步骤、期望、实际、证据（截图路径/日志片段）、结论（pass/fail）。

- [ ] **Step 2: 记录决策**

若全部 pass：写「进入 P1」。若 E1/E2/E4 任一 fail：写「切 Electron 兜底，前端代码保留，Rust 服务改 Node 侧实现」，并停止本计划、向用户汇报。

- [ ] **Step 3: 提交**

```powershell
git add -A; git commit -m "docs(p0): validation report and go/no-go decision"
```

---

## Task 7: 数据层（设置、条目、IPC 状态）

**Files:**
- Create: `src-tauri/src/item.rs`, `src-tauri/src/settings.rs`, `src-tauri/src/store.rs`, `src-tauri/src/app_state.rs`, `src-tauri/src/ipc.rs`
- Modify: `src-tauri/src/lib.rs`（注册 command）
- Create: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/lib/state.ts`
- Test: `settings.rs`、`store.rs` 内联单测；`src/lib/types.test.ts` 可省略（类型无运行时）

**Interfaces:**
- Consumes: Task 1 依赖 `serde`/`serde_json`；`%LOCALAPPDATA%`。
- Produces:
  - Rust：`Settings`（字段与 spec §3.6 完全一致）、`Item { id, path, kind, hash: u64, width, height, created_at, added_at }`、`AppState { items, settings, version }`；
  - Commands：`get_app_state() -> AppState`、`set_setting(key: String, value: serde_json::Value) -> Result<AppState>`；
  - Events：`state`（全量）。
  - TS：`AppState`/`Item`/`Settings` 类型、`invokeState()`、`useAppStore`。

- [ ] **Step 1: 写失败的 settings 单测**

```rust
#[test]
fn defaults_match_spec() {
    let s = Settings::default();
    assert_eq!(s.capture_hotkey, "Ctrl+Alt+S");
    assert_eq!(s.toggle_hotkey, "Ctrl+Alt+T");
    assert_eq!(s.keep_on_line, KeepOnLine::Fit);
    assert_eq!(s.reveal_peek_ms, 2500);
    assert!(s.watch_clipboard);
    assert!(!s.start_at_login);
}
#[test]
fn unknown_key_is_rejected() {
    let mut s = Settings::default();
    assert!(s.set_json("nope", serde_json::json!(1)).is_err());
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test settings::tests
```
Expected：FAIL（类型不存在）。

- [ ] **Step 3: 实现模型与原子持久化**

`settings.rs`：`Settings` derive `Serialize/Deserialize/Clone/Debug`；`load(dir)` 读 JSON，失败用默认值并写回；`save_atomic(dir, &Settings)`（`.tmp` + `rename`）；`set_json` 白名单校验。

`store.rs`：`load(dir) -> Vec<Item>`（过滤 `path.exists()` 为 false 的项）；`save_atomic`；`prune(&mut Vec<Item>) -> Vec<String>` 返回被移除 id。

`app_state.rs`：`AppState` 包在 `Mutex<Inner>`；`emit_state(app)` 广播 `state` 事件。

- [ ] **Step 4: 实现 command 与前端桥**

`ipc.rs`：
```rust
#[tauri::command]
pub fn get_app_state(state: tauri::State<'_, AppState>) -> crate::app_state::PublicState { state.snapshot() }

#[tauri::command]
pub fn set_setting(key: String, value: serde_json::Value, app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<crate::app_state::PublicState, String> {
    state.set_json(&key, value).map_err(|e| e.to_string())?;
    let snap = state.snapshot();
    app.emit("state", &snap).ok();
    Ok(snap)
}
```

- [ ] **Step 5: 跑通前后端握手**

```powershell
pnpm tauri dev
```
Expected：控制台打印 `state loaded: 0 items`（通过 `console.log` 在 `useAppStore` 初始化处输出）；修改设置后重启仍保留。

- [ ] **Step 6: 提交**

```powershell
git add -A; git commit -m "feat: settings, item store and IPC state layer"
```

---

## Task 8: 目录监听与去重

**Files:**
- Create: `src-tauri/src/watchers/mod.rs`, `src-tauri/src/watchers/dirs.rs`, `src-tauri/src/dedupe.rs`
- Modify: `src-tauri/src/lib.rs`（setup 启动 watcher）、`src-tauri/src/app_state.rs`（新增条目 API）
- Test: `dirs.rs` 候选过滤单测、`dedupe.rs` LRU 单测；临时目录集成测试

**Interfaces:**
- Consumes: Task 7 的 `Item`、`AppState::add_item`。
- Produces: `dirs::start(app, folder, opts)`；`dirs::is_candidate(path, opts) -> bool`；`dedupe::Deduper::check_or_insert(hash) -> bool`（true = 新）。

- [ ] **Step 1: 写失败的过滤单测**

```rust
#[test]
fn candidate_rules() {
    assert!(is_candidate(Path::new("a.PNG"), Opts::system_screenshots()));
    assert!(is_candidate(Path::new("a.mp4"), Opts::recordings()));
    assert!(!is_candidate(Path::new("a.txt"), Opts::system_screenshots()));
    assert!(!is_candidate(Path::new(".hidden.png"), Opts::watch_any_image()));
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test watchers::dirs::tests
```
Expected：FAIL。

- [ ] **Step 3: 实现 watcher**

`notify` 递归监听目录；事件防抖 200ms（`tokio::time::sleep` 或 std 线程 + `Instant`）；过滤隐藏文件/临时文件；`on_new(path)` 读文件计算 BLAKE3、`image` 读尺寸、`AppState::add_item`、`emit state`、`reveal` 事件。

监听集合：`%USERPROFILE%\Pictures\Screenshots`、`%USERPROFILE%\Videos\Captures`、`%LOCALAPPDATA%\Hangline\Screenshots`、用户额外目录。

- [ ] **Step 4: 集成测试（临时目录）**

Rust 测试：创建临时目录 → 启动 watcher（测试开关，监听事件）→ 写入 `a.png`（1×1 PNG 字节）→ 2 s 内收到回调；写 `a.txt` 不触发；重复写同路径只触发一次。

- [ ] **Step 5: 手动验证**

资源管理器向 `图片\Screenshots` 复制一张 png → 面板出现一张卡片；同一张图再从剪贴板复制的场景在 Task 9 验证去重。

- [ ] **Step 6: 提交**

```powershell
git add -A; git commit -m "feat: screenshot/recording folder watchers with hash dedupe"
```

---

## Task 9: 剪贴板监听

**Files:**
- Create: `src-tauri/src/watchers/clipboard.rs`
- Modify: `src-tauri/src/watchers/mod.rs`, `src-tauri/src/lib.rs`
- Test: `should_accept` 决策表单测

**Interfaces:**
- Consumes: Task 8 的 `Deduper`、`AppState::add_item`；Task 7 的 settings `watch_clipboard`。
- Produces: `clipboard::should_accept(formats: &[&str]) -> bool`（纯函数）；`clipboard::start(app)`（消息窗 + 300ms 轮询兜底）。

- [ ] **Step 1: 写失败的决策表单测**

```rust
#[test]
fn accepts_pure_images_only() {
    assert!(should_accept(&["PNG"]));
    assert!(should_accept(&["CF_DIBV5"]));
    assert!(!should_accept(&["PNG", "CF_UNICODETEXT"]));
    assert!(!should_accept(&["PNG", "CF_HDROP"]));
    assert!(!should_accept(&["PNG", "ExcludeClipboardContentFromMonitorProcessing"]));
    assert!(!should_accept(&["PNG", "Clipboard Viewer Ignore"]));
    assert!(!should_accept(&[]));
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test watchers::clipboard::tests
```
Expected：FAIL。

- [ ] **Step 3: 实现监听与保存**

消息窗线程：`RegisterClassW` + `CreateWindowExW(HWND_MESSAGE)` + `AddClipboardFormatListener(hwnd)` + `GetMessageW` 循环；收到 `WM_CLIPBOARDUPDATE` → `handle_clipboard(app)`（读格式 → 决策 → 解码 RGBA → 去重 → 保存 PNG → `add_item`）。
兜底：若消息窗创建失败，起 300ms 轮询线程比较 `GetClipboardSequenceNumber`。

- [ ] **Step 4: 手动验证**

复制纯图片（Win+Shift+S / 微信 / Excel 单图）→ 上栏；复制浏览器图片 / Excel 区域（带 HTML 或文本）→ 按过滤规则不采集；复制文件 → 不采集；两次复制同一张图 → 只一条。

- [ ] **Step 5: 提交**

```powershell
git add -A; git commit -m "feat: clipboard image watcher with privacy filters"
```

---

## Task 10: 面板静态 UI（绳、夹子、卡片、缩略图）

**Files:**
- Create: `src/components/Rope.tsx`, `src/components/GlassCard.tsx`, `src/components/ClothesPin.tsx`, `src/windows/panel/Panel.tsx`, `src/windows/panel/Panel.css`, `src/lib/layout.ts`, `src/lib/layout.test.ts`
- Modify: `src/lib/state.ts`（渲染 items）、`src/styles/base.css`、`src-tauri/src/lib.rs`（`get_scale_factor` command；缩略图走 asset protocol，新增目录时用 `asset_protocol_scope().allow_directory` 放行）
- Test: `layout.test.ts`（容量/坐标/尺寸档）

**Interfaces:**
- Consumes: Task 7 的 `AppState.items`、Task 2 的 `set_hit_rects`。
- Produces: `layout::panelHeight(size) -> number`、`layout::cardWidth(size) -> number`、`layout::capacity(panelWidth, size) -> number`、`layout::cardX(index, capacity, size, panelWidth) -> number`；`GlassCard({ item, index, onCopy, onOpen, onRemove })`。

- [ ] **Step 1: 写失败的布局单测**

```ts
import { describe, expect, it } from "vitest";
import { capacity, cardWidth, panelHeight } from "./layout";

describe("layout", () => {
  it("matches spec geometry", () => {
    expect(cardWidth("small")).toBe(120);
    expect(cardWidth("medium")).toBe(150);
    expect(cardWidth("large")).toBe(202);
    expect(panelHeight("medium")).toBe(210);
    expect(capacity(1024, "medium")).toBe(4);    // floor((1024-200)/174)=4
    expect(capacity(500, "medium")).toBe(3);     // 下限 3
    expect(capacity(1920, "medium")).toBe(9);    // floor(1720/174)=9（spec 公式；cap 12 在 ≥2288 宽才出现）
    expect(capacity(2288, "medium")).toBe(12);   // 首次触到 12 上限
  });
});
```

- [ ] **Step 2: 运行确认失败**

```powershell
pnpm test src/lib/layout.test.ts
```
Expected：FAIL（模块不存在）。

- [ ] **Step 3: 实现 layout 与组件**

`layout.ts`：
```ts
export type CardSize = "small" | "medium" | "large";
const SCALE = { small: 0.8, medium: 1.0, large: 1.35 } as const;
export const cardWidth = (s: CardSize) => (s === "medium" ? 150 : s === "small" ? 120 : 202);
export const photoMaxHeight = (s: CardSize) => Math.round(104 * SCALE[s]);
export const panelHeight = (s: CardSize) => photoMaxHeight(s) + 106;
export const capacity = (panelWidth: number, s: CardSize) =>
  Math.max(3, Math.min(12, Math.floor((panelWidth - 200) / (cardWidth(s) + 24))));
```

`Rope.tsx`：SVG `<path d="M0,10 Q{width/2},{10+4*sag*0.25}, {width},10" />` + 三层 stroke + 渐变 mask。
`GlassCard.tsx`：`background: linear-gradient(...)` + `backdrop-filter: blur(18px) saturate(1.2)`（模拟玻璃）+ 高光描边 + 阴影；图片用 `convertFileSrc(path)`。
`ClothesPin.tsx`：9×26 dp 的 CSS/SVG 夹子（渐变金属）。

- [ ] **Step 4: 渲染真实条目**

`Panel.tsx`：`items.slice(-capacity)` 渲染；`useLayoutEffect` 计算每张卡片 physical rect 并 `invoke("set_hit_rects")`；滚动容器监听 `wheel`（横向滚动）与 `scroll` 后重报 rect。

- [ ] **Step 5: 手动验证**

向 watcher 目录放 3 张图 → 面板出现 3 张玻璃卡片、绳有悬垂、夹子居中；深浅色跟随系统；截图 1920/2560 宽下容量正确。

- [ ] **Step 6: 提交**

```powershell
git add -A; git commit -m "feat: panel static UI with rope, glass cards and layout model"
```

---

## Task 11: 展开/收起状态机与自动展开

**Files:**
- Create: `src-tauri/src/reveal.rs`, `src/lib/spring.ts`, `src/lib/spring.test.ts`
- Modify: `src-tauri/src/lib.rs`、`src-tauri/src/panel.rs`、`src/windows/panel/Panel.tsx`、`src/windows/panel/Panel.css`
- Test: `reveal.rs` 状态转换单测（纯逻辑）、`spring.test.ts` 收敛与稳定性

**Interfaces:**
- Consumes: Task 10 的面板 UI；Task 3 的 `panel::toggle`。
- Produces: `reveal::State { wanted, revealed, pinned, peek_until, dismissed_until_leave }`；`reveal::on_new_item(state, now) -> bool`（应展开）；`reveal::on_pointer_leave(state, now) -> bool`（应收起）；Rust → 前端事件 `reveal`/`tuck`；前端 CSS class `.revealed` 驱动位移。

- [ ] **Step 1: 写失败的状态机单测**

```rust
#[test]
fn new_item_peeks_then_tucks_after_delay() {
    let mut s = State::default();
    assert!(on_new_item(&mut s, 0));           // 新条目 → 展开
    assert!(!on_pointer_leave(&mut s, 1000));  // peek 未结束 → 不收起
    assert!(on_pointer_leave(&mut s, 2600));   // peek 结束 + 离开 → 收起
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test reveal::tests
```
Expected：FAIL。

- [ ] **Step 3: 实现状态机与事件**

`reveal.rs` 纯函数 + `RevealController` 持有状态；30Hz 轮询线程在面板可见时检测「指针是否在面板+向下 40dp 区域」与「左键是否在面板外按下」；触发 `reveal`/`tuck` 事件并 `panel.show()/hide()`（隐藏时 `hide()`，不留隐形窗口）。

展开动画：先写 `spring.ts` 单测（`stiffness 224 / damping 25` 下 700ms 内收敛到目标 ±0.5 且不发散），再实现半隐式欧拉积分器 `tick(state, target, stiffness, damping, dt)`；面板用 rAF 驱动 `translateY`。收起用 `easeIn 220ms`（CSS transition）。

- [ ] **Step 4: 手动验证**

新截图 → 自动亮 2.5s → 离开收起；`Ctrl+Alt+T` → pinned 展开 → 点击桌面收起；空线按热键 → 不展开（或展开即收，按 spec §3.4）。

- [ ] **Step 5: 提交**

```powershell
git add -A; git commit -m "feat: reveal/tuck state machine with peek and dismiss suppression"
```

---

## Task 12: 复制 / 打开 / 移入回收站

**Files:**
- Create: `src-tauri/src/clipboard_io.rs`, `src-tauri/src/trash.rs`, `src/components/CopiedPill.tsx`
- Modify: `src-tauri/src/lib.rs`（commands）、`src/components/GlassCard.tsx`（点击/双击/叉）、`src/lib/state.ts`
- Test: `clipboard_io.rs` 格式规划单测（`formats_for(item) -> Vec<ClipFormat>`），`trash.rs` 路径校验单测
- Test: 手动 M8/M11

**Interfaces:**
- Consumes: Task 7 的 `Item`；Task 10 的卡片组件。
- Produces: `clipboard_io::copy_image(path) -> anyhow::Result<()>`、`clipboard_io::copy_file(path)`；`trash::recycle(path) -> Result<()>`；commands `copy_item`, `open_item`, `take_down`。

- [ ] **Step 1: 写失败的格式规划单测**

```rust
#[test]
fn image_items_advertise_bitmap_and_file() {
    let f = formats_for(&ItemKind::Capture);
    assert!(f.contains(&ClipFormat::DibV5));
    assert!(f.contains(&ClipFormat::Png));
    assert!(f.contains(&ClipFormat::FileDrop));
}
#[test]
fn recording_only_ships_file_drop() {
    assert_eq!(formats_for(&ItemKind::Recording), vec![ClipFormat::FileDrop]);
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test clipboard_io::tests
```
Expected：FAIL。

- [ ] **Step 3: 实现剪贴板写入**

`OpenClipboard(panel_hwnd)`（**用面板窗口的 HWND 作为 owner；不要用 NULL**——`OpenClipboard(NULL)` 在 Windows 上会间歇性让 Set/Get/Close 失败并报 `ERROR_CLIPBOARD_NOT_OPEN`）→ `EmptyClipboard` → 依次 `SetClipboardData`：
- `CF_DIBV5`：`BITMAPV5HEADER` + BGRA 像素（top-down，`biHeight` 为负——与 CF_DIB 约定兼容，已验证）；
- `"PNG"`：注册格式后的 PNG 字节（`RegisterClipboardFormatW("PNG")`）；
- `CF_HDROP`：`DROPFILES` 结构 + UTF-16 路径 `\0\0`。
写完后用 `IsClipboardFormatAvailable` 逐个校验；缺失任一期望格式则重试整套写入（≤2 次），仍失败返回 Err（前端不弹 Copied），最后 `CloseClipboard`。所有句柄用 `GMEM_MOVEABLE` 分配，失败时 `GlobalFree`。成功时 `app.emit("clipboard-copied", id)`（spec §4.4 事件契约）。

- [ ] **Step 4: 实现回收站**

```rust
pub fn recycle(path: &Path) -> anyhow::Result<()> {
    // CoInitializeEx(COINIT_APARTMENTTHREADED) → CoCreateInstance(CLSID_FileOperation)
    // → SetOperationFlags(FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION)
    // → CreateItemFromParsingName(path) → PerformOperations() → 检查 HRESULT
    todo!()
}
```
（提交时必须是完整实现。）

- [ ] **Step 5: 前端接线**

单击 → `invoke("copy_item", { id })` 并显示 `Copied` 胶囊 1.2s；双击 → `open_item`；叉 → `take_down({ id, deleteFile })`。

- [ ] **Step 6: 手动验证 M8/M11**

微信/浏览器粘贴得到图片；双击用默认应用打开；叉 → 文件进回收站、卡片消失；右键「取下线」不删文件。

- [ ] **Step 7: 提交**

```powershell
git add -A; git commit -m "feat: clipboard copy, open and recycle-bin delete"
```

---

## Task 13: 拖出语义与 prune 规则

**Files:**
- Modify: `src-tauri/src/dragout.rs`（对外 effect 改为 COPY|MOVE，暴露 drag effect）、`src-tauri/src/lib.rs`（`after_drag` command：600 ms 后 `prune` + `emit_state`）、`src/windows/panel/Panel.tsx`（`drag-ended` 后调用 `after_drag`）、`src-tauri/src/store.rs`（prune 已有）
- Test: `store.rs` prune 单测、`dragout.rs` 的 `should_prune` 决策表单测、Manual M9/M10/M11

**Interfaces:**
- Consumes: Task 4 的 OLE 拖出与 `drag-ended {ok, effect}`、Task 7/8 的 `store::prune` + `app_state::emit_state`。
- Produces: `#[tauri::command] after_drag(id: String)`；纯函数 `should_prune(effect: DragEffect, file_exists: bool) -> bool`；`DragEffect { Copy, Move, None }`。

- [ ] **Step 1: 写失败的单测**

```rust
#[test]
fn prune_drops_items_whose_files_are_gone() {
    let dir = tempdir().unwrap();
    let keep = dir.path().join("keep.png"); std::fs::write(&keep, b"x").unwrap();
    let gone = dir.path().join("gone.png");
    let mut items = vec![item(&keep), item(&gone)];
    let removed = prune(&mut items);
    assert_eq!(removed.len(), 1);
    assert_eq!(items.len(), 1);
}

#[test]
fn pruning_follows_effect_and_existence() {
    assert!(should_prune(DragEffect::Move, true));    // 文件夹/回收站 MOVE：文件已经被移走
    assert!(!should_prune(DragEffect::Copy, true));   // 拖进 app：副本，保留
    assert!(should_prune(DragEffect::Copy, false));   // 兜底：文件消失即掉落
    assert!(!should_prune(DragEffect::None, true));   // 取消
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test store::tests::prune_drops_items_whose_files_are_gone dragout::tests::pruning_follows_effect_and_existence
```
Expected：FAIL。

- [ ] **Step 3: 实现拖放语义与 after_drag**

`dragout.rs`：`DoDragDrop` 的允许 effect 改为 `DROPEFFECT_COPY | DROPEFFECT_MOVE`（不再只 COPY）；把最终 effect 映射为 `DragEffect::{Copy,Move,Link,None}` 放进现有 `drag-ended` 事件；纯 `should_prune(effect, file_exists)` 表：Move+exists→重试（不立即 prune）、Move+missing→true、Copy+exists→false、Copy+missing→true、Link→false、None+exists→false、None+missing→true（安全网）。`lib.rs`：`after_drag(id, effect)` 在后台线程 sleep 600 ms 后执行策略——Move 且文件仍在时每 400 ms 重试一次、最多 ~2 s（慢速 shell 移动），仍存在则保留并记日志；其余情况调用 `AppState::prune()`，有变化则 `app_state::emit_state`；未知 id no-op；返回立即完成（不阻塞 IPC）。`Panel.tsx`：从 `drag-ended` payload 取 `effect`，`invoke("after_drag", { id, effect })`（取消也调用）。

- [ ] **Step 4: 验证 M9/M10/M11**

拖到微信（COPY，源文件保留、卡片保留）；拖到文件夹/桌面（Explorer MOVE，600 ms 内卡片掉落，文件在目标处）；拖到回收站（shell 删除，卡片掉落，可在回收站还原）。物理鼠标在锁屏/远程会话下不可注入时记录 MANUAL-PENDING，先用程序化 OLE effect + prune 单测作为门禁证据，并写 `docs/experiments/P1-drag-prune.md`。

- [ ] **Step 5: 提交**

```powershell
git add -A; git commit -m "feat: drag-end pruning with move/copy semantics"
```
## Task 14: 容量、横向滚动与尺寸档

> **2026-10-30 控制器补充（承接 Task 10 遗留）**：
> 1. `get_monitors` 必须是纯查询；新增 `sync_panel_geometry()` 命令负责把面板移到工作区（当前/指针所在屏）并按工作区宽/高/DPI 重算窗口 size。`cardSize`/`keepOnLine` 设置变化后（`set_setting` 内或前端调用）必须重新 sync，否则 small→large 会裁切卡片。
> 2. 面板可见期间若指针跨屏（500 ms 去抖），重新 sync 到新屏；隐藏时不动。多屏 follow 不做逐帧追踪。
> 3. 横向滚动修正 Task 10 的 deltaX 双滚动：统一用非 passive `wheel` + `preventDefault` 的单一 offset 模型（取主轴，步长 1 张），不要让原生 `scrollLeft` 与 handler 叠加。
> 4. `visibleSlice(total, keep, capacity, offset) -> {start,end}` 加边界用例：total<keep、keep<capacity、keep>capacity、offset 越界 clamp、items 变化后 offset 复位。
> 5. 前端多屏日志改为打印所有 monitor + Rust 选中的 target（不要只打 primary）。
> 6. 运行时证据：≥30 张时滚轮/双向滚动、命中矩形同步；`set_setting cardSize` 后窗口高度 small 189 / large 246（100% 缩放）实测；指针跨屏后面板宽度/位置切换（150% 屏物理 1920 → 逻辑 1280）；真实物理指针为 MANUAL-PENDING。
> 7. **（2026-10-30 复审补充）** `set_setting`/显式 sync 必须 re-fit **面板当前所在显示器**（不是指针屏，避免托盘/设置操作把线传送到另一屏）；`reveal.rs` 的 follow 锚点每 tick 从窗口实际位置派生（外部移动后不会把 DISPLAY1 当作 applied 卡死）；follow 在 `busy || dragout::is_dragging()` 时必须跳过/延后（拖拽跨屏不得移动源窗口）；前端 hit-rect 采集要检测 `window.devicePixelRatio` 与缓存 scale 不一致时刷新并重报（等逻辑尺寸跨 DPI 也安全）；follow 轮询 ≤250 ms（dwell 仍 500 ms）；`lineShift` 抽纯函数并测试。

**Files:**
- Modify: `src/lib/layout.ts`, `src/windows/panel/Panel.tsx`, `src-tauri/src/settings.rs`（`keepOnLine`/`cardSize` 已存在）、`src-tauri/src/app_state.rs`
- Test: `layout.test.ts` 增加可见窗口与滚动偏移用例

**Interfaces:**
- Consumes: Task 10 的 `capacity`/`cardX`；Task 7 的 settings。
- Produces: `layout::visibleSlice(total, keep, capacity, offset) -> Range`；前端滚轮事件 → 偏移 → 重算 rect 并 `set_hit_rects`。

- [ ] **Step 1: 写失败的滚动单测**

```ts
it("keeps newest on the right and scrolls older in", () => {
  // maxItems=max(capacity,keep)=25；保留最新 25 张（索引 5..29），offset 0 显示最新 10 张
  expect(visibleSlice(30, 25, 10, 0)).toEqual({ start: 20, end: 30 });
  expect(visibleSlice(30, 25, 10, 5)).toEqual({ start: 15, end: 25 });
  expect(visibleSlice(30, 25, 10, 15)).toEqual({ start: 5, end: 15 });
  expect(visibleSlice(30, 25, 10, 999)).toEqual({ start: 5, end: 15 }); // offset clamp
});
```

- [ ] **Step 2: 运行确认失败**

```powershell
pnpm test src/lib/layout.test.ts
```
Expected：FAIL。

- [ ] **Step 3: 实现滚动与尺寸切换**

`onWheel` 调整 offset（步长 1 张），重排；`keepOnLine` 改变时重算 `maxItems`；`cardSize` 改变时重算面板高度与窗口 size；`overflow` 时最新固定在右侧，旧的从左侧滑入。

- [ ] **Step 4: 手动验证 M14**

保留 100 张 + 滚轮：滚动流畅、无错位、命中区域同步。

- [ ] **Step 5: 提交**

```powershell
git add -A; git commit -m "feat: capacity, horizontal scroll and card size"
```

---

## Task 15: 托盘完整菜单、设置联动与开机自启

> **2026-10-30 控制器补充（承接 Task 3/7/10/11/14 遗留）**：
> 1. **热键**：启动时必须从持久化 settings 注册（Task 3 只注册默认值）；`set_setting` 改 `captureHotkey`/`toggleHotkey` 后必须重注册（失败回滚旧键 + 托盘提示项/日志）；托盘「快捷键 ▸」显示当前键 + 打开最小「改键窗口」（`?window=shortcut`，捕获键 + 校验 normalize + 保存）。热键冲突 UI 用托盘菜单禁用项 + tooltip（Tauri TrayIcon 无 balloon API，不新增依赖）。
> 2. **监听重启**：`watchSystemScreenshots`/`watchRecordings`/`watchClipboard`/`extraFolders` 变化后要真正启停 watcher/clipboard listener（重启 API）；新增额外目录先 `watchers::allow_asset_dir` 再 watch；移除目录不撤销 asset scope（已知，卡片可能回退瓦片）。
> 3. **READY gating**：托盘/热键/二次启动的早于前端 ready 的 toggle/reveal 要排队，ready 后 flush；`on_ready` 不得 clobber 排队状态；前端永不 ready 时 3 s 超时兜底显示。
> 4. **Add Folder…** 用系统文件夹选择器（windows crate：`IFileOpenDialog`/`SHBrowseForFolderW`，不加依赖），取消返回 None，最多 3 个。
> 5. **语言**：Rust 侧 `i18n.rs`（en/zh-Hans）供托盘/菜单文案；语言切换重建托盘菜单并持久化；前端 i18n 同步切换。
> 6. **开机自启**：`tauri-plugin-autostart` 注册/注销 + 菜单勾选状态与真实注册状态一致；失败提示（托盘禁用项/日志）。
> 7. **托盘完整菜单**按 spec §3.5 全部项；`Take everything down` 走 Task 12 take_down（自有目录进回收站）；`Check for updates…` 用 ShellExecute 打开项目 Releases 页（app 自身不出网）；`Quit` 停 watchers/clipboard 后退出，Inbox 模式无需还原（Windows 未改系统设置）。
> 8. **卡右键菜单仍不在本任务范围**（spec §3.1 有，但 P1 DoD 未要求）：记录为 V1.1 已知缺口，不在 Task 15 偷做。
> 9. 运行时证据：菜单 en/zh-Hans 截图、逐项 toggle 生效 + 重启保留、watcher/clipboard 启停日志、add folder 选择器（人工可点则截图，否则 MANUAL-PENDING）、autostart 注册表项、改键窗口、Quit 优雅退出；M15 冲突场景。

**Files:**
- Modify: `src-tauri/src/tray.rs`、`src-tauri/src/settings.rs`、`src-tauri/src/lib.rs`、`src/lib/i18n.ts`（tray 文案走 Rust 侧 strings）
- Create: `src-tauri/src/i18n.rs`（Rust 侧菜单文案表）
- Test: `tray.rs` 菜单状态映射单测；手动 M15（热键冲突）与设置持久化

**Interfaces:**
- Consumes: Task 7 settings、Task 3 托盘骨架、Task 11 reveal、Task 12/13/14 行为。
- Produces: 完整菜单（spec §3.5）、`set_setting` 生效、`start_at_login` 注册、托盘文案本地化。

- [ ] **Step 1: 写失败的菜单映射单测**

```rust
#[test]
fn menu_labels_follow_state() {
    let s = Settings::default();
    let labels = menu_labels(&s, true /*line visible*/);
    assert_eq!(labels.show_line, "Hide line");
    assert!(labels.handle_screenshots);
    assert_eq!(labels.keep_on_line, KeepOnLine::Fit);
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test tray::tests
```
Expected：FAIL。

- [ ] **Step 3: 实现完整菜单与设置联动**

按 spec §3.5 逐项构建；`on_menu_event` 中调用 settings/reveal/autostart；`start_at_login` → `tauri_plugin_autostart::AutoLaunchManager`；`sound` 用 `MessageBeep`；语言菜单切换 `src-tauri/src/i18n.rs` 的文案表并重建菜单。

- [ ] **Step 4: 手动验证设置项**

逐项改设置 → 立即生效 + 重启保留；开机自启注册/注销（任务管理器启动项可见）；热键冲突场景（M15）。

- [ ] **Step 5: 提交**

```powershell
git add -A; git commit -m "feat: full tray menu, settings wiring and autostart"
```

---

## Task 16: 自建截图全流程（热键 → 冻结覆盖层 → 框选 → 上栏）

> **2026-11-02 控制器补充（承接 Task 3/5/8/10/14/15 遗留）**：
> 1. **坐标**：`crop()` 只接受图像相对坐标；本任务新增 `crop_virtual(img, PhysicalRect)`（减 `virtual_screen_rect().origin`）或等价换算，覆盖负原点双屏。冻结覆盖层用 `window.devicePixelRatio` 做 CSS↔物理换算（覆盖层跨屏时 DPR 可能不是 1），框选物理矩形直接来自实际光标位置，不假设 100%。
> 2. **CAPTUREBLT 会抓到面板自身**：抓屏前若面板可见先 `panel::hide()`，抓完按原可见性恢复（不破坏 reveal 状态）；覆盖层窗口在抓屏之后再创建。
> 3. **去重契约（Task 8）**：保存顺序必须 `encode 到 <final>.png.tmp → hash 精确文件字节（BLAKE3-64）→ Deduper::remember(hash) → rename 到最终名 → add_item + emit_state + reveal{newItem}`；失败路径 `forget(hash)`。
> 4. **热键**：把 Task 3 的 capture 动作从「日志 + capture-requested」替换为真正 `request_capture()`（Task 15 已做持久化/注册，不要破坏其接口）。
> 5. **窗口**：capture 窗口覆盖整个虚拟屏幕（含负坐标），无边框/置顶/可聚焦，加载 `index.html?window=capture`；Esc/右键取消；拖框显示尺寸；松开鼠标确认。
> 6. **文件名**：同秒冲突用后缀（与 clipboard 保存一致）；落盘目录 `capture::screenshots_dir()`。
> 7. **运行时证据**：CDP 驱动 overlay 框选（含跨屏/150% 区域）→ 文件生成、卡片上栏、watcher 不重复；Esc 取消无文件；真实物理热键与真实拖框为 MANUAL-PENDING。M1/M5 手动项汇总到 Task 17。

**Files:**
- Create: `src/windows/capture/Capture.tsx`, `src/windows/capture/Capture.css`
- Modify: `src/main.tsx`（window=capture 路由）、`src-tauri/src/capture.rs`（`start_capture`、`finish_capture`）、`src-tauri/src/lib.rs`（commands）、`src-tauri/src/hotkeys.rs`（capture target）
- Test: `geometry` 裁剪单测；手动 M1/M5

**Interfaces:**
- Consumes: Task 5 的 `grab_virtual_screen`/`crop`/`save_png`、Task 11 的自动展开、Task 8 的 watcher（自建目录）。
- Produces: command `request_capture()`、`finish_capture(rect: PhysicalRect)`、`cancel_capture()`；事件 `capture-ready`（含截图路径，前端用 `convertFileSrc` 显示冻结画面）。

- [ ] **Step 1: 写失败的裁剪单测**

```rust
#[test]
fn crop_clamps_to_image_bounds() {
    let img = image::RgbaImage::new(100, 80);
    let out = crop(&img, PhysicalRect { x: -10, y: -10, w: 40, h: 40 });
    assert_eq!(out.dimensions(), (30, 30));
}
```

- [ ] **Step 2: 运行确认失败**

```powershell
cargo test capture::tests::crop_clamps_to_image_bounds
```
Expected：FAIL。

- [ ] **Step 3: 实现截图覆盖层**

Rust：`grab_virtual_screen()` → 存临时 PNG（`%LOCALAPPDATA%\Hangline\capture-tmp.png`，已在 asset protocol scope 内）→ 创建全屏 `capture` 窗口（透明、置顶、可聚焦）→ 前端加载该图作为背景 + 半透明遮罩 + 拖框（显示尺寸提示）→ 回车/松开鼠标确认 → `finish_capture(rect)`（物理像素）→ `save_png` → 上栏 → 销毁窗口并删除临时图。
Esc / 右键取消。

- [ ] **Step 4: 接入自建目录 watcher 去重**

保存后把 `hash` 放入 `Deduper` 的「自建」集合，避免目录 watcher 重复上栏（spec §3.2 规则 2）。

- [ ] **Step 5: 手动验证 M1**

`Ctrl+Alt+S` → 框选 → 上栏；Esc 取消无副作用；双屏/150% 下坐标正确。

- [ ] **Step 6: 提交**

```powershell
git add -A; git commit -m "feat: own capture hotkey with freeze-frame region selection"
```

---

## Task 17: P1 验收与里程碑打标

**Files:**
- Create: `docs/acceptance/P1-core.md`
- Modify: `README.md`（功能与使用方法初稿）
- Test: 手动矩阵 M1/M5/M6/M8/M9/M10/M11/M16

**Interfaces:**
- Consumes: Task 1–16 全部。
- Produces: 可日用版本 + `v0.1.0-core` tag + 验收记录。

- [ ] **Step 1: 跑质量门**

```powershell
pnpm check
cd src-tauri; cargo clippy -- -D warnings; cargo test
```
Expected：全部通过。

- [ ] **Step 2: 执行验收矩阵**

按 spec §7.2 的 M1/M5/M6/M8/M9/M10/M11/M16 逐项执行，记录 pass/fail 与截图。

- [ ] **Step 3: 写验收记录**

`docs/acceptance/P1-core.md`：环境、构建产物、矩阵结果、已知问题（如拖拽到回收站的文件消失判定、单击复制在微信中的粘贴格式）。

- [ ] **Step 4: 打 tag 并提交**

```powershell
git add -A; git commit -m "chore: P1 core milestone acceptance"
git tag v0.1.0-core
```

- [ ] **Step 5: 汇报下一阶段**

向用户汇报 P1 结果，并提示下一份计划为 **P2 视觉灵魂**（玻璃质感、飞入/坠落、真实弹簧），以及 P3 标注/录屏、P4 设置完善、P5 发布。

---

## Self-Review Checklist（作者自检，已执行）

- **Spec 覆盖**：P0 E1–E6 对应 Task 2–6；三/四路截图来源对应 Task 8/9/5/16；面板几何 Task 10；展开收起 Task 11；复制/打开/删除 Task 12；拖放语义 Task 13；容量/多屏 Task 14/5；托盘设置 Task 15；持久化 Task 7/8；质量门 Task 1/17。**注**：P2–P5（视觉、标注、录屏、发布）按计划拆分到后续计划，不在本文件内——已在「里程碑定义」声明。
- **占位符扫描**：本文件中的 `todo!()` 仅出现在「Step 3 实现前」的示例中，并明确要求提交前替换为完整实现；不存在 TBD/待补/待定。
- **类型一致性**：`PhysicalRect{ x,y,w,h }`、`Item{ id,path,kind,hash,width,height,created_at,added_at }`、`Settings` 字段、command/event 名称在全文保持一致；TS 侧与 Rust serde 命名通过 `#[serde(rename_all = "camelCase")]` 对齐（Task 7 实现时必须加该属性）。

---

## Execution Handoff

两种执行方式，二选一：

1. **Subagent-Driven（推荐）**：每个 Task 派一个全新 subagent 实现，Lead 在任务之间 review；适合本计划 17 个任务的长链路。
2. **Inline Execution**：在当前会话按 `executing-plans` 批量执行，带检查点。

选定后进入对应 skill；实施时每个任务结束必须 `pnpm check && cargo test` 全绿并 commit。
