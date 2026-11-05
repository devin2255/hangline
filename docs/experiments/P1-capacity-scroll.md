# P1 容量、横向滚动与尺寸档实验报告（Task 14）

- **任务**：Task 14「容量、横向滚动与尺寸档（含 Task 10 遗留的多屏 follow / cardSize 窗口同步 / 滚轮修正）」
- **分支 / 基线**：`feat/core`，派发时 `f2724d6`；执行中 controller 追加 `a2a8d15`（`visibleSlice` plan 期望修正）作为实现提交的父提交
- **日期**：2026-10-30
- **结论**：`keepOnLine` 的 25/50/100 保留量通过纯函数 `visibleSlice` 生效；滚轮/触控板偏移为单一 offset 模型（非 passive 原生监听 + `preventDefault`，取主轴、每次 1 张，`scrollLeft` 恒为 0），32 张条目下双向滚动、越界 clamp、偏移变化后命中矩形重报均实测通过；`cardSize` 通过 `set_setting` 在 Rust 侧即时重算窗口（100% 下 small/medium/large 窗口高 189/210/246、容量 11/9/7、卡宽 120/150/202）；`get_monitors` 已成为纯查询，新增 `sync_panel_geometry`；面板可见期间指针在 150% 副屏停留 500 ms 后面板迁移到工作区 `(-1920,0) 1920×315`（逻辑视口 1280×210、dpr 1.5、容量 6），回到主屏后恢复 `(0,0) 1920×210`；隐藏期间不动，重新显示时立即按指针所在屏重同步。真实物理滚轮/真实鼠标跨屏仍列 MANUAL-PENDING（本轮用 CDP `WheelEvent` 与 `SetCursorPos` 程序化验证）。

## 1. 目的与范围

验证 Task 14 的实现：

1. `src/lib/layout.ts` 的 `maxItems`/`maxScrollOffset`/`visibleSlice`/`wheelStep`/`nextScrollOffset` 纯函数语义（最新在右、旧卡从左侧滑入、offset clamp）；
2. 面板只渲染可见窗口（`items.slice(start, end)`），偏移变化后重新上报物理命中矩形；不再有原生横向滚动与 handler 叠加；
3. `keepOnLine` 改变时重算保留窗口；新条目到达时偏移复位到最新端，旧条目被移除时只 clamp；
4. `cardSize` 改变时 Rust `set_setting` 重算并应用窗口高度，webview 收到 `resize` 后按新视口重算容量/卡片尺寸；
5. `get_monitors` 为纯查询；新增 `sync_panel_geometry` 命令供启动 resync/日志；`sync_panel_geometry` 同时被 `set_setting` 和 reveal 控制器复用；
6. 面板可见期间指针跨屏（500 ms 去抖）后迁移到指针所在显示器（工作区宽/高/DPI），隐藏期间不移动；重新显示时首帧立即同步；
7. 前端多屏日志打印所有 monitor + Rust 选中的 target。

不在本任务：托盘/设置 UI（Task 15）、reveal/tuck 语义（Task 11）、真实物理输入注入。

## 2. 环境与运行方式

| 项 | 值 |
| --- | --- |
| OS | Microsoft Windows 11（10.0.26300），Per-Monitor-V2 |
| Rust | rustc 1.90.0 / cargo 1.90.0 |
| App | Hangline 0.1.0（debug，`cargo build`） |
| 前端 | Vite 7.3.7 dev server（`http://localhost:5173`） |
| 显示器 | `\\.\DISPLAY1` 1920×1080 @100%，工作区 1920×1032，主屏；`\\.\DISPLAY2` 1920×1080 @150%（x=-1920），工作区 1920×1008 |
| 条目 | 32 张自生成 PNG（`hangline-t14-01…32.png`，尺寸 640×360/480×640/500×500/800×400/360×640/640×480 循环、不同底色与编号） |
| 观测 | `%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log` + WebView2 CDP（`--remote-debugging-port=9222`） |

**运行方式（沙箱 workaround）**：

