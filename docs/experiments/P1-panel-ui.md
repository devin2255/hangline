# P1 面板静态 UI 实验报告：绳、夹子、玻璃卡片、缩略图与布局模型

- **任务**：Task 10「面板静态 UI（绳、夹子、卡片、缩略图）」
- **分支 / 基线**：`feat/core`，父提交 `3d827e5`
- **日期**：2026-10-29
- **结论**：面板由固定 1440×210 dp 改为「目标显示器工作区宽度 × (`photoMaxHeight(cardSize)+106`) dp」，位置贴工作区顶部；真实条目按容量上限渲染为玻璃卡片（缩略图、确定性倾斜、夹子挂在悬垂的 SVG 绳上）；命中矩形在布局/缩放/可见裁剪变化后按物理像素上报，且始终先于 `report_panel_ready`。100% 主屏实测窗口 1920×210 px、容量 9；150% 副屏实测窗口 1920×315 px、逻辑视口 1280×210、容量 6；小/中/大三档分别为 189/210/246 px 高与 120/150/202 dp 卡宽。程序化判据全部通过；真实的桌面壁纸合成、鼠标 hover 与连续多屏跟随列入手动待验证（§6）。

## 1. 目的与范围

验证 Task 10 的实现（设计文档 §3.3 / §4.4）：

1. Rust 用 `geometry::monitors()` 计算目标显示器工作区，面板宽 = `work_area.width`、高 = `photoMaxHeight + 106`、位置 = 工作区左上；
2. 新增 `get_monitors` command 返回 `MonitorInfo[]`（camelCase），并在调用时把面板几何同步到光标所在显示器；
3. 前端 `layout.ts` 的纯函数与规格数字一致（120/150/202、83/104/140、189/210/246、容量上下限）；
4. `PublicState.items` 的最新 `capacity` 条渲染为玻璃卡片；缩略图走 asset protocol，录屏 0×0 显示播放角标占位，HEIC/缺失文件显示带文件名的中性占位；
5. 绳为二次贝塞尔三层描边 + 渐隐遮罩；夹子为 9×26 dp 矢量；
6. 命中矩形按可见卡片裁剪后换算成物理像素并上报，布局、窗口缩放、字体/图片加载、滚动、条目变化后重报。

不在本任务：点击/双击/叉（Task 12）、原生拖出（Task 13）、展开/收起动画（Task 11）、横向滚动与保留数量语义（Task 14）。

## 2. 环境与运行方式

| 项 | 值 |
| --- | --- |
| OS | Microsoft Windows 11 家庭版中文版，10.0.26300 |
| Rust | rustc 1.90.0 / cargo 1.90.0 |
| App | Hangline 0.1.0（debug，`cargo build`） |
| 前端 | Vite 7.3.7 dev server（`http://localhost:5173`） |
| 显示器 | `\\.\DISPLAY1` 1920×1080 @100%，工作区 1920×1032，主屏；`\\.\DISPLAY2` 1920×1080 @150%（`x=-1920`），工作区 1920×1008 |
| 观测 | `%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log` + WebView2 CDP（`--remote-debugging-port=9222`） |

**运行方式（沿用 Task 2/5/7/8 的沙箱 workaround）**：

1. `pnpm dev` 启动 Vite；
2. `cargo build --manifest-path src-tauri/Cargo.toml`，把 `src-tauri/target/debug/hangline.exe` 复制到 `%TEMP%\hangline-task10\hangline.exe`；
3. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*` 启动副本；
4. 通过 CDP `Runtime.evaluate` 读 DOM、调用 `get_monitors` / `set_setting`；`Page.captureScreenshot` 抓取面板 webview（透明背景，保留 alpha），再合成到中性深色/浅色底上供文档阅读；
5. 条目来源：12 张自生成的 PNG（尺寸 640×360、480×640、500×500、800×400、360×640、640×480、1280×720、300×900、900×300、512×512、720×480、1024×768）复制进 `%LOCALAPPDATA%\Hangline\Screenshots`；另在 `%USERPROFILE%\Videos\Captures` 放 1 个 1 KB 伪 MP4（录屏 0×0），在 Screenshots 放 1 个伪 HEIC（`image` 无解码器，0×0）。

> 证据图只包含面板 webview 内容，不含桌面壁纸、远程会话 ID 或任务栏；透明像素在文档图中被压到 `#14171e`（深色）或 `#e8ecf1`（浅色），不改动实际窗口内容。原始 alpha PNG 保留在 `%TEMP%\hangline-task10`（不提交）。

## 3. 预期 vs 观察

