# P1 剪贴板监听实验报告：隐私过滤、PNG 落盘与跨源去重

- **任务**：Task 9「剪贴板监听」
- **分支 / 基线**：`feat/core`，父提交 `87d9e91`
- **日期**：2026-10-29
- **结论**：消息窗监听链路启动成功；`CF_DIBV5`/`CF_DIB`/`PNG` 三种图片来源均被解码为 RGBA 并以 `Clipboard yyyy-MM-dd HH.mm.ss.png` 落盘、以 `clipboard` 类型上栏；`PNG+文本+HTML`、`ExcludeClipboardContentFromMonitorProcessing`、`CanIncludeInClipboardHistory=0`、`org.nspasteboard.ConcealedType`、纯文本、`CF_HDROP` 六类复制全部被拒；重复内容命中哈希去重；10000×8001 的 PNG 被 80 MP 上限拒绝。程序化判据全过，真实浏览器/截图工具/微信的人工项见 §7。

## 1. 目的与范围

验证 Task 9 实现（`src-tauri/src/watchers/clipboard.rs` + `watchers/mod.rs` 接线）在真实 Tauri 进程中的行为：

1. `RegisterClassW` + `CreateWindowExW(HWND_MESSAGE)` + `AddClipboardFormatListener` + `GetMessageW` 事件驱动监听；
2. 过滤链（design spec 4.6 D）：`PNG`/`CF_DIBV5`/`CF_DIB` 至少其一，且不含文本/文件/隐身/历史开关等标记；
3. `PNG` 优先、`CF_DIBV5`/`CF_DIB` 兜底的 RGBA 解码（16/24/32 bpp、上下行、行对齐、alpha 规则）；
4. 编码一次、哈希一次、`.png.tmp` + rename 原子落盘；同一秒冲突加 `-2/-3`；
5. 与目录 watcher 共享 `Deduper`：剪贴板落盘的文件不会被目录 watcher 二次上栏；重复复制只上一条；
6. `state` 全量事件 + `reveal {reason:"newItem"}`（`revealOnNew=true`）；
7. `watchClipboard=false` 时启动不建立监听、复制不采集。

## 2. 环境

| 项 | 值 |
| --- | --- |
| OS | Windows 11（10.0.26300），交互式桌面会话 |
| Rust | rustc 1.90.0 / cargo 1.90.0 |
| App | Hangline 0.1.0（debug，`cargo build`） |
| 前端 | Vite 7.3.7 dev server（`http://localhost:5173`） |
| 观测 | `%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log`、`%LOCALAPPDATA%\Hangline\state.json`、`Screenshots\`、WebView2 CDP（`--remote-debugging-port=9222`） |
| 剪贴板注入 | `%TEMP%\hangline-task9\clipset.ps1`：`OpenClipboard`/`EmptyClipboard`/`SetClipboardData` P/Invoke，写入真实 `HGLOBAL`（DIB 字节、PNG 字节、`CF_HDROP`、标记格式），非 HTTP mock |

**运行方式（沿用 Task 2/5/7/8 的沙箱 workaround）**：workspace 内的 exe 在初始化 `tauri-plugin-log` 时写 `%LOCALAPPDATA%` 被环境拒绝，因此：

1. `pnpm dev` 启动 Vite；
2. `cargo build` 后把 `src-tauri/target/debug/hangline.exe` 复制到 `%TEMP%\hangline-task9\hangline.exe`（同一 dev 构建、同一 `devUrl`）；
3. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*` 启动该副本；
4. CDP `Runtime.evaluate` 注册 `state`/`reveal` 监听器（`plugin:event|listen` + `transformCallback`）并读 `get_app_state`。

`clipset.ps1` 的 `SetImage` 等价物是**真实 CF_DIB**（WinForms `Clipboard.SetImage` 只写 `CF_BITMAP`，按 spec 4.6 D 不在白名单，故不用于 E2E）：Windows 会把 `CF_DIB` 合成 `CF_BITMAP`/`CF_DIBV5` 一并枚举（实测 `formats: CF_DIB, CF_BITMAP, CF_DIBV5`），链路读取 `CF_DIBV5`。

## 3. 预期 vs 观察