1. `pnpm dev` 启动 Vite；
2. `cargo build --manifest-path src-tauri/Cargo.toml`，把 `src-tauri/target/debug/hangline.exe` 复制到 `%TEMP%\hangline-task14\hangline.exe`；
3. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*` 启动副本；
4. 用 CDP `Runtime.evaluate` 调用 Tauri command / 派发 `WheelEvent` / 读取 DOM；用 `SetCursorPos` 把指针移入/移出副屏（`%TEMP%\hangline-task14\hold-cursor.ps1` 在测试窗口内每 80 ms 重设一次，抵消测试机上的外部指针漂移）；
5. 面板可见性：启动后空线 700 ms 自动收起；上栏后通过**第二次启动 exe**（single-instance 插件 → `reveal(toggle, pinned)`）固定面板，并用 `set_panel_busy(true)` 防止外部点击把面板收起；
6. 32 张 PNG 在 app 启动后复制进 `%LOCALAPPDATA%\Hangline\Screenshots`，由目录 watcher 上栏（先保证 `state.json` 不存在，测试结束后删除）。

> 证据只包含 DOM/日志/窗口矩形；本报告未提交截图。窗口矩形用 `GetWindowRect`（先 `SetThreadDpiAwarenessContext(-4)`，PMv2）读取。

## 3. 预期 vs 观察

| # | 项 | 预期 | 观察 | 结论 |
| --- | --- | --- | --- | --- |
| 1 | 32 条 + `keepOnLine:"fit"` | 只渲染容量 9 张，最新在右 | `data-keep=fit`、`capacity=9`、`offset=0`、`maxOffset=0`、`range=[23,32)`、9 张卡 | PASS |
| 2 | `keepOnLine=100` | 保留 32 张可滚，最新 9 张可见 | `data-keep=100`、`maxOffset=23`、`range=[23,32)`、9 张卡 | PASS |
| 3 | 滚轮 +1（deltaY=+120） | offset 1，窗口 22..31，1 张/事件 | `offset=1`、`range=[22,31)`、`visible=9`、`expectedFiles=23..31.png`、`cancelled=true`（`preventDefault` 生效） | PASS |
| 4 | 滚轮 +10（deltaY=+120 ×10） | offset 11，窗口 12..21 | `offset=11`、`range=[12,21)`、`expectedFiles=13..21.png` | PASS |
| 5 | 横向 deltaX +120 ×5 | offset 16，窗口 7..16 | `offset=16`、`range=[7,16)`、`expectedFiles=08..16.png` | PASS |
| 6 | 反向 deltaY=-120 ×3 | offset 13，窗口 10..19 | `offset=13`、`range=[10,19)`、`expectedFiles=11..19.png` | PASS |
| 7 | 越界 clamp | max 23 / min 0，继续滚动不变 | +30 → `offset=23, range=[0,9)`（最旧保留 9 张）；再 +5 仍 23；-40 → `offset=0, range=[23,32)`；再 -5 仍 0 | PASS |
| 8 | 主轴判定 / 平局 | `|deltaY|>=|deltaX|` 取 deltaY；步长恒 1 | offset 6 时 `dx=+120,dy=-120` → 5（deltaY 负向胜）；`dx=-120,dy=+120` → 6 | PASS |
| 9 | 单一滚动模型 | 无原生 `scrollLeft` 叠加 | `scrollLeft=0`、`scrollWidth=clientWidth=1920`、`overflow:hidden`；每个 wheel 事件 `dispatchEvent` 返回 false（被 `preventDefault`） | PASS |
| 10 | 偏移变化后命中矩形重报 | 每次 offset 变化触发 `set_hit_rects`，矩形随可见卡片变化 | 单步滚轮后 Rust 日志报告数 14→16，首矩形随可见卡片旋转外接框变化（如 `9 rect(s), first at (189,41) 152x115` → `(187,40) 152x92`）；150% 下 DOM 卡片 CSS `(130.02,38.32,152.46,115.32)`×1.5 = `(195,57,229,173)`，与日志 `first at (195,57) 229x173` 一致 | PASS |
| 11 | 新条目到达时在滚动位置 | 复位到最新端 | offset 5（窗口 6..15）时上栏第 21 张 → `offset=0`、`maxOffset=12`、`range=[12,21)`、DOM 最右卡 `hangline-t14-21.png` | PASS |
| 12 | `cardSize` 即时改窗口（100%） | small 189 / large 246 / medium 210；容量与卡宽同步 | small：窗口 1920×189、`capacity=11`、`cardWidth=120`；large：1920×246、7、202；medium：1920×210、9、150；`set_setting` 日志 `panel: moved to … 1920x189/246/210 px`、`moved=true` | PASS |
| 13 | `keepOnLine` 设置路径 | 重走 geometry 但窗口几何不变 | 日志 `ipc: set_setting keepOnLine persisted; panel geometry re-applied (moved=false, target DISPLAY1)` | PASS |
| 14 | 指针跨屏 follow（可见） | 500 ms 去抖后迁移到指针屏工作区 | 指针在 DISPLAY2（150%）停留后：`panel: pointer dwelt on \\.\DISPLAY2 for 500 ms; following it` → `panel: moved to \\.\DISPLAY2 at (-1920,0), 1920x315 px (scale 1.50, work area 1920x1008)`；`GetWindowRect=(-1920,0) 1920×315`；CDP `innerWidth=1280, innerHeight=210, dpr=1.5, capacity=6`；回主屏后 `(0,0) 1920×210`、`innerWidth=1920`、容量 9 | PASS |
| 15 | 隐藏期间不移动 | 指针换屏但面板不动 | 面板在 DISPLAY1（`(0,0) 1920×210`）时隐藏，指针在 DISPLAY2 保持 ~2.6 s：`GetWindowRect` 仍为 `(0,0) 1920×210`，隐藏后日志无 `moved`/`dwelt` 行 | PASS |
| 16 | 重新显示时立即同步 | 首帧按指针屏重同步，不必等 500 ms | 隐藏期间指针在 DISPLAY2，显示后日志顺序为 `reveal: shown (reason SecondInstance…)` → `panel: moved to \\.\DISPLAY2 … 1920x315`，中间无 `dwelt` 行；`innerWidth=1280`、dpr 1.5、容量 6 | PASS |
| 17 | `get_monitors` 纯查询 | 只返回 monitor，不动窗口 | `ipc: get_monitors -> 2 monitor(s)` 后无 `moved`；`sync_panel_geometry` 返回 `monitors/target/size/position/moved` | PASS |
| 18 | 前端多屏日志 | 打印全部 monitor + Rust target | 前端 console 逐条打印 `monitor …DISPLAY2 … scale 1.5`、`monitor …DISPLAY1 … primary`，以及 `Rust target \\.\DISPLAY1 … window 1920x210 … moved=false` | PASS（CDP `invoke` 返回值核对） |
| 19 | 真实物理滚轮 / 真实鼠标跨屏 | 硬件输入路径 | 本轮用 CDP `WheelEvent`（cancelable，走同一非 passive 监听）与 `SetCursorPos`（真实系统光标位移，非合成鼠标消息）验证；**真实硬件滚轮/触控板与“人手移动鼠标跨屏”未做** | MANUAL-PENDING |

### 3.1 观测说明

- **日志截断**：debug 构建下 `tauri-plugin-log` 的 dev 日志文件在约 100 KB 时发生滚动/截断（长时间 30 Hz/500 ms 调试日志所致），因此本报告的证据片段都是在动作发生后立即抓取的；被截断的早期行不再引用。
- **跨 DPI 瞬时窗口矩形**：面板从 100% 移到 150% 屏后，若在 `set_position`/`set_size` 完成前用 `GetWindowRect` 采样，会读到过渡值（如 `2880×315` 或 `1280×210`）；稳定后读数为 `(-1920,0) 1920×315`，与 Rust 日志和 CDP `innerWidth=1280/dpr=1.5` 一致。这是测量时序伪影，不是实现行为。
- **测试机指针漂移**：远程/共享会话中指针会被外部移动，`SetCursorPos` 后不保证停留；跨屏用例用 `hold-cursor.ps1` 在测试窗口内每 80 ms 重设，确保 500 ms 驻留由同一屏累积。

## 4. 证据（节选）

### 4.1 启动：工作区几何 + 纯查询/同步命令

```text
[04:33:56][hangline_lib::panel][INFO] panel: 1920x210 px at (0, 0) on \\.\DISPLAY1 (scale 1.00, work area 1920x1032, cardSize Medium)
[04:33:56][hangline_lib::ipc][INFO] ipc: sync_panel_geometry -> 2 monitor(s), target Some("\\\\.\\DISPLAY1"), moved=false, size Some(PixelSize { width: 1920, height: 210 })
```

前端 `sync_panel_geometry` 返回值（CDP）：

```json
{
  "monitors": [
    {"id": "\\\\.\\DISPLAY2", "workArea": {"x": -1920, "y": 0, "w": 1920, "h": 1008}, "scale": 1.5, "primary": false},
    {"id": "\\\\.\\DISPLAY1", "workArea": {"x": 0, "y": 0, "w": 1920, "h": 1032}, "scale": 1.0, "primary": true}
  ],
  "target": "\\\\.\\DISPLAY1",
  "size": {"width": 1920, "height": 210},
  "position": {"x": 0, "y": 0},
  "moved": false
}
```

### 4.2 32 条上栏与保留量

```text
{"items":32,"revision":32,"newest":["hangline-t14-31.png","hangline-t14-32.png"]}
[04:34:22][hangline_lib::watchers::dirs][INFO] watchers: hung ...\hangline-t14-32.png (Capture) 480x640 hash 550c33d681916c51
[04:34:22][hangline_lib][DEBUG] panel hit rects updated: 9 rect(s), first at (189, 41) 151x114
{"keep":100,"revision":33}
[04:34:45][hangline_lib::ipc][INFO] ipc: set_setting keepOnLine persisted; panel geometry re-applied (moved=false, target Some("\\\\.\\DISPLAY1"))
{"inner":[1920,210],"dpr":1,"keep":"100","capacity":9,"offset":0,"maxOffset":23,"range":[23,32],"cards":9}
```

### 4.3 滚轮 / 双向滚动 / clamp / 主轴

每行是 CDP 派发后的 DOM 快照（`canceled=true` 表示事件被 `preventDefault`）：

```text
dx=0   dy=120  ×1   -> offset=1  range=[22,31) visible=9 files=23..31.png  cancelled=true
dx=0   dy=120  ×10  -> offset=11 range=[12,21) visible=9 files=13..21.png
dx=120 dy=0    ×5   -> offset=16 range=[7,16)  visible=9 files=08..16.png
dx=0   dy=-120 ×3   -> offset=13 range=[10,19) visible=9 files=11..19.png
dx=0   dy=120  ×30  -> offset=23 range=[0,9)   visible=9 files=01..09.png   (max clamp)
dx=0   dy=120  ×5   -> offset=23 range=[0,9)                                 (stays clamped)
dx=0   dy=-120 ×40  -> offset=0  range=[23,32) visible=9 classes=24..32.png  (min clamp)
dx=0   dy=-120 ×5   -> offset=0  range=[23,32)                               (stays clamped)
dx=120 dy=-120 ×1   -> offset=5  (from 6; |dy| tie wins, negative)
dx=-120 dy=120 ×1   -> offset=6  (from 5; |dy| tie wins, positive)
```

原生滚动模型（同一时刻）：

```json
{"scrollLeft":0,"scrollWidth":1920,"clientWidth":1920,"overflow":"hidden"}
```

### 4.4 命中矩形：偏移变化后重报 + 150% 物理换算

单步滚轮前后的 Rust 命中矩形报告数：

```text
hit-rect reports before=14 after=16
[04:34:58][hangline_lib][DEBUG] panel hit rects updated: 9 rect(s), first at (189, 41) 152x115
[04:34:58][hangline_lib][DEBUG] panel hit rects updated: 9 rect(s), first at (187, 40) 152x92
```

150% 副屏（`dpr=1.5`）上 DOM 与命中矩形一致：

```json
{"firstCss":{"x":130.02,"y":38.32,"w":152.46,"h":115.32},
 "firstPhysical":{"x":195,"y":57,"w":229,"h":173}}