| # | 项 | 预期 | 观察 | 结论 |
| --- | --- | --- | --- | --- |
| 1 | 100% 启动几何 | 宽 = 工作区 1920 px，高 = 210 dp，位置 (0,0) | Rust log `panel: 1920x210 px at (0, 0) on \\.\DISPLAY1 (scale 1.00, work area 1920x1032, cardSize Medium)`；PMv2 `GetWindowRect` = (0,0) 1920×210；DevTools `innerWidth=1920`, `dpr=1` | PASS |
| 2 | 3 个条目 | 3 张玻璃卡片，最新在右 | DOM 3 张，`data-capacity=9`，`data-card-count=3`；x = 711/885/1059（组宽 498 居中） | PASS |
| 3 | 容量 1920 @ medium | 规格公式 `max(3,min(12,floor((1920-200)/174)))` = 9 | 12 个条目时只渲染最新 9 张（04…12），`set_hit_rects` 计数 9；x = 189 + 174·i，最后一个右边缘 1731，左右边距各 189 | PASS（plan 的旧期望 12 已由 controller 更正为 9，见 §8.1） |
| 4 | 150% 显示器 | 窗口宽 = 工作区 1920 物理像素；高 210 dp × 1.5 = 315 px；逻辑宽 1280 | 光标移到 `x=-1000` 后调用 `get_monitors`：`panel: moved to \\.\DISPLAY2 at (-1920, 0), 1920x315 px (scale 1.50, work area 1920x1008)`；PMv2 `GetWindowRect` = (-1920,0) 1920×315；`innerWidth=1280`、`dpr=1.5`、容量 6、卡片 6 张 | PASS |
| 5 | 尺寸档 | small/medium/large = 面板 189/210/246、卡宽 120/150/202、照片上限 83/104/140 | `set_setting cardSize` + `get_monitors` 后：large 窗口 1920×246、容量 7、卡宽 202、照片 140；small 窗口 1920×189、容量 11、卡宽 120；恢复 medium 1920×210、容量 9、卡宽 150 | PASS |
| 6 | 绳 | `sag=min(30, w*0.018)`，`ropeTop=10`，三层描边 1.4/1.2/0.4 dp + 端部渐隐 | SVG 路径 1920 宽为 `M0,10 Q960,70 1920,10`（中点 y=40），1280 宽为 `M0,10 Q640,56.08 1280,10`（sag 23.04）；DOM 有 shadow/core/highlight 三层；CSS mask 为 4%–96% 渐隐 | PASS |
| 7 | 夹子 | 9×26 dp，金属渐变，每个卡片一个，位于绳与卡片之间 | 每张卡 1 个 9×26 SVG；卡顶随绳悬垂（100% 九卡时最外侧 42 dp、中央 58 dp；三卡时 57/58/57 dp），夹子 top=-18，挂在绳上 | PASS（视觉见证据图） |
| 8 | 玻璃卡片 | 分层渐变 + 0.75 dp 顶部高光 + 0.5 dp 外框 + 阴影；不依赖 backdrop-filter | 计算样式含 157° 渐变、`inset 0 0.75px` 高光、0.5 dp 边框、双层阴影；100% 下边框计算为 1 px（0.5 dp 在 1x 设备像素取整），150% 下为 0.6667 CSS px = 1 物理 px | PASS |
| 9 | 确定性倾斜 | 同一 id 每次渲染角度相同，范围 ±2.5° | DOM `data-tilt` 示例 1.98 / 0.29 / -2.08 / 2.2；刷新与状态更新后不重掷；单测锁定 `tiltFor("a")=1.95`、`tiltFor("b")=2.01` | PASS |
| 10 | 缩略图 | `convertFileSrc` + `loading=lazy`；真实尺寸；失败降级 | 12 张图全部 `complete=true`，`naturalWidth/Height` 与源文件一致（如 640×360、480×640）；asset URL 形如 `http://asset.localhost/C%3A%5C...png` | PASS |
| 11 | 录屏占位 | 0×0 → 16:9 占位 + 播放角标 | 伪 MP4 上栏为 `recording`、0×0；卡片无 `<img>`、有 `.glass-card__play`、文件名在底部 | PASS |
| 12 | HEIC/失败占位 | 中性占位 + 文件名，不崩 | 伪 HEIC 上栏 0×0；`<img>` 加载失败后降级为 `.glass-card__fallback`，显示 `hangline-t10-heic.heic`，面板继续工作 | PASS |
| 13 | 命中矩形 | 可见卡片物理像素；先上报再 ready；布局/滚动/缩放/条目变化重报 | 启动日志顺序：`hit rects 0` → `get_app_state` → `hit rects 9` → `frontend reported ready` → `panel: shown`；条目逐个上栏时计数 1→2→…→9，超过容量后保持 9；150% 时 6 | PASS |
| 14 | 裁剪卡片 | 被视口裁掉的卡片不得上报 | CDP 把 viewport 临时缩到 400 px 并触发 resize：DOM 仍有 6 张卡，`set_hit_rects` 从 6 降为 2；恢复 1280 px 后回到 6 | PASS |
| 15 | 主题 | `settings.theme` 覆盖、`system` 跟随 `prefers-color-scheme` | 默认 system→dark（`data-theme=dark`、`color-scheme: dark`）；`set_setting theme=light` 后 `data-theme=light`、`color-scheme: light`、绳变深、玻璃变浅；恢复 system 回 dark | PASS |
| 16 | PMv2 守卫 | `monitors()` 检测线程 DPI 上下文，非 PMv2 时警告 | 启动与 command 线程均记录 `geometry: enumerating monitors on a Per-Monitor-V2 thread`（如果非 PMv2 会输出 warning 并说明几何可能被虚拟化） | PASS |
| 17 | 空线 | 0 条目时无卡片、上报 0 个矩形 | 首次启动日志 `panel hit rects updated: 0 rect(s)`，面板只画绳 | PASS |