| # | 操作 | 预期 | 观察 | 证据 |
| --- | --- | --- | --- | --- |
| 1 | 启动（`watchClipboard=true`） | 消息窗监听 active | `clipboard: listening for WM_CLIPBOARDUPDATE in a message-only window`；`clipboard listener on` | §4.1 |
| 2 | 复制 8×5 24bpp `CF_DIB` | 上栏 `clipboard` 8×5 | 通过（`CF_DIBV5` 合成格式，`hash 768daaf1da78df28`） | §4.2、§4.4 |
| 3 | 复制 9×6 32bpp `CF_DIB` | 上栏 `clipboard` 9×6，alpha 全 0 → 不透明 | 通过（`hash a2be0f358fd56acd`） | §4.2、§4.5 |
| 4 | 复制 `PNG`+`CF_DIBV5`+`CF_DIB`（无文本） | 优先 PNG 解码上栏 | 通过（`PNG flavour 8x5`，`hash c6faf1988568ac9d`） | §4.2、§4.4 |
| 5 | 复制 `PNG`+DIB+`CF_UNICODETEXT`+`HTML Format`（浏览器式） | 拒绝，无事件 | `ignored a copy (formats: PNG, CF_DIBV5, CF_DIB, CF_UNICODETEXT, HTML Format, CF_LOCALE, CF_TEXT, CF_OEMTEXT, CF_BITMAP)` | §4.3 |
| 6 | 复制 `PNG`+`ExcludeClipboardContentFromMonitorProcessing` | 拒绝 | `ignored a copy (formats: PNG, ExcludeClipboardContentFromMonitorProcessing)` | §4.3 |
| 7 | 复制 `PNG`+`CanIncludeInClipboardHistory`(DWORD 0) | 拒绝 | `the copy opted out of clipboard history` + `ignored a copy (formats: PNG, CanIncludeInClipboardHistory)` | §4.3 |
| 8 | 复制 `PNG`+`org.nspasteboard.ConcealedType` | 拒绝 | `ignored a copy (formats: PNG, org.nspasteboard.ConcealedType)` | §4.3 |
| 9 | 纯文本 | 拒绝（无图片格式） | `ignored a copy (formats: CF_UNICODETEXT, CF_LOCALE, CF_TEXT, CF_OEMTEXT)` | §4.3 |
| 10 | 复制文件（`CF_HDROP`） | 拒绝 | `ignored a copy (formats: CF_HDROP)` | §4.3 |
| 11 | 再次复制第 2 步同一张图 | 哈希去重，不上栏/无事件 | `accepted` 后 `dedupe hit (hash 768daaf1da78df28)`；事件数仍 3 | §4.4 |
| 12 | 复制 IHDR 声明 10000×8001 的 PNG | 超 80 MP 上限，拒绝并记日志 | `image dimensions 10000x8001 exceed the 80000000-pixel (80 MP) cap` | §4.4 |
| 13 | 落盘文件与目录 watcher | 只上栏一次（不重复 `capture`） | `watchers: ignored ... (already hung on the line)` 3 次 | §4.4 |
| 14 | `watchClipboard=false` 重启后复制图片 | 不建监听、不采集 | `clipboard watching disabled in the settings`、`clipboard listener off`；复制后无 clipboard 日志、`state.json` 仍 3 条 | §4.6 |
| 15 | 两次同一秒内的复制 | 文件名加 `-2/-3` | 单测 `same_second_saves_get_numbered_suffixes_instead_of_overwriting` 通过（本轮 E2E 两次复制跨秒） | `cargo test` |

**最终事件计数**：`stateEvents=3`、`revealEvents=3`（3 个新 item）、`revisions=[1,2,3]`、`currentRevision=3`；`gating` 阶段复制后仍为 3 条。

## 4. 证据

### 4.1 启动：消息窗监听

```text
[INFO][hangline_lib::watchers::clipboard] clipboard: watching the clipboard (listener thread started)
[INFO][hangline_lib::watchers] watchers: 3 folder watcher(s) active, clipboard listener on
[INFO][hangline_lib::watchers::clipboard] clipboard: listening for WM_CLIPBOARDUPDATE in a message-only window
```

### 4.2 CDP 最终快照（真实页面上下文）