```

```text
[04:40:21][hangline_lib][DEBUG] panel hit rects updated: 6 rect(s), first at (195, 57) 229x173
```

### 4.5 `cardSize` 即时窗口/卡片/容量（100% 主屏）

```text
{"cardSize":"small","revision":34}
- small  -> inner=[1920,189] capacity=11 cardWidth=120 offset=6 maxOffset=21 range=[15,26)
  panel: moved to \\.\DISPLAY1 at (0, 0), 1920x189 px (scale 1.00, work area 1920x1032, cardSize Small)
  set_setting cardSize persisted; panel geometry re-applied (moved=true, target Some("\\\\.\\DISPLAY1"))
{"cardSize":"large","revision":35}
- large  -> inner=[1920,246] capacity=7  cardWidth=202 offset=6 maxOffset=25 range=[19,26)
  panel: moved to \\.\DISPLAY1 at (0, 0), 1920x246 px (scale 1.00, work area 1920x1032, cardSize Large)
{"cardSize":"medium","revision":36}
- medium -> inner=[1920,210] capacity=9  cardWidth=150 offset=6 maxOffset=23 range=[17,26)
  panel: moved to \\.\DISPLAY1 at (0, 0), 1920x210 px (scale 1.00, work area 1920x1032, cardSize Medium)