## 4. 证据

### 4.1 100% 主屏：工作区宽度与容量 9

重启应用（已有 14 条状态）后的原始日志：

```text
[09:46:13][hangline_lib::store] store: loaded 14 item(s) from %USERPROFILE%\AppData\Local\Hangline\state.json
[09:46:15][hangline_lib::geometry] geometry: enumerating monitors on a Per-Monitor-V2 thread
[09:46:15][hangline_lib::panel] panel: 1920x210 px at (0, 0) on \\.\DISPLAY1 (scale 1.00, work area 1920x1032, cardSize Medium)
[09:46:15][hangline_lib::ipc] ipc: get_monitors -> 2 monitor(s)
[09:46:15][hangline_lib] panel hit rects updated: 0 rect(s)
[09:46:15][hangline_lib::ipc] ipc: get_app_state -> 14 item(s), version 0.1.0
[09:46:15][hangline_lib] panel hit rects updated: 9 rect(s)
[09:46:15][hangline_lib] panel hit rects updated: 9 rect(s)
[09:46:15][hangline_lib] panel: frontend reported ready, showing the panel
[09:46:15][hangline_lib::panel] panel: shown
```

CDP 读取的 DOM（节选）：

```json
{
  "viewport": { "width": 1920, "height": 210, "dpr": 1 },
  "panel": { "theme": "dark", "cardSize": "medium", "panelWidth": "1920", "capacity": "9", "cardCount": "9" },
  "lineWidth": "1920px",
  "cards": [
    { "slotLeft": "189px", "slotTop": "42px", "kind": "capture" },
    { "slotLeft": "363px", "kind": "capture" },
    { "slotLeft": "537px", "kind": "capture" },
    { "slotLeft": "711px", "kind": "capture" },
    { "slotLeft": "885px", "kind": "capture" },
    { "slotLeft": "1059px", "kind": "capture" },
    { "slotLeft": "1233px", "kind": "capture" },
    { "slotLeft": "1407px", "kind": "recording" },
    { "slotLeft": "1581px", "kind": "capture" }
  ]
}
```

- 9 张卡片等距 174 dp（150 + 24），组宽 1542 dp，左右边距各 189 dp；卡片顶部随绳悬垂（最外侧 42 dp、中央 58 dp）；
- 本次重启共 14 条状态（12 张 PNG + 1 个录屏 + 1 个 HEIC），只渲染最新 9 张：PNG `06…12`、录屏、HEIC；更早的 `01…05` 未渲染。单独的 12 条目运行渲染的是 `04…12`（见 §3 行 3）；
- 12 个条目上栏时 `set_hit_rects` 在 9 后不再增加，证明容量上限生效。

### 4.2 3 个条目：玻璃卡片 / 夹子 / 绳

3 张图（640×360 横图、480×640 竖图、500×500 方图）上栏后：

```json
{
  "cardCount": 3,
  "capacity": 9,
  "cards": [
    { "slotLeft": "711px", "slotTop": "57px", "tilt": "1.98", "photoHeight": "80px", "rect": { "w": 152.95, "h": 93.13 } },
    { "slotLeft": "885px", "slotTop": "58px", "tilt": "0.29", "photoHeight": "104px", "rect": { "w": 150.56, "h": 112.76 } },
    { "slotLeft": "1059px", "slotTop": "57px", "tilt": "-2.08", "photoHeight": "104px", "rect": { "w": 153.97, "h": 117.37 } }
  ]
}
```