```json
{
  "stateEvents": 3,
  "revealEvents": [
    { "reason": "newItem" },
    { "reason": "newItem" },
    { "reason": "newItem" }
  ],
  "revisions": [1, 2, 3],
  "currentRevision": 3,
  "currentItems": [
    { "path": "...\\Screenshots\\Clipboard 2026-10-29 16.53.38.png", "kind": "clipboard", "width": 8, "height": 5 },
    { "path": "...\\Screenshots\\Clipboard 2026-10-29 16.53.39.png", "kind": "clipboard", "width": 9, "height": 6 },
    { "path": "...\\Screenshots\\Clipboard 2026-10-29 16.53.41.png", "kind": "clipboard", "width": 8, "height": 5 }
  ],
  "hookErrors": []
}
```

> 注：CDP 以 JSON number 传输 `u64` 哈希，超过 2^53 会在 JS 侧丢精度，因此报告不引用 JS 的 `hash` 数值；精确值以 `state.json` 与日志十六进制为准。

### 4.3 接受 / 拒绝日志（同一进程）

```text
[INFO][clipboard] clipboard: accepted a copy (CF_DIBV5 flavour, 8x5, formats: CF_DIB, CF_BITMAP, CF_DIBV5)
[INFO][clipboard] clipboard: hung ...\Clipboard 2026-10-29 16.53.38.png (Clipboard) 8x5 hash 768daaf1da78df28
[INFO][dirs] watchers: ignored ...\Clipboard 2026-10-29 16.53.38.png (already hung on the line)
[INFO][clipboard] clipboard: accepted a copy (CF_DIBV5 flavour, 9x6, formats: CF_DIB, CF_BITMAP, CF_DIBV5)
[INFO][clipboard] clipboard: hung ...\Clipboard 2026-10-29 16.53.39.png (Clipboard) 9x6 hash a2be0f358fd56acd
[INFO][dirs] watchers: ignored ...\Clipboard 2026-10-29 16.53.39.png (already hung on the line)
[INFO][clipboard] clipboard: accepted a copy (PNG flavour, 8x5, formats: PNG, CF_DIBV5, CF_DIB, CF_BITMAP)
[INFO][clipboard] clipboard: hung ...\Clipboard 2026-10-29 16.53.41.png (Clipboard) 8x5 hash c6faf1988568ac9d
[INFO][dirs] watchers: ignored ...\Clipboard 2026-10-29 16.53.41.png (already hung on the line)
[INFO][clipboard] clipboard: ignored a copy (formats: PNG, CF_DIBV5, CF_DIB, CF_UNICODETEXT, HTML Format, CF_LOCALE, CF_TEXT, CF_OEMTEXT, CF_BITMAP)
[INFO][clipboard] clipboard: ignored a copy (formats: PNG, ExcludeClipboardContentFromMonitorProcessing)
[INFO][clipboard] clipboard: the copy opted out of clipboard history; not capturing it
[INFO][clipboard] clipboard: ignored a copy (formats: PNG, CanIncludeInClipboardHistory)
[INFO][clipboard] clipboard: ignored a copy (formats: PNG, org.nspasteboard.ConcealedType)
[INFO][clipboard] clipboard: ignored a copy (formats: CF_UNICODETEXT, CF_LOCALE, CF_TEXT, CF_OEMTEXT)
[INFO][clipboard] clipboard: ignored a copy (formats: CF_HDROP)
[INFO][clipboard] clipboard: accepted a copy (CF_DIBV5 flavour, 8x5, formats: CF_DIB, CF_BITMAP, CF_DIBV5)
[INFO][clipboard] clipboard: dedupe hit (hash 768daaf1da78df28); the same image is already hung
[WARN][clipboard] clipboard: cannot decode the copied image (PNG flavour): image dimensions 10000x8001 exceed the 80000000-pixel (80 MP) cap
```

### 4.4 落盘的 `state.json`（3 条）

| path | kind | width×height | hash (u64 十进制) | 文件字节 |
| --- | --- | --- | --- | --- |
| `...\Clipboard 2026-10-29 16.53.38.png` | clipboard | 8×5 | 8542672023920893736 | 160 |
| `...\Clipboard 2026-10-29 16.53.39.png` | clipboard | 9×6 | 11726827202440161997 | 172 |
| `...\Clipboard 2026-10-29 16.53.41.png` | clipboard | 8×5 | 14338038001062947997 | 159 |

哈希与日志十六进制一一对应：`0x768DAAF1DA78DF28`、`0xA2BE0F358FD56ACD`、`0xC6FAF1988568AC9D`。用 `System.Drawing.Image.FromFile` 复核三个文件均为有效 PNG，尺寸与 item 一致（8×5、9×6、8×5）。

### 4.5 解码路径