```

窗口矩形（PMv2 `GetWindowRect`）：small `(0,0) 1920×189`、large `(0,0) 1920×246`、medium `(0,0) 1920×210`。

### 4.6 跨屏 follow / 隐藏不动 / 重新显示同步

```text
# 指针在 DISPLAY2 (150%) 驻留
[04:40:20][reveal][INFO] panel: pointer dwelt on \\.\DISPLAY2 for 500 ms; following it
[04:40:20][panel][INFO] panel: moved to \\.\DISPLAY2 at (-1920, 0), 1920x315 px (scale 1.50, work area 1920x1008, cardSize Medium)
# 窗口/视口
window -1920 0 1920 315 / inner [1280,210] dpr 1.5 capacity 6 cards 6
# 回主屏
[04:44:34][reveal][INFO] panel: pointer dwelt on \\.\DISPLAY1 for 500 ms; following it
[04:44:34][panel][INFO] panel: moved to \\.\DISPLAY1 at (0, 0), 1920x210 px (scale 1.00, work area 1920x1032, cardSize Medium)
window 0 0 1920 210 / inner [1920,210] dpr 1 capacity 9 cards 9
```

隐藏不动 + 重新显示立即同步（清空后重做的定向用例，使用已知面板 HWND `2692092` 读隐藏窗口矩形）：

```text
visible on DISPLAY1 before hide: window 0 0 1920 210
hidden; pointer held on DISPLAY2
rect while hidden: window 0 0 1920 210     ← 隐藏期间没有迁移
--- log while hidden ---
[04:43:12][single-instance] second launch detected, toggling the panel
[04:43:12][reveal] reveal: tuck (Toggle) …
[04:43:12][panel] panel: hidden
[04:43:12][reveal] reveal: hidden at +556168 ms (tuck animation finished)
--- show while pointer held on DISPLAY2 ---
[04:43:21][reveal] reveal: shown (reason SecondInstance, peek=false, items=32)
[04:43:21][panel] panel: moved to \\.\DISPLAY2 at (-1920, 0), 1920x315 px (scale 1.50, work area 1920x1008, cardSize Medium)
settled rect: window -1920 0 1920 315 / dom inner [1280,210] dpr 1.5 capacity 6
```

`reveal: shown` 之后紧跟 `moved to DISPLAY2`、中间没有 `pointer dwelt` 行 ⇒ 这是“重新显示首帧 resync”，不是 500 ms follow。

### 4.7 滚动中新增条目 → 复位到最新

```text
after 5 wheel steps: offset=5 maxOffset=11 range=[6,15)
new item hangline-t14-21.png -> items=21
after arrival: offset=0 maxOffset=12 range=[12,21)
visible labels = [hangline-t14-13 … hangline-t14-21]，newestVisible=hangline-t14-21
```

## 5. 关键命令

```powershell
pnpm dev
cargo build --manifest-path src-tauri/Cargo.toml
Copy-Item src-tauri\target\debug\hangline.exe "$env:TEMP\hangline-task14\hangline.exe"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222 --remote-allow-origins=*'
Start-Process "$env:TEMP\hangline-task14\hangline.exe"