- 16:9 照片按 `142 / (16/9) ≈ 80 dp` 收缩；竖图与方图夹到照片上限 104 dp；
- 旋转后的 `getBoundingClientRect()` 比未旋转卡片略宽/高（如第三张 153.97×117.37），命中矩形使用该外接框；
- 卡片顶部随绳的悬垂变化（57/58/57 dp）。

证据图：`P1-panel-ui-100pct-3cards.png`。

### 4.3 150% 副屏：物理宽度、逻辑视口与容量

把光标放到副屏（`SetCursorPos(-1000, 500)`），通过 CDP 调用 `get_monitors` 触发 `sync_geometry`：

```text
[09:41:57][hangline_lib::geometry] geometry: enumerating monitors on a Per-Monitor-V2 thread
[09:41:57][hangline_lib::panel] panel: moved to \\.\DISPLAY2 at (-1920, 0), 1920x315 px (scale 1.50, work area 1920x1008, cardSize Medium)
[09:41:57][hangline_lib::ipc] ipc: get_monitors -> 2 monitor(s)
[09:41:57][hangline_lib] panel hit rects updated: 6 rect(s)
```

PMv2 线程下的 `GetWindowRect`（PowerShell 先 `SetThreadDpiAwarenessContext(-4)`，否则坐标会被虚拟化）：

```text
PMv2 window rect: (-1920,0) 1920x315
PMv2 client rect: 1920x315
```

CDP：

```json
{
  "viewport": { "width": 1280, "height": 210, "dpr": 1.5 },
  "panel": { "capacity": "6", "cardCount": "6" },
  "rope": "M0,10 Q640,56.08 1280,10",
  "cards": [
    { "slotLeft": "130px", "physical": { "x": 190, "y": 56, "w": 230, "h": 141 }, "border": "0.666667px rgba(255, 255, 255, 0.34)" },
    { "slotLeft": "304px" }, { "slotLeft": "478px" },
    { "slotLeft": "652px" }, { "slotLeft": "826px" }, { "slotLeft": "1000px" }
  ]
}
```

- 物理窗口仍覆盖整个 1920 px 工作区；逻辑宽度 1920/1.5 = 1280 dp，`Panel` 的 `usePanelWidth` 因此算容量 6；
- 命中矩形用 `getBoundingClientRect() × get_scale_factor()`，DOM 中第一张的 CSS 矩形 126.68→物理 190，宽 153.27→230，符合 1.5 倍换算；
- 0.5 dp 边框在 1.5x 下计算为 0.666667 CSS px（= 1 物理像素），符合 Chromium 的最小设备像素取整。

证据图：`P1-panel-ui-150pct.png`（含录屏/HEIC 占位）。

### 4.4 尺寸档

通过 CDP 调用 `set_setting cardSize=…` 后调用 `get_monitors`（`sync_geometry` 读取新设置并改窗口物理高度）：

| 档 | 窗口物理高 | 逻辑视口 | 容量 | 卡宽 | 照片上限/首张照片高 |
| --- | --- | --- | --- | --- | --- |
| small | 1920×189（日志） | 189 | 11 | 120 px | 83 / 56（首张为 2:1 横图，自然高 56） |
| medium | 1920×210 | 210 | 9 | 150 px | 104 / 104（首张为竖图） |
| large | 1920×246 | 246 | 7 | 202 px | 140 / 140（首张为竖图） |

对应日志：

```text
[09:45:53] panel: moved to \\.\DISPLAY1 at (0, 0), 1920x246 px (scale 1.00, work area 1920x1032, cardSize Large)
[09:45:54] panel: moved to \\.\DISPLAY1 at (0, 0), 1920x189 px (scale 1.00, work area 1920x1032, cardSize Small)
[09:45:56] panel: moved to \\.\DISPLAY1 at (0, 0), 1920x210 px (scale 1.00, work area 1920x1032, cardSize Medium)
```

> 任务边界：本任务只验证「设置驱动的尺寸计算」；托盘/设置 UI 在运行中改 `cardSize` 时自动重排窗口属于 Task 14/15，本轮由显式 `get_monitors` 触发。

### 4.5 命中矩形：顺序、可见裁剪与重报

**先上报再显示**（重启日志见 §4.1）：`hit rects 0` → `get_app_state` → `hit rects 9` → `reported ready` → `shown`。空线时也先上报空数组，避免 Rust 侧保留上一次的旧矩形。

**条目变化**：3 张时计数 3；逐个增加到 9 时计数 1→2→…→9；第 10–12 个条目上栏后仍为 9（未渲染的不上报）。

