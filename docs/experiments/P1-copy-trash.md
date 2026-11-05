# P1 复制 / 打开 / 移入回收站实验报告

- **任务**：Task 12「复制 / 打开 / 移入回收站（含卡片手势与 Copied 胶囊）」
- **分支 / 基线**：`feat/core`，父提交 `2c8e211`；T12 评审修复基线 `3f8dd6c`
- **日期**：2026-10-30
- **结论**：程序化判据全部通过。单击卡片在系统双击间隔（500 ms）后写入 `PNG` + `CF_DIBV5` + `CF_HDROP`，`Copied` 胶囊显示 1.2 s；双击取消待复制、用默认应用打开且剪贴板序号不变；叉的热区为 26 CSS px，点击后卡片移除并广播新 revision；自有截图文件进入回收站（`Shell.Application` namespace 10 可见），watched-folder 文件仅下线、保留在磁盘；按压期间 `set_panel_busy(true)` 阻止收起，释放后 510 ms 正常收起。原始 `CF_DIBV5` 为 124 字节头、负高度（top-down）、32 bpp BGRA。T12 评审修复后复跑：写入以面板窗口 HWND 为 owner（`OpenClipboard(panel_hwnd)`），三种格式逐项 `IsClipboardFormatAvailable` 校验通过，`CloseClipboard` 不再报 `ERROR_CLIPBOARD_NOT_OPEN`，`clipboard-copied` 事件在页面监听器中收到；文件被外部删除后叉卡片按「文件已不存在」只下线并移除卡片。真实微信/QQ/浏览器粘贴与默认应用 GUI 展示仍需人工确认（§5）。

> **证据口径**：本文件区分【运行观察】与【单元测试/接线】；未跑过的项不写成已观察。主表（§3）的完整双条目数据来自 2026-10-30 09:38–09:39 的受控轮次（debug 二进制）。此后只改了 `CloseClipboard` 日志级别/文案与 `GlassCard` 回调稳定性（不改变手势行为），并用最终二进制在 09:44–09:45 重跑了单击/双击/叉主链路，结果与主表一致（§6.5）。T12 评审修复（owner/校验/事件/缺失文件）在 10:20–10:23 用新二进制复跑，证据见 §4.6；主表未重跑项以 §4.6 的修复轮次为准。

## 1. 目的与范围

验证 Task 12 的实现（设计文档 §3.1 / §4.7 / §4.6 E）：

1. `clipboard_io` 的格式决策表、PNG/DIBV5/HDROP 负载构造、`HGLOBAL` 所有权；
2. `copy_item` 的实际剪贴板格式与内容，复制不修改源文件；
3. 单击延迟复制与 1.2 s `Copied` 胶囊；双击直接打开且不污染剪贴板；
4. 26 dp 叉热区、无障碍标题、右侧状态区；
5. `take_down` 的 kind/路径策略：自有截图目录可回收，watched-folder/系统目录只下线；
6. `IFileOperation` 回收站行为与 Recycle Bin 可见性、状态 revision 广播；
7. 按压 busy 不收起、释放后恢复收起。

不在本任务：右键上下文菜单、长按标注、原生拖出（Task 13/15）、`show_in_explorer`（可选，未实现）、完整 M8 人工验收。

## 2. 环境与运行方式

| 项 | 值 |
| --- | --- |
| OS | Microsoft Windows 11 家庭版中文版，10.0.26300 |
| Rust | rustc 1.90.0 / cargo 1.90.0 |
| App | Hangline 0.1.0（debug，`cargo build`） |
| 前端 | Vite 7.3.7 dev server（`http://localhost:5173`） |
| 显示器 | `\\.\DISPLAY1` 1920×1080 @100%，工作区 1920×1032，主屏 |
| 观测 | `%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log`、WebView2 CDP :9222、PowerShell -STA 剪贴板读取、Win32 原始 `CF_DIBV5` 读取、`Shell.Application` namespace 10 |
| 条目 | 自有目录 80×60 PNG（红/绿/蓝/白四象限，hash `8DB5F9B4…162487`）；watched folder 40×30 teal PNG（hash `9E5EE8B9…6A4D`） |