# CDP：DOM/几何（Node 22 内置 WebSocket）
node cdp.mjs "JSON.stringify({inner:[innerWidth,innerHeight],dpr:devicePixelRatio})"
# 派发 N 次 wheel（主轴由实现选取，这里显式给 dx/dy）
node wheel.mjs 0 120 5
# 设置项（Rust 侧即时应用窗口几何）
node cdp.mjs "window.__TAURI_INTERNALS__.invoke('set_setting',{key:'cardSize',value:'large'})"
# 把指针移到副屏并保持（hold-cursor.ps1 每 80 ms 重设）
.\hold-cursor.ps1 -X -1000 -Y 500 -Seconds 15
# 读窗口矩形（先 SetThreadDpiAwarenessContext(-4)）
.\win32probe.ps1 -ProcessId <pid>
```

## 6. 手动待验证（MANUAL-PENDING）

1. **真实硬件滚轮/触控板**：本轮用 CDP `WheelEvent`（cancelable、走同一非 passive 监听）验证 offset/命中矩形；真实鼠标滚轮、触控板双指横向手势的 deltaMode/动量事件未在硬件上验。
2. **真实人手跨屏**：本轮用 `SetCursorPos` + 保持循环制造真实系统光标位移；人手移动鼠标跨屏（以及跨屏瞬间的 DPI 切换观感）未人工确认。
3. **150% 屏真实桌面合成**：CDP 只读 webview 内容；150% 下透明窗口与真实壁纸的 DWM 合成观感需人工看一眼。
4. **真实设置 UI 联动**：本轮通过 CDP `set_setting` 触发；托盘/设置 UI 的交互属 Task 15。
5. **真实显示器热插拔**：`monitor_signature` 变化路径由单测覆盖，运行中拔插显示器未实际制造。

## 7. 清理与恢复

- 删除 32 张 `%LOCALAPPDATA%\Hangline\Screenshots\hangline-t14-*.png`；
- 删除本轮产生的 `%LOCALAPPDATA%\Hangline\state.json`（测试前不存在）；
- `settings.json` 从备份恢复，SHA256 `8E315C7BA3B111186B9BE3C573FBC907FBB5C14C2453E363BD533CE4FCA79089`（`keepOnLine:"fit"`、`cardSize:"medium"`）；
- `Hangline.log` 从测试前备份恢复，SHA256 `1130405650B385051C425F131CF01969CE9FF0F0BBAF01EF986A645C13BDB134`；
- 停止 app 与 Vite；删除 `%TEMP%\hangline-task14`（exe、seed 图、CDP 脚本、证据片段）；应用数据目录无残留测试条目。

## 8. 偏差与决策

1. **`visibleSlice` 的 plan 示例数值修正（重要）**：Task 14 的 plan Step 1 / brief 原写 `visibleSlice(30,25,10,0) -> {20,25}`、`offset 5 -> {15,20}`。该期望在语义上自相矛盾：capacity 10 时 `{20,25}` 只有 5 张、且把最新的 5 张（25..29）排除，违背“最新固定在右侧/总是显示最新”。数值像是把绝对坐标（`start = total - capacity = 20`）与保留窗口坐标（`end = keep = 25`）混用（丢掉了被淘汰前缀 `total - keep = 5`）。执行中 controller 已用 `a2a8d15` 把 plan 修正为 `{20,30}` / `{15,25}` / `{5,15}`（offset 999 clamp），与本实现一致；以下保留原始错误值作为背景。实现取一致的绝对坐标：
   - `maxItems = max(capacity, keep)`，保留窗口 = 最新 `min(total, maxItems)` 张；
   - `offset ∈ [0, max(0, retained - capacity)]`；
   - `end = total - offset`，`start = max(total - retained, end - capacity)`；
   - 因此 `visibleSlice(30,25,10,0) = {20,30}`、`offset 5 = {15,25}`、`offset 15 = {5,15}`。offset 5 的 `start=15` 与 brief 相同，end 相差的正是被淘汰的 5 张前缀。测试与代码注释都记录了这一修正。
2. **几何 owner 选择**：Rust 是唯一 owner。`set_setting`（`cardSize`/`keepOnLine`）直接调用 `panel::sync_panel_geometry`；前端只在启动时调用 `sync_panel_geometry` 命令做 resync/日志，不在 `cardSize` 变更后再调一次。避免“前端 + 后端各改一次窗口”的竞态。
3. **`keepOnLine` 也重应用几何**：它不改变窗口尺寸，但按 controller 要求复用同一路径（`moved=false`），为将来 keep 相关布局留出单一入口。
4. **`get_monitors` 纯查询**：移除了 Task 10 的“查询即移动面板”副作用；启动时的窗口创建仍由 `panel::create` 应用几何，显式 resync 走 `sync_panel_geometry`。
5. **重新显示首帧 resync**：隐藏期间不轮询、不移动；但每次面板变为可见时，首帧先调用一次 `sync_panel_geometry`（一次窗口查询，不是逐帧），让“隐藏时指针换了屏/显示器变化”在显示瞬间纠正。可见期间的跨屏仍严格 500 ms 去抖。
6. **无原生横向滚动**：`.panel__viewport` 改为 `overflow:hidden`，滚轮由非 passive 监听 `preventDefault` + 单一 offset 状态驱动；`lineWidth` 只在面板窄于最小 3 卡组时溢出，此时整条线右对齐（保证最新卡完整可见，最旧侧裁切），不再依赖原生滚动条。
7. **偏移复位策略**：新条目（最新 id 变化）到达 → 复位到 0，保证“总是显示最新”；`keepOnLine` 变化 → 复位到 0；其它条目变化（prune/take-down）→ 只 clamp，保留用户浏览位置；容量变化（cardSize/窗口宽度）→ clamp。面板渲染/resize 与状态 effect 之间有至多一帧的旧 offset，下一帧即修正。
8. **`app_state.rs` 未改**：`settings.rs` 的 `keepOnLine`/`cardSize` 字段与 `AppState::set_json` 已满足需求；几何路径在 `settings::affects_panel_geometry` + `ipc::set_setting` + `panel::sync_panel_geometry`，不需要改数据层。
9. **日志滚动**：debug 构建下 dev 日志在约 100 KB 发生滚动/截断（见 §3.1），不是本轮代码改动引入；证据片段均即时抓取。
10. **`set_hit_rects` 日志**：为运行时取证在 debug 日志中追加了首个矩形（`first at (x,y) w×h`），不改变 IPC 行为。

## 9. 复审修复证据（2026-10-30，fix 提交）

本节是 Task 14 复审三项 Important 与折叠 minor 的修复记录。环境同 §2：`\\.\DISPLAY1` 1920×1080@100%（primary）、`\\.\DISPLAY2` 1920×1080@150%（x=-1920），debug 构建 + Vite dev server + CDP；面板用 second-instance 启动固定（pinned），上栏 2 张测试图（`hangline-t14fix-04/05.png`）。窗口矩形是 PMv2 `GetWindowRect` 物理像素；DOM 值是 CSS 像素（log 时间为 UTC）。

**修复摘要**

| 项 | 修复 |
| --- | --- |
| Important 1 | `panel::GeometryTarget::{CurrentPanel, Cursor}`：`set_setting` 与 `sync_panel_geometry` 命令用 `CurrentPanel`（窗口中心所在屏，窗口不存在回退 primary），只有 reveal follow 用 `Cursor`。follow 每次 poll 从窗口实际 `outer_position`+`inner_size` 中心重新派生锚点（`MonitorFollow::anchor_to_window`），外部移动后下一 poll 采纳新显示器；`sync_panel_geometry` 窗口不存在时 `size/position=None`（不再返回未应用的 desired）。 |
| Important 2 | `reveal.rs` 在 follow block 之前采样 `busy = is_busy() \|\| dragout::is_dragging()`；busy/drag 期间不枚举显示器、不 follow、display-change 也延后，pending 候选被丢弃，手势结束后需重新满 500 ms dwell。纯 helper `panel::follow_target(follow, observed, now, busy)` + 单测。 |
| Important 3 | `collectHitRects` 用 `resolveHitRectScale(cached, window.devicePixelRatio)` 与缓存比较，变化时刷新并安排一次补充上报；新增可见期 500 ms DPR watch（`document.visibilityState==="hidden"` 时跳过，卸载清理）；`sync_panel_geometry` 返回后从当前 DPR 重新播种并上报；resize 也改为直接读取 DPR。纯 helper + 4 个 vitest。 |
| folds | `FOLLOW_POLL_MS` 500→200（dwell 仍 500）；`lineShift(panelWidth, line)` 抽纯函数（normal 0 / 500,654 → -154）并用于 Panel；删除 `ipc.rs` 与 `settings.rs` 重复的 geometry 测试并更正测试计数；修正 `layout.ts`/`layout.test.ts`/`geometry.rs`/`panel.rs` 的过时注释（“可滚动溢出”“Task 14 消费 LogicalRect/to_logical/work_area_of”“唯一 mover”）；滚轮方向映射保持不变。 |

### 9.1 设置变更不再把线传送到指针屏（Important 1）

场景：面板在 DISPLAY1，指针移到 DISPLAY2，随后 `set_setting cardSize medium→large`。为隔离“设置路径本身”与 500 ms follow，先用 CDP `set_panel_busy(true)` 冻结 follow（busy 门控见 §9.2）；释放 busy 后再验证指针跨屏 follow 仍然工作。

| 观测点 | 窗口矩形（PMv2） | DOM | 结论 |
| --- | --- | --- | --- |
| 初始（指针 DISPLAY1） | `(0,0) 1920×210` | `inner 1920×210, dpr 1, cardSize medium` | 基线 |
| 指针 DISPLAY2 + `set_setting cardSize=large`（busy 冻结 follow） | `(0,0) 1920×246` | `inner 1920×246, dpr 1, cardSize large` | **re-fit DISPLAY1，未传送** |
| 设置后 1.3 s（指针仍在 DISPLAY2，follow 仍冻结） | `(0,0) 1920×246` | 同上 | 仍 DISPLAY1 |
| 释放 busy 1.7 s 后（指针 DISPLAY2） | `(-1920,0) 1920×369` | `inner 1280×246, dpr 1.5, large` | 去抖后正常 follow |
| 指针回 DISPLAY1 | `(0,0) 1920×210` | `inner 1920×210, dpr 1, medium` | 恢复正常 |

原始日志（时间 UTC）：

```text
[05:46:20][reveal][INFO] panel: monitor follow paused while a press/menu/drag gesture is in progress
[05:46:21][panel][INFO] panel: moved to \\.\DISPLAY1 at (0, 0), 1920x246 px (scale 1.00, work area 1920x1032, cardSize Large)
[05:46:21][ipc][INFO] ipc: set_setting cardSize persisted; panel geometry re-applied (moved=true, target Some("\\\\.\\DISPLAY1"))
[05:46:25][reveal][INFO] panel: pointer dwelt on \\.\DISPLAY2 for 500 ms; following it
[05:46:25][panel][INFO] panel: moved to \\.\DISPLAY2 at (-1920, 0), 1920x369 px (scale 1.50, work area 1920x1008, cardSize Large)
```

显式 sync 命令同样是 `CurrentPanel`：面板在 DISPLAY1、指针在 DISPLAY2、follow 被 busy 冻结时调用 `sync_panel_geometry` 返回
`{"target":"\\\\.\\DISPLAY1","moved":false,"size":{"width":1920,"height":210},"position":{"x":0,"y":0}}`，日志
`ipc: sync_panel_geometry (current panel) -> 2 monitor(s), target Some("\\\\.\\DISPLAY1"), moved=false`，窗口保持 `(0,0) 1920×210`。

### 9.2 follow 锚点来自窗口实际位置（外部移动后指针返回有效）

修复前 `MonitorFollow.applied` 只由 follow 自己的 sync 更新；窗口被外部移动到 DISPLAY2 后仍记为 DISPLAY1，指针回到 DISPLAY1 会被 `observe` 短路。修复后每个 follow poll 用窗口 `outer_position + inner_size` 中心派生锚点。

场景：面板/指针都在 DISPLAY1，用 PMv2 `SetWindowPos`（NOSIZE|NOZORDER）把面板外部移动到 `(-1920,0)`，指针保持 DISPLAY1（每 60 ms 重设，抵消测试机漂移）：

```text
settled:                              rect (0,0) 1920x210 / inner 1920x210 dpr 1
external SetWindowPos -> DISPLAY2:    rect (-1920,0) 2880x315（跨 DPI 瞬间 Windows 自动缩放）
pointer held on DISPLAY1 for 1.7 s:   rect (0,0) 1920x210 / inner 1920x210 dpr 1
```

```text
[05:52:03][reveal][INFO] panel: pointer dwelt on \\.\DISPLAY1 for 500 ms; following it
[05:52:03][panel][INFO] panel: moved to \\.\DISPLAY1 at (0, 0), 1920x210 px (scale 1.00, work area 1920x1032, cardSize Medium)
```

外部移动后窗口中心的显示器（DISPLAY2）被采纳为锚点，指针所在 DISPLAY1 成为真正的“另一屏”，500 ms dwell 后窗口被 follow 回 DISPLAY1。无 hide/show。

### 9.3 busy/drag 期间不移动窗口（Important 2）

场景：面板/指针在 DISPLAY1；CDP `set_panel_busy(true)` 模拟 press/menu/drag 手势；指针移到 DISPLAY2 保持 1.8 s：

```text
busy=true, pointer DISPLAY2 1.8 s:  rect (0,0) 1920x210 / inner 1920x210 dpr 1   ← 未移动
log: [05:53:18][reveal][INFO] panel: monitor follow paused while a press/menu/drag gesture is in progress
     （该窗口内没有 pointer dwelt / moved 行）