**可见裁剪探针**：把 `.panel__viewport` 临时设为 400 px 宽并派发 `resize`（DOM 仍挂载 6 张卡）：

```text
[09:45:02] panel hit rects updated: 2 rect(s)     ← 只有与 400 px 视口相交的 2 张
[09:45:03] panel hit rects updated: 6 rect(s)     ← 恢复 1280 px 后 6 张
```

这证明被裁掉的卡片不会阻塞点击；滚动（`scroll`）与图片 `load` 走同一 `scheduleReport` 合并路径，但因本机无窄于最小容量组的真实显示器，未在真实溢出场景下单独取证（见 §6）。

### 4.6 缩略图、录屏与失败降级

- 12 张 PNG 的 `<img>` 全部 `complete`，`naturalWidth/Height` 与文件一致；asset 协议 URL 由 `convertFileSrc` 生成；
- 伪 MP4（1 KB）上栏为 `recording`、0×0；DOM：`kind=recording`、无 `<img>`、`.glass-card__play` 存在、文件名显示在底部；
- 伪 HEIC（512 B）上栏 0×0，`<img>` 失败后出现 `.glass-card__fallback`，文件名 `hangline-t10-heic.heic` 正常显示，面板未崩溃；
- 两张失败卡片仍参与布局与命中矩形（它们仍是可见卡片）。

### 4.7 主题与 PMv2 守卫

- 系统深色时 `data-theme=dark`；`set_setting theme=light` 后 `data-theme=light`、`color-scheme: light`，绳 `rgba(54,64,74,.66)`、边框 `rgba(255,255,255,.9)`；恢复 `system` 后跟随系统回到 dark。证据图 `P1-panel-ui-150pct-light.png`。
- `monitors()` 每次调用先检查 `GetThreadDpiAwarenessContext() == DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`：启动线程与 `get_monitors` command 线程都输出 `enumerating monitors on a Per-Monitor-V2 thread`；非 PMv2 时输出 warning（代码路径，运行时未触发）。

### 4.8 证据图

| 文件 | 像素 | 字节 | SHA256（前 16） | 说明 |
| --- | --- | --- | --- | --- |
| `P1-panel-ui-100pct-3cards.png` | 1920×210 | 78,313 | `3B7841A8A952C758` | 100%、3 张卡片，绳/夹子/玻璃细节 |
| `P1-panel-ui-100pct.png` | 1920×210 | 213,278 | `08D5A82DB34281C1` | 100%、9 张（含录屏/HEIC 占位） |
| `P1-panel-ui-150pct.png` | 1920×315 | 259,149 | `360ECE641C81DA58` | 150%、6 张，物理 1920×315 |
| `P1-panel-ui-150pct-light.png` | 1920×315 | 269,183 | `9D46EAB9DA4BA1F5` | 150%、浅色主题 |
| `P1-panel-ui-card-detail.png` | 900×290 | 147,441 | `CB22DE6DF702B527` | 卡片/夹子/录屏占位放大裁切 |
| `P1-panel-ui-extra-folder.png` | 1920×210 | 39,735 | `38CBE7CD330518E3` | 评审修复：`settings.extraFolders` 目录中的图片缩略图（§9） |

图片均为 CDP 抓取的 webview 内容（透明背景保留 alpha），再合成到 `#14171e`（深）或 `#e8ecf1`（浅）中性底；桌面壁纸与远程会话信息不在图中。

## 5. 关键命令

```powershell
pnpm dev
cargo build --manifest-path src-tauri/Cargo.toml
Copy-Item src-tauri\target\debug\hangline.exe "$env:TEMP\hangline-task10\hangline.exe"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222 --remote-allow-origins=*'
Start-Process "$env:TEMP\hangline-task10\hangline.exe"

# CDP（Node 22 内置 WebSocket）：DOM/几何读取
node cdp.mjs "JSON.stringify({innerWidth:window.innerWidth, dpr:window.devicePixelRatio, ...})"
# 触发几何同步（光标在哪个显示器，面板就贴哪个工作区）
node cdp.mjs "window.__TAURI_INTERNALS__.invoke('get_monitors')"
# 透明截图
node cdp-shot.mjs "$env:TEMP\hangline-task10\panel.png" transparent
```

## 6. 手动待验证（agent 无法在真实桌面上完成）