**运行方式**（沿用 Task 2/5/7/8/10/11 的沙箱 workaround）：

1. 备份 `%LOCALAPPDATA%\Hangline\settings.json`（SHA256 `8E315C7B…A79089`），把 `extraFolders` 临时指向 `%TEMP%\hangline-task12\watched`；
2. `pnpm dev`；`cargo build --manifest-path src-tauri/Cargo.toml`；把 `src-tauri/target/debug/hangline.exe` 复制到 `%TEMP%\hangline-task12\hangline.exe`；
3. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*` 启动副本；
4. 目录 watcher 只处理启动后的新事件，因此启动后再把两张 PNG 复制进各自目录，得到 `Capture` 与 `WatchedFolder` 两条 item；
5. 用 CDP `Input.dispatchMouseEvent` 产生单击/双击/叉的真实浏览器输入事件；面板用第二次启动（single-instance toggle）pin 住，避免 peek 到期动画干扰；OS 指针移到面板外 (960,700)，避免共享桌面的真实鼠标误点卡片；
6. 剪贴板用 `powershell.exe -STA` 的 `System.Windows.Forms.Clipboard` 检查格式/内容，另用 P/Invoke 直接读取 format 17 的原始 `BITMAPV5HEADER`；回收站用 `(New-Object -ComObject Shell.Application).NameSpace(10)` 枚举；
7. 状态 revision 通过 CDP 调用 `window.__TAURI_INTERNALS__.invoke('get_app_state')` 读取。

> **共享桌面干扰**：第一次无控运行把 OS 指针放在卡片上，期间发生真实鼠标点击（日志出现两次复制、一次打开，Photos 进程被拉起）。该轮证据全部丢弃；重置 state、把指针移出面板后重新运行，§3 的时间与数值均来自受控轮次（2026-10-30 09:38–09:39）。面板仍可能被真实“点击面板外”收起，最终叉测试是重新用 second-instance 展开后在数秒内完成的。

## 3. 预期 vs 观察

| # | 项 | 预期 | 观察 | 结论 |
| --- | --- | --- | --- | --- |
| 1 | 单击复制（格式 + 胶囊） | 500 ms 后写 PNG + DIBV5 + HDROP；胶囊 1.2 s | CDP 单击后 650 ms：`data-copied=true`、胶囊文本「已复制」；1.2 s 后 `data-copied=false`、胶囊消失；剪贴板序号 936→942；格式 `PNG, Format17, FileDrop, Bitmap, DeviceIndependentBitmap`；`PNG` 解码 80×60，四象限颜色/透明度正确；源文件 SHA256 复制前后均为 `8DB5F9B4…162487` | PASS |
| 2 | 原始 DIBV5 | 124 字节头、32 bpp、top-down、BGRA、alpha 255 | 原始 format 17 共 19,324 B（124 + 80×4×60）；headerSize 124、width 80、height **-60**、planes 1、bitCount 32、compression 3、sizeImage 19,200、masks `00FF0000/0000FF00/000000FF/FF000000`、CSType `0x73524742`、intent 4；首像素 BGRA `00 00 FF FF`（左上红，A=255） | PASS |
| 3 | 双击打开 | 不复制；默认程序打开 | 双击前剪贴板序号 942；双击后 300 ms 与 1.5 s 胶囊均未出现；序号仍 **942**；日志 `shell: opened … (ShellExecuteW code 42)`；本轮没有新的复制日志 | PASS（命令行/日志）；GUI 展示见 §5 |
| 4 | 叉热区与无障碍 | 26 dp、可聚焦、有标题 | CSS `width/height: 26px`；`getBoundingClientRect()` 受 hover 1.03 缩放为 26.4–27.4 px；按钮 `aria-label`/`title` 均为「取下线」；卡片 `title` 为「单击复制，双击打开」、`aria-label` 含文件名+提示 | PASS |
| 5 | watched-folder 叉 | 只下线，不删文件 | 状态 revision 2→3，剩余 1 条；文件仍存在；日志 `take_down: …\Task12 watched image.png only leaves the line (watched-folder item); the file is kept` | PASS |
| 6 | 自有截图叉 | 文件进回收站，卡片消失 | 状态 revision 3→4，剩余 0 条；文件不存在；日志 `trash: recycled … into the Recycle Bin (FOF_ALLOWUNDO)`；Recycle Bin 枚举到 `Task12 own image.png`、`DeletedFrom=%USERPROFILE%\AppData\Local\Hangline\Screenshots`、`DateDeleted=2026-10-30T01:39:38`（日志 UTC） | PASS |
| 7 | 按压 busy / 释放清理 | 按住不收起；释放后恢复 500 ms 防抖 | CDP 按住 3.5 s（指针在面板外、peek 已过期）：每 500 ms 采样 `data-revealed=true`、`translateY=0`；释放后 1.2 s 采样 `data-revealed=false`、`translateY=-222`；日志 `tuck (PointerLeave) … busy=false, pointer outside Some(510) ms` | PASS（受控轮次，功能代码与最终提交相同） |
| 8 | 双击间隔命令 | 与系统一致 | `get_double_click_interval_ms` 返回 500；`System.Windows.Forms.SystemInformation.DoubleClickTime` 也是 500 | PASS |
| 9 | Recording 只写文件 | 仅 `CF_HDROP` | `clipboard_io::tests::recording_only_ships_file_drop` 通过 | PASS（单测；未跑录屏运行时） |
| 10 | 复制不修改源文件 | 哈希/mtime 不变 | 单击前后 SHA256 均为 `8DB5F9B4…162487`；回收前再次读取仍相同 | PASS |
| 11 | 状态广播 | `remove` 后 revision 递增并 emit | 每次叉后 `get_app_state` revision +1（2→3→4；busy 清理 7→8）；前端卡片立即消失 | PASS |
| 12 | 回收站路径校验 | 非自有目录不删 | 单测覆盖 7 个 case（大小写、`.`/`..`、同名前缀、watched-folder、系统截图、录屏、目录本身）+ 相对路径拒绝；运行时 watched 文件保留 | PASS（单测 + 运行时 watched 分支） |

## 4. 证据

### 4.1 单击后的剪贴板（`clipboard-probe.ps1` 节选）

```json
{
  "sequence": 942,
  "formats": ["PNG", "Format17", "FileDrop", "Bitmap", "DeviceIndependentBitmap"],
  "pngWidth": 80, "pngHeight": 60,
  "pngTopLeft": "FFFF0000", "pngTopRight": "FF00FF00", "pngBottomLeft": "FF0000FF",
  "dibHeaderSize": 40, "dibWidth": 80, "dibHeight": -60, "dibCompression": 3,
  "dibFirstPixel": "00 00 FF FF",
  "fileDrop": ["%USERPROFILE%\\AppData\\Local\\Hangline\\Screenshots\\Task12 own image.png"],
  "hasText": false
}
```

`dibHeaderSize: 40` 是 .NET `Clipboard.GetData("DeviceIndependentBitmap")` 的归一化视图（Windows 由 CF_DIBV5 合成 CF_DIB）；format 17 原始 payload 由下节 P/Invoke 直接读取。

### 4.2 原始 CF_DIBV5（`clipboard-raw-dibv5.ps1`）

```json
{
  "payloadBytes": 19324, "headerSize": 124, "width": 80, "height": -60,
  "planes": 1, "bitCount": 32, "compression": 3, "imageSize": 19200,
  "redMask": "0x00FF0000", "greenMask": "0x0000FF00",
  "blueMask": "0x000000FF", "alphaMask": "0xFF000000",
  "csType": "0x73524742", "intent": 4,
  "firstPixelBgra": "00 00 FF FF",
  "matchesExpectedSize": true
}
```

### 4.3 日志节选（T12 评审修复轮次，2026-10-30 10:21–10:23）

```text
[2026-10-30][02:21:15][hangline_lib::clipboard_io][INFO] clipboard: copying …\Task12 fix own image.png as [Png, DibV5, FileDrop] (3 payload(s), 19880 byte(s), owner Some(HWND(0xbd0ae4)))
[2026-10-30][02:21:15][hangline_lib::clipboard_io][INFO] clipboard: wrote 3 format(s) and verified all of them
[2026-10-30][02:21:15][hangline_lib::watchers::clipboard][INFO] clipboard: ignored a copy (formats: PNG, CF_DIBV5, CF_HDROP, CF_BITMAP, CF_DIB)
[2026-10-30][02:23:02][hangline_lib][INFO] take_down: …\Task12 fix own image.png only leaves the line (the file is already gone); the file is kept
[2026-10-30][02:23:02][hangline_lib][INFO] take_down: item 6dee74851ac1c87c3c08ada14ecf9686 is off the line
```

**关于 `ERROR_CLIPBOARD_NOT_OPEN`（更正 §6.3 旧结论）**：它**不是**进程内 clipboard watcher 的读/写竞争。Windows 剪贴板由打开它的线程拥有，另一个线程不能关闭本线程打开的剪贴板；旧日志里 `CloseClipboard` 发生在 watcher 读取之前，时间顺序也不支持竞争解释。真实原因是 `OpenClipboard(NULL)` 已知的间歇性行为：NULL owner 把剪贴板关联到调用任务，Set/Get/Close 偶发报 `ERROR_CLIPBOARD_NOT_OPEN`。修复为：

1. 用面板窗口的 HWND 作为 owner（Tauri `WebviewWindow::hwnd()`；拿不到时才回退 NULL 并记录日志）；
2. `EmptyClipboard` + 全部 `SetClipboardData` 之后，用 `IsClipboardFormatAvailable` 逐个校验期望格式（`CF_HDROP`、注册的 `"PNG"` id、`CF_DIBV5`）；
3. 任一格式缺失则整套写入重试（初次 + 2 次重试，间隔 30 ms），仍失败返回 `Err`，前端不显示胶囊，绝不把部分写入报告为成功；
4. `ERROR_CLIPBOARD_NOT_OPEN` 判定为可重试，其他错误直接失败并记录。

修复轮次四次单击复制均为 `owner Some(HWND(0xbd0ae4))`、`wrote 3 format(s) and verified all of them`，日志中没有 `CloseClipboard` 警告/错误；watcher 仍按设计忽略自己写入的副本。

### 4.4 busy 采样（受控轮次）

```json
{"during":[
 {"elapsed":500,"revealed":"true"},{"elapsed":1000,"revealed":"true"},
 {"elapsed":1500,"revealed":"true"},{"elapsed":2000,"revealed":"true"},
 {"elapsed":2500,"revealed":"true"},{"elapsed":3000,"revealed":"true"},
 {"elapsed":3500,"revealed":"true"}],
 "afterRelease":{"revealed":"false","transform":"matrix(1, 0, 0, 1, 0, -222)"}}