busy=false + pointer DISPLAY2 1.7 s: rect (-1920,0) 1920x315 / inner 1280x210 dpr 1.5
log: [05:53:24][reveal][INFO] panel: pointer dwelt on \\.\DISPLAY2 for 500 ms; following it
     [05:53:24][panel][INFO] panel: moved to \\.\DISPLAY2 at (-1920, 0), 1920x315 px (scale 1.50, ...)
```

busy 期间 poll 节奏保持但完全不枚举显示器；`follow_target(..., busy=true)` 丢弃 pending，因此手势结束后必须重新满足 500 ms dwell 才移动。单测 `follow_target_never_moves_while_busy_and_dwells_again_afterward` 覆盖同一决策。

### 9.4 命中矩形 DPI 缓存（Important 3）

真实 150% 屏上的 DOM × DPR 与 Rust 物理矩形一致（面板在 DISPLAY2，`dpr=1.5`）：

```json
{"firstCss":{"x":477.68,"y":50.78,"width":150.32,"height":109.44},"dpr":1.5}
firstCss × 1.5 = (716.5, 76.2) 225.5 × 164.2
[05:47:51][hangline_lib][DEBUG] panel hit rects updated: 2 rect(s), first at (717, 76) 225x164
```

100% 屏对照：CSS `(797.68, 57.78) 150.32×109.44` → Rust `first at (798, 58) 150x109`。

可见期 500 ms DPR watch 在真实 DISPLAY1↔DISPLAY2 往返中触发（console 时间本地 13:52，已按 CDP timestamp 过滤掉 WebView2 `Runtime.enable` 回放的历史条目）：

```text
13:52:37.452 Hangline panel: devicePixelRatio changed to 1.5; re-reporting hit rects
13:52:39.951 Hangline panel: devicePixelRatio changed to 1; re-reporting hit rects
```

**MANUAL-PENDING：等逻辑尺寸的跨 DPI 移动（1920 CSS px @100% → 1920 CSS px @200%，即 4K@200% 显示器）**。本机只有 1920×1080@100%/@150%，无法真实复现；尝试用 CDP `Emulation.setDeviceMetricsOverride({deviceScaleFactor:2})` 模拟时 WebView2 不接受该覆盖（直接读取 `devicePixelRatio` 仍为 1、`innerWidth` 不变，仅有瞬时事件），因此不把该合成结果当作等逻辑尺寸证据。该路径由纯函数 `resolveHitRectScale` 的单测覆盖（`cached 1 → dpr 2 → refreshed=true`），真实硬件需在 4K@200% 环境补验。

### 9.5 follow 轮询与延迟

`FOLLOW_POLL_MS` 由 500 降为 200，dwell 仍 500 ms。程序化测量“`SetCursorPos` 到日志出现 `moved to DISPLAY2`”的端到端延迟（PS 循环内每 ~15 ms 重设指针并轮询日志；含日志写入/检测开销）三次为 **833 / 824 / 918 ms**，与 200 ms 轮询下 500–700 ms 的理论首次采样窗口一致（原 500 ms 轮询为 0.5–1.0 s+）。日志中的 dwell 文案仍为 `500 ms`，与实际 debounce 常量一致。

### 9.6 folded minors

- `lineShift(panelWidth, line)` 纯函数 + 测试：`1920,1542 → 0`、`1024,1024 → 0`、`500,654 → -154`、`NaN → 0`；`Panel.tsx` 使用它，不再内联。
- `layout.ts`/`layout.test.ts` 中“溢出可滚动”的注释改为“右对齐、裁切最旧侧、无原生滚动”；`geometry.rs` 的 `LogicalRect`/`to_logical`/`work_area_of` 注释改为 E5 真实会话测试使用（非 Task 14 面板布局）；`panel.rs` 的“唯一 mover”改为“`create` 之后唯一 mover”。
- 删除 `ipc.rs` 中与 `settings.rs` 重复的 `set_setting_reapplies_...` 单测；`settings.rs::geometry_affecting_settings_are_card_size_and_keep_on_line` 保留唯一覆盖。修正后的测试计数见任务报告 §8。
- `sync_panel_geometry` 在窗口不存在时返回 `size/position=None`（代码路径；运行时窗口始终存在，未另行注入）。
- 滚轮方向映射保持原样；**MANUAL-PENDING：真实硬件滚轮/触控板方向**（spec §3.1 的方向措辞存在歧义，仍以“向下/向右 = 看更旧”实现）。

### 9.7 本轮清理

- 删除 `%LOCALAPPDATA%\Hangline\Screenshots\hangline-t14fix-01…05.png` 与 `state.json`（测试前不存在）；
- 从备份恢复 `settings.json`（SHA256 `8E31…9089`）与 `Hangline.log`（SHA256 `1130…B134`）；
- 停止 app 与 Vite；删除 `%TEMP%\hangline-t14fix`（exe、脚本、seed、备份）。