1. **玻璃与桌面壁纸的真实合成**：CDP 截图只含 webview 内容；DWM 把透明窗口合成到实时壁纸上时的观感（尤其是浅色壁纸上的白边/深色壁纸上的可读性）需人工看一眼。
2. **hover 抬升**：`scale(1.03)` + 阴影增强只在 CSS 中实现，自动化未移动真实鼠标到卡片上验证。
3. **真实点击穿透边界**：命中矩形外扩 4 dp 的实际鼠标行为属于 Task 2 的 hittest；本轮只验证矩形内容与可见裁剪，未重复 E1/E2 的真机鼠标测试。
4. **连续多屏跟随 / 显示器热插拔**：本轮只验证 `get_monitors` 触发的同步；指针跨屏自动迁移与 `WM_DISPLAYCHANGE` 防抖属于 Task 14。
5. **真实滚动**：本机无窄于最小容量组的显示器，`wheel`/`scroll` 重报路径未在真实溢出场景下取证（Task 14 的 25/50/100 保留量会覆盖）。
6. **真实 HEIC**：本轮用 512 B 伪 HEIC 触发 `<img>` 失败路径；真实 HEIC 是否被 WebView2 解码取决于系统扩展。
7. **真实录屏首帧**：本轮为 0×0 占位；首帧解码/播放角标属于后续任务。
8. **系统主题切换**：用 `set_setting` 验证了 light/dark 覆盖；Windows 实时切换主题时的 `prefers-color-scheme` 事件未手工触发。

## 7. 清理与恢复

- 删除 `%LOCALAPPDATA%\Hangline\Screenshots\hangline-t10-*.png`、`hangline-t10-heic.heic`；
- 删除 `%USERPROFILE%\Videos\Captures\hangline-t10-rec.mp4`；
- 删除本轮产生的 `%LOCALAPPDATA%\Hangline\state.json`（测试前不存在）；
- `settings.json` 从备份恢复并校验哈希一致（`theme=system`、`cardSize=medium` 与现实一致）；
- `Hangline.log` 从测试前备份恢复（本轮日志另存于 `%TEMP%\hangline-task10\Hangline.task10.log`）；
- 停止应用与 Vite；删除 `%TEMP%\hangline-task10` 中的 exe/测试图（证据图已提交到 `docs/experiments/`）。

## 8. 偏差与决策

1. **`capacity(1920, medium)` 的旧 brief 期望已由 plan 更正**：Task 10 的原始 brief/Step 1 写 `expect(capacity(1920,"medium")).toBe(12)`（上限 12），但同一 brief 与 spec §3.3 的公式是 `max(3,min(12,floor((panelWidth-200)/spacing)))`，`spacing=150+24=174`，因此 `floor(1720/174)=9`；任何单一线性除数都无法同时满足 `1024→4` 与 `1920→12`。实现按规格公式，测试覆盖 `1024→4`、`500→3`、`1920→9`，并用 `2287→11` / `2288→12` / `2560→12` 锁定 12 的上限。执行期间 controller 已在 `3d827e5` 修正 plan 为 `1920→9`、`2288→12`，与本实现一致；若未来要让 1920 显示 12 张，需修改 spec 的间距/边距而不是改实现。
2. **绳控制点**：plan Step 3 的 `Q{width/2},{10+4*sag*0.25}` 会让中点只下垂 `sag/2`；spec §3.3 明确 `ropeY(t)=ropeTop+4*sag*t*(1-t)`，对应控制点 `ropeTop+2*sag`。实现取后者（1920 宽控制点 y=70，中点 y=40），`ropePath` 单测锁定。
3. **卡片居中基准**：`cardX(index, capacity, size, panelWidth)` 以「实际在场卡片数」为组宽（3 张时居中成 498 dp 组），而不是按容量槽位左对齐；最小容量 3 在窄屏溢出时 `left` 钳到 0，由横向滚动承接（`lineWidth`）。Task 14 可复用该函数并把 `capacity` 换成可见 slice 长度。
4. **P0 拖出 demo 从 Panel 移除**：Task 10 是静态 UI，`dragSession`/`dragThreshold` 模块与单测保留，`start_drag`/`drag_demo_file` command 保留；真实条目拖出在 Task 13 接线。
5. **`get_monitors` 带几何同步副作用**：command 文档明确说明；纯查询语义会让「显示器变化后前端 resync」多一个 command，而 Task 14 会替换为连续跟随。已用 150% 切换验证副作用正确且幂等（几何相同时不调用窗口 API）。
6. **0.5 dp 外框在 100% 下被 Chromium 取整为 1 物理像素**（150% 下 0.6667 CSS px = 1 物理 px），这是设备像素网格限制，不是样式错误。
7. **Vite 冷启动的首次空白**：第一次启动 app 时 Vite 刚 ready，webview 的 `#root` 为空、未完成握手；手动 `Page.reload` 后正常。Vite 预热后做了一次完整重启（kill + 重新启动 exe），日志显示无需 reload 即完成 `hit rects → ready → shown`，因此判定为 dev server 冷启动/依赖预打包的时序问题，不是生产路径缺陷；仍未在 CI 中复现。