- 24bpp `CF_DIB`（8×5）→ Windows 合成 `CF_DIBV5`（244 B）→ 读 32bpp V5 → RGBA → PNG 160 B；
- 32bpp `CF_DIB`（9×6，alpha 全 0）→ 强制不透明 → PNG 172 B；
- `PNG` 注册格式（8×5）→ 直接解码 → PNG 159 B（内容与 24bpp 样本不同，`hash c6faf…`）；
- 单测另外覆盖 24bpp 行对齐（3×2，stride 12）、16bpp RGB555、16bpp 565 `BI_BITFIELDS`、V5 掩码、top-down（负高度）、8bpp/截断/超大 header 拒绝。

### 4.6 开关门控

`settings.json` 置 `watchClipboard=false` 后重启并复制同一张图：

```text
[INFO][watchers] watchers: clipboard watching disabled in the settings
[INFO][watchers] watchers: 3 folder watcher(s) active, clipboard listener off
```

复制后日志无任何 `clipboard:` 行，`state.json` 仍为 3 条（复制未产生 item）。测试后 `settings.json` 已按字节恢复（`Get-FileHash` 与备份一致）。

### 4.7 首次读取竞态的发现与处理（实现细节）

首轮 E2E 出现 `GetClipboardData(17/8) failed: 线程没有打开的剪贴板 (0x8007058A)`：`WM_CLIPBOARDUPDATE` 会在 owner 写入多格式的过程中多次投递，第一帧读到的是中间态（只含 `CF_DIB`），且与 owner 的 `CloseClipboard` 竞争。修复（已包含在提交内）：

1. 通知到达后先等待 `CLIPBOARD_SETTLE_DELAY = 120 ms`，再重新读取序列号；同一最终状态排队的多个通知被序列号合并为一次读取；
2. 若“宣告有图片但读取失败”，整个读取（open → 枚举 → 过滤 → 读字节）最多重试 `READ_ATTEMPTS = 3` 次，每次 100 ms；重试会重新跑过滤链（只会更严格）；
3. 重试仍失败只记一条 warn，不重复刷日志。

修复后本轮 3 次接受、6 次拒绝、去重与超限均稳定通过；`GetClipboardData` 竞态未再出现。

## 5. 关键命令

```powershell
cargo test  --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml --all-targets   # 0 warnings
pnpm check                                                       # exit 0
pnpm dev                                                         # Vite :5173
# 复制 target/debug/hangline.exe 到 %TEMP%\hangline-task9 后：
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222 --remote-allow-origins=*'
Start-Process .\hangline.exe
# CDP: Runtime.evaluate 注册 plugin:event|listen('state'/'reveal')
# 剪贴板：clipset.ps1 -Action dib24|dib32|png|png_text|png_exclude|png_history|png_concealed|text|files|huge
```

## 6. 自动化测试（`cargo test`）

`cargo test --manifest-path src-tauri/Cargo.toml` → **95 passed, 0 failed, 5 ignored**（5 个 ignored 为既有 Task 5 的 E5 环境测试）。Task 9 新增 14 个：

- `should_accept` 决策表（1）：接受 `PNG`/`CF_DIBV5`/`CF_DIB`（含大小写不敏感）；拒绝 `CF_UNICODETEXT`/`CF_TEXT`/`CF_HDROP`/`ExcludeClipboardContentFromMonitorProcessing`/`Clipboard Viewer Ignore`/`org.nspasteboard.ConcealedType`/`HTML Format`/`HISTORY_EXCLUDED`；`CanIncludeInClipboardHistory` 仅存在（非 0）可通过；空列表/仅 `CF_BITMAP` 拒绝；
- DIB 解码（7）：32bpp 底向上 + alpha 全 0 → 255、32bpp 有 alpha 保留、24bpp stride 12 行对齐、16bpp RGB555、16bpp 565 BITFIELDS、V5 掩码 + top-down、8bpp/截断/12B 头/缺像素拒绝；
- 上限（2）：80 MP 正好通过/超 1 拒绝/`u32::MAX×u32::MAX` 不溢出；64 MiB payload 边界；
- PNG（1）：`IEND` 截断块对齐尾部 padding；
- 文件名（2）：`Clipboard yyyy-MM-dd HH.mm.ss` 精确格式；同秒冲突 `-2`/`-3`；
- 保存 + 去重集成（1）：PNG round-trip、`hash_file` 与内存哈希一致、重复内容不再落盘、已在线路径不污染 LRU。