```

日志：`tuck (PointerLeave) at +224386 ms (busy=false, pointer outside Some(510) ms)`，即 busy 期间不收起，释放后重新计 500 ms 防抖。

### 4.5 回收站

```text
Name: Task12 own image.png
Path: C:\$Recycle.Bin\S-1-5-21-…\$R…png
DeletedFrom: %USERPROFILE%\AppData\Local\Hangline\Screenshots
DateDeleted: 2026-10-30T01:39:38
```

### 4.6 T12 评审修复复跑（owner HWND + 格式校验 + 事件 + 缺失文件）

**运行**：2026-10-30 10:20–10:23，新 debug 二进制（`cargo build`，复制到 `%TEMP%\hangline-task12\hangline.exe`），Vite 7.3.7 dev server，CDP `Input.dispatchMouseEvent` 单击；测试条目为自有目录 80×60 PNG `Task12 fix own image.png`（内容 hash 与主轮次 `own-source.png` 相同）。

**单击复制（同一 CDP 会话内采样胶囊）**：

```json
{"samples":[
  {"offset":450,"pill":null},
  {"offset":650,"pill":null},
  {"offset":900,"pill":"已复制","testid":"已复制"},
  {"offset":1250,"pill":"已复制"},
  {"offset":1800,"pill":"已复制"}],
 "copiedEvents":["6dee74851ac1c87c3c08ada14ecf9686"]}