## 9. 评审修复：额外监视目录的 asset-protocol scope（2026-10-29）

### 9.1 问题与修复

`tauri.conf.json` 的静态 `assetProtocol.scope` 只覆盖 `$LOCALDATA/Hangline/**`、`$PICTURE/Screenshots/**`、`$VIDEO/Captures/**`。面板用 `convertFileSrc(item.path)` 渲染每张卡片的缩略图，而 `watchers::start_all` 还会监视 `settings.extraFolders`；未被静态 scope 覆盖的额外目录会在 asset protocol 层被拒绝，卡片降级为带文件名的占位块，而不是照片。plan（约 line 918）要求对动态目录调用 `asset_protocol_scope().allow_directory`。

修复（`src-tauri/src/watchers/mod.rs`）：

- 新增 `pub fn allow_asset_dir(app: &AppHandle, path: &Path)`：对存在的目录调用 `app.asset_protocol_scope().allow_directory(path, true)`（递归）；失败只记 `warn`，不影响启动；用「路径 components 归一化 + 小写」的进程内集合去重，同一目录重复调用为 no-op（幂等）；失败不写入集合，后续调用会重试；不存在的目录记 `warn` 并跳过（不创建用户目录）。
- `start_one` 在 `dirs::start` **之前**调用 helper：系统截图目录、录屏目录、Hangline 自有截图目录和每个 `settings.extraFolders` 条目（存在的）都在 watcher 启动前放行，避免第一个条目与 scope 注册之间的竞态。
- 模块文档与 `allow_asset_dir` 文档写明 Task 15 契约：设置 UI 在启动新加入的额外目录 watcher 之前必须调用该 `pub` helper；scope 只增不减（移除目录不撤销授权，已上栏卡片的缩略图继续可读，重启后回到配置 scope），见 §9.5。

### 9.2 运行时 E2E 设置

环境与运行方式同 §2（`pnpm dev` + `cargo build`，exe 复制到 `%TEMP%\hangline-task10-fix`，`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*`）。

- 额外目录：`%TEMP%\hangline-task10-fix\extra`（绝对路径，作为唯一一条写入 `settings.json.extraFolders`，≤3）；
- **启动前**放入 `extra-folder-photo.png`（640×360）：用于证明启动时注册的 scope 对「目录里已存在的文件」也生效，不依赖 watcher 事件；
- 对照目录：`%TEMP%\hangline-task10-fix\unscoped`（不在任何 scope/监视列表），放 `unscoped-photo.png`（320×180），用于证明 scope 不是全局放开；
- app 启动时状态为空；启动后再写入 `extra-folder-new-arrival.png`（480×640）触发 watcher 上栏，验证真实卡片的 `<img>`；
- 观察手段：应用日志 + CDP `Runtime.evaluate`（DOM、`naturalWidth`、`Image` 探针）+ `Page.captureScreenshot`。

### 9.3 预期 vs 观察

| # | 项 | 预期 | 观察 | 结论 |
| --- | --- | --- | --- | --- |
| 1 | 启动时注册动态 scope | 四个被监视目录各一行 allow，含 extra | 日志：`watchers: asset protocol allows %USERPROFILE%\Pictures\Screenshots (recursive)`、`...\Videos\Captures`、`...\Hangline\Screenshots`、`...\Temp\hangline-task10-fix\extra`，随后 `watchers: watching ...\extra (Opts { kind: WatchedFolder, ... })` | PASS |
| 2 | 启动前已存在的 extra 图片可读 | `Image` 加载成功，natural 尺寸 640×360 | `Image` 探针 `ok=true`、`naturalWidth=640`、`naturalHeight=360`，src 为 `http://asset.localhost/C%3A%5C...%5Cextra%5Cextra-folder-photo.png` | PASS |
| 3 | 未授权目录仍被拒（负对照） | 加载失败，naturalWidth 0，日志有拒绝原因 | `Image` 探针 `ok=false`、`naturalWidth=0`；日志 `[tauri::protocol::asset][ERROR] asset protocol not configured to allow the path: ...\unscoped\unscoped-photo.png` | PASS |
| 4 | 新文件经 watcher 上栏 | `watchedFolder` 条目，480×640 | 日志 `watchers: hung ...\extra\extra-folder-new-arrival.png (WatchedFolder) 480x640 hash 65ca6dabdf5b7e84`；`get_app_state` 中 `kind=watchedFolder`、`width=480`、`height=640` | PASS |
| 5 | 卡片缩略图真实加载（非占位） | 卡片 `<img>` 的 `naturalWidth > 0`，无 fallback tile | DOM：`fallbackTiles=0`；`<img>` `complete=true`、`naturalWidth=480`、`naturalHeight=640`；卡片 `left=885px`、`top=58px`、`aria-label=extra-folder-new-arrival.png` | PASS |
| 6 | 截图证据 | 能看到带照片的卡片 | `P1-panel-ui-extra-folder.png`（1920×210，透明 webview 合成到 `#14171e`）显示居中卡片与 “NEW ARRIVAL IN EXTRA FOLDER” 照片 | PASS |
| 7 | 清理与恢复 | 设置/日志哈希回滚、状态删除、进程停止 | `settings.json` SHA256 恢复为 `8E315C7B...A79089`，`Hangline.log` 恢复为 `CE69D501...D2FA92`，测试前不存在的 `state.json` 已删除；app 与 Vite 已停止 | PASS |