## 7. 手动待验证（真实桌面/真实应用）

1. **真实浏览器复制图片**：Chrome/Edge 右键「复制图片」通常同时带 `CF_DIB`/`PNG`/`HTML`/文本 → 按设计不采集；若某站点只写 `PNG`/`DIB`（无文本），应上栏。本轮用 P/Invoke 合成同样的格式组合验证过滤逻辑，未真实点击浏览器。
2. **Win+Shift+S 仅剪贴板截图**：截图工具写 `CF_DIBV5`/`PNG`（通常无文本）→ 应上栏；本轮用 DIB 合成数据覆盖同链路。
3. **微信/QQ/Excel 复制**：Excel 区域复制通常带文本 → 不采集；聊天软件图片可能只写 `CF_DIB` → 应上栏。需真实应用人工确认。
4. **托盘 Quit 优雅停机**：agent 无法点击托盘，本轮用 `Stop-Process` 结束进程；`ClipboardWatcher::drop`（`WM_CLOSE` 唤醒 + join）与 `Watchers::drop` 由代码审查覆盖，真实退出路径待 Task 15/17 人工确认。
5. **轮询兜底**：`AddClipboardFormatListener` 失败/消息窗创建失败的分支在真实进程中未触发（无法在不改代码的情况下强制失败），仅有代码审查；`GetClipboardSequenceNumber` 每 300 ms 的比较逻辑与无窗口时的 50 ms 分片睡眠已在单测外审阅。
6. **系统剪贴板历史/云剪贴板干扰**：本轮 E2E 期间未启用剪贴板历史；真实环境若由系统服务在 owner 之后重写剪贴板，可能产生额外通知（序列号去重 + 哈希去重应吸收）。

## 8. 清理与恢复

- 删除本轮创建的 3 个 `Clipboard *.png`、以及测试产生的 `state.json`（测试前不存在）；
- `settings.json` 恢复为测试前原文件（`Get-FileHash` 与备份一致）；`watchClipboard` 恢复为 `true`；
- 恢复测试前的 `Hangline.log`（备份覆盖，字节级一致）；
- 停止 app 与 Vite（5173/9222 均无监听）；删除 `%TEMP%\hangline-task9`；
- 未触碰用户既有的 `Hangline 2026-10-29 13.12.25.png`、`Hangline 2026-10-29 13.16.00.png`、`probe.txt`。

## 9. 结论与偏差

- 事件驱动监听、过滤链、三种图片来源的解码、原子落盘、哈希/路径双层去重、`state`/`reveal` 事件、`watchClipboard` 门控均在真实进程中成立。
- **偏差 1（解码优先级）**：spec 4.7 写「优先 `CF_DIBV5`，其次 `PNG`、`CF_DIB`」；任务书要求「优先 `PNG`，其次 `CF_DIBV5`/`CF_DIB`」。实现按任务书：`PNG` 自描述、无损、alpha 正确。E2E 中带 `PNG` 的复制走 PNG 分支，只有 DIB 的复制走 V4/V5 分支，两种路径均验证。
- **偏差 2（`CanIncludeInClipboardHistory` 值）**：纯函数 `should_accept` 只看格式名；读取端在「格式存在且 DWORD=0 或读不到」时追加派生标记 `CanIncludeInClipboardHistory=0` 参与决策，日志仍只打印 owner 的真实格式名。
- **偏差 3（`CF_BITMAP` only）**：按 spec 只接受 `PNG`/`CF_DIBV5`/`CF_DIB`；仅写 `CF_BITMAP` 的复制（如 WinForms `Clipboard.SetImage`）不采集。
- **偏差 4（权限）**：Windows 无 POSIX `0600`；落盘文件继承 `%LOCALAPPDATA%\Hangline\Screenshots` 的「仅当前用户（+SYSTEM/Administrators）」ACL。`apply_restrictive_permissions` 在 Windows 为文档化 no-op，非 Windows 测试目标设 0600。
- **偏差 5（门控时机）**：`watchClipboard` 仅启动时读取；运行时切换需要 Task 15 的重启 API。
- **偏差 6（settle/重试常量）**：120 ms 延迟与 3 次重试是为修复真实观测到的 owner 竞态而加；若后续发现真实应用写入更慢，可在 `clipboard.rs` 单点调整。