```

胶囊在复制完成后出现（本机 IPC/手势链路使写入落在约 900 ms 采样点），并在 1.2 s 窗口内保持；页面通过 `plugin:event|listen` 注册的监听器收到 `clipboard-copied`，payload 为卡片 id `6dee74851ac1c87c3c08ada14ecf9686`。多次单击每次成功都收到一次事件。

**剪贴板探测（`clipboard-probe.ps1`，单击后）**：

```json
{
  "sequence": 974,
  "formats": ["PNG", "Format17", "FileDrop", "Bitmap", "DeviceIndependentBitmap"],
  "pngWidth": 80, "pngHeight": 60,
  "pngTopLeft": "FFFF0000", "pngTopRight": "FF00FF00", "pngBottomLeft": "FF0000FF",
  "dibWidth": 80, "dibHeight": -60, "dibCompression": 3,
  "dibFirstPixel": "00 00 FF FF",
  "fileDrop": ["%USERPROFILE%\\AppData\\Local\\Hangline\\Screenshots\\Task12 fix own image.png"],
  "hasText": false
}
```

源文件 SHA256 复制前后均为 `8DB5F9B46904…162487`，mtime 未变；写入日志为 `owner Some(HWND(0xbd0ae4))`、`wrote 3 format(s) and verified all of them`，整段运行无 `CloseClipboard` 警告/错误，watcher 记录 `ignored a copy (formats: PNG, CF_DIBV5, CF_HDROP, CF_BITMAP, CF_DIB)`。

**缺失文件叉卡片（folded minor）**：外部先删除 `Task12 fix own image.png`，再经 CDP 调用 `take_down(id, deleteFile=true)`。命令返回 Ok（`null`），`get_app_state` revision 1→2、items 0，DOM 卡片数 0；日志：

```text
[2026-10-30][02:23:02][hangline_lib][INFO] take_down: …\Task12 fix own image.png only leaves the line (the file is already gone); the file is kept
[2026-10-30][02:23:02][hangline_lib][INFO] take_down: item 6dee74851ac1c87c3c08ada14ecf9686 is off the line
```

没有调用 `trash::recycle`，Recycle Bin 中无 `Task12 fix *` 条目；卡片不再因 `recycle` 的 missing-file `Err` 而卡住。

## 5. 手动待验证（MANUAL-PENDING）

1. **M8 真实粘贴**：用微信 / QQ / 浏览器实际粘贴单击复制的 80×60 图，确认得到图片而不是文件；本轮只验证了程序化剪贴板格式与像素。
2. **默认应用 GUI**：`ShellExecuteW` 返回 42，日志记录打开；首轮干扰运行中 Photos 进程被拉起，最终受控轮次的 GUI 实际展示未逐像素检查（按任务要求不依赖 GUI 判据）。
3. **真实物理鼠标**：单击/双击/叉的交互由 CDP `Input.dispatchMouseEvent` 注入；真实 OS 命中测试（30 Hz hit-test / click-through）与手感未单独复核。共享桌面上发生的真实点击曾触发卡片复制/打开，说明事件路径可达，但不是受控证据。
4. **150% DPI / 触摸**：26 CSS px 热区在 150% 屏上的物理尺寸与触摸命中未测。
5. **录制文件运行时复制**：只由单测覆盖“仅 CF_HDROP”；未用真实 mp4 走 `copy_item`。
6. **删除文件可选参数**：`take_down(id, deleteFile=false)` 无 UI 入口（右键菜单属 Task 13/15），本轮只通过叉的 `deleteFile=true` 验证 watched 降级。
7. **`show_in_explorer`**：可选命令未实现（需要 `Win32_UI_Shell_Common`，当前未启用；未为可选功能扩大 feature 集）。

## 6. 偏差 / 关注点

1. **DIBV5 行序**：plan Task 12 Step 3 写“自底向上”；任务书明确要求“BGRA top-down”。实现采用负 `bV5Height` + 显示顺序行（top-down），原始 format 17 读回 `height=-60` 且首像素为左上红，证明与实现一致。两种语义对正确读取 header 的消费者等价。
2. **`clipboard-copied` 事件**：评审后 `copy_item` 成功时 `app.emit("clipboard-copied", id)`，事件类型在 `src/lib/types.ts` 中镜像（`ClipboardCopiedEvent`）。卡片胶囊仍由命令成功返回驱动，事件是 spec §4.4 的信息性契约；事件发送失败只记 WARN，不把已成功的复制变成错误。复跑中页面监听器已收到该事件（§4.6）。
3. **`CloseClipboard` 的 `ERROR_CLIPBOARD_NOT_OPEN`（已修复）**：根因是 `OpenClipboard(NULL)` 的 NULL-owner 间歇性行为，**不是** watcher 读/写竞争（见 §4.3 的更正）。现以面板窗口 HWND 为 owner，并在写入后逐格式校验；修复轮次不再出现该错误。NULL owner 仅在拿不到 HWND 时作为记录日志的兜底。
4. **共享桌面**：首轮无控运行出现真实点击（两次复制 + 一次打开 + Photos），全部丢弃；受控轮次把 OS 指针移出面板并使用 CDP，仍有一次“点击面板外”将面板收起，叉测试在重新展开后完成。
5. **收尾改动后的复跑**：busy 采样来自主受控轮次；之后做两处非行为改动——`CloseClipboard` 失败降为 debug、`GlassCard` 用 `itemRef` 保持手势回调身份稳定（避免 `state` 广播重建 gesture controller 而取消待复制）。最终二进制在 09:44–09:45 重新验证：单击后胶囊「已复制」、序号 952→958、格式 `PNG, Format17, FileDrop…`、80×60；双击序号 958→958；叉后 revision 2、item 0、文件不存在并进回收站。busy 代码未改。`CloseClipboard` 的根因与后续修复见 §4.3。
6. **26 dp 的测量口径**：CSS 为 26×26 px；`getBoundingClientRect` 在 hover 放大 1.03 后为 26.4–27.4 px，故表格同时记录两者。
7. **未安装 rustfmt**：工具链没有 `cargo-fmt`，无法运行 `cargo fmt --check`；Rust 代码按现有文件风格手写。
8. **缺失文件的叉（folded minor，已修复）**：`trash::resolve_action` 在文件已不存在时把 `Recycle` 降级为 `OffLineOnly`；`take_down` 照常 `remove` + `emit_state`。§4.6 运行时验证了 revision 递增、卡片消失、无回收站条目；单测覆盖该决策。
9. **API 形状变化**：`clipboard_io::copy_image` / `copy_file` / `copy_for_kind` 新增 `Option<HWND>` owner 参数（命令层传入面板 HWND），旧任务书接口中的单参数形式不再存在；这是评审要求的 owner 修复所必需。

## 7. 环境恢复

- App 与 Vite 已停止；`state.json` 已删除（测试前不存在）；测试 PNG 已移除。
- `settings.json` 从备份恢复，SHA256 `8E315C7B…A79089`（与测试前一致，`extraFolders=[]`）；修复轮次未改设置，复跑后哈希仍为 `8E315C7BA3B111186B9BE3C573FBC907FBB5C14C2453E363BD533CE4FCA79089`。
- Recycle Bin 中 5 个 `Task12 *` 测试条目（主轮次 4 个 + 最终冒烟 1 个）已永久删除，枚举剩余 0；修复轮次未回收任何文件（缺失文件只下线），无新增条目。
- 剪贴板恢复为文本 `Hangline T12 fix clipboard baseline`（主轮次为 `Hangline Task12 clipboard baseline`）；未保留测试图片。
- `%LOCALAPPDATA%\Hangline\Screenshots` 中测试前的两张 E5 PNG 与 `probe.txt` 未改动。
- 应用日志保留测试记录（未回滚）；本文件引用的节选另存于 `%TEMP%\hangline-task12\evidence-final.log`（临时，不提交）。