CDP 关键返回（节选）：

```json
{
  "cards": [{ "kind": "watchedFolder", "left": "885px", "top": "58px", "name": "extra-folder-new-arrival.png" }],
  "images": [{
    "src": "http://asset.localhost/C%3A%5CUsers%5C<user>%5CAppData%5CLocal%5CTemp%5Changline-task10-fix%5Cextra%5Cextra-folder-new-arrival.png",
    "complete": true,
    "naturalWidth": 480,
    "naturalHeight": 640
  }],
  "fallbackTiles": 0
}
```

证据图：`P1-panel-ui-extra-folder.png`（1920×210，39,735 bytes，SHA256 `38CBE7CD330518E3`）；与 §4.8 的图片一样只含 webview 内容，透明像素压到 `#14171e`，不含桌面壁纸或远程会话信息。

### 9.4 Rust 测试补强（carry-over A：work area vs screen rect）

原有 `panel.rs` 测试的 `monitor()` 令 `rect == work_area`，无法区分两者。新增 `monitor_with(rect, work_area, scale)` 与 `panel_geometry_uses_work_area_not_the_full_screen_rect`：

- 底部任务栏：`rect` 1920×1080、`work_area` 1920×1032（同原点）→ 面板 1920×210 @ (0,0)；
- 左侧任务栏：`work_area` 从 x=80 开始、宽 1840 → **宽度 1840（不是 1920）**、位置 (80,0)；
- 顶部任务栏：`work_area.y=40` → 位置 (0,40)；
- 200% 缩放 + 左侧任务栏：`work_area` 3680×2160 → 尺寸 3680×420、位置 (-3840,0)。

实现上新增纯函数 `panel_position(monitor)`（返回 work-area 原点），`apply_geometry` 与 `geometry_matches` 共用它，避免初始放置和 `get_monitors` 重同步各写一份坐标逻辑。另新增 `asset_scope_keys_dedupe_case_and_trailing_separators`，锁定 `allow_asset_dir` 幂等键的大小写/尾分隔符归一化。

### 9.5 Task 15 契约与已知限制

**Task 15 必须遵守的契约：**

1. 新增/启动某个额外目录的 watcher 前，先调用 `hangline_lib::watchers::allow_asset_dir(&app, path)`（`pub`，模块文档有说明）；目录不存在时 helper 记 `warn` 并跳过。
2. 调用应发生在该目录 watcher 产生第一个事件之前（与 `start_one` 相同顺序），否则新卡片可能先以 403/占位形式出现，直到图片重载。
3. scope 只增不减：移除额外目录不会撤销已授予的读取权限；已上栏卡片的缩略图因此继续可读，进程重启后回到 `tauri.conf.json` 的静态 scope。若未来产品要求立即撤销，需要额外的 `forbid_directory` 设计（并决定已上栏条目如何降级），不在 Task 10 范围。
4. 动态 scope 不持久化：每次启动由 `start_all` 重新注册；Task 15 若在运行中改 `extraFolders`，必须自行调用 helper，不能只写 `settings.json`。

**已知次要项（未在本次修复中处理）：**

1. 真实设置 UI 的添加/移除流程未验证：本轮直接改 `settings.json` 并重启（watcher 只在 setup 启动一次，运行中增删目录属 Task 15）。
2. UNC / `\\?\` 前缀路径：幂等键只做 components 归一化 + 小写；Tauri 的 `push_pattern` 会额外处理 Windows verbatim 前缀，本轮未构造 UNC 额外目录取证。
3. 目录被删除/图片被删：scope 仍允许该路径，`<img>` 失败后走既有 fallback；未做运行中删除目录的 E2E。
4. 剩余人工项同 §6（桌面壁纸合成、hover、真实点击穿透、连续多屏跟随等），与本次 scope 修复无关。
