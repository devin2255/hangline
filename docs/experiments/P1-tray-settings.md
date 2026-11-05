# P1 Task 15 实验报告：托盘完整菜单、设置联动、热键持久化与 READY gating

- **日期**：2026-11-02（本地时间 09:33–10:01，UTC+8；应用日志时间戳为 UTC）
- **分支**：`feat/core`（Task 15，base `b0213f0` + 控制器 `76d8112`）
- **结论**：在自动化可测范围内 **PASS**。**注：本报告是 P1 历史记录；表中 #20“检查更新”当时是占位 URL、应用不出网，P5 起已改为 `tauri-plugin-updater` 真实检查（唯一网络请求），以 `README`/`P5-core.md` 为准。**完整托盘菜单（en/zh-Hans、勾选态、动作）、设置项即时生效与重启保留、目录/剪贴板 watcher 真正启停、开机自启注册表、语言切换（托盘重建 + 前端 i18n）、改键窗口（捕获/保存/Esc/冲突回滚，M15）、READY 早于前端的 toggle 排队 + 3 s 兜底、全部取下线语义、Quit 优雅退出均已运行时验证。系统文件夹选择器为 MANUAL-PENDING（模态对话框无法在无人值守下点击）；卡右键菜单按 Task 15 范围注记为 V1.1 已知缺口。

## 1. 环境

| 项 | 值 |
| --- | --- |
| OS | Microsoft Windows 11（10.0.26300），Per-Monitor-V2，中文系统 UI |
| Rust / 依赖 | rustc 1.90.0 MSVC；tauri 2.12.5、tauri-plugin-autostart 2.7.0、global-shortcut 2.4.1 |
| 前端 | Vite 7.3.7 dev server（`http://localhost:5173`） |
| 应用 | Hangline 0.1.0，debug（`cargo build`） |
| 显示器 | `\\.\DISPLAY2` 1920×1080 @100%（主屏，panel 初始目标）；`\\.\DISPLAY1` 1920×1080 @100%（左侧） |
| 观测 | `%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log`（dev 日志会滚动）、进程 stdout、WebView2 CDP（`--remote-debugging-port=9222`）、Win32 窗口/菜单探针 |
| 数据 | `%LOCALAPPDATA%\Hangline\{settings.json,state.json}`；测试前后已备份/还原（见 §8） |

## 2. 运行方式与 agent 沙箱限制

与 E1–E3、Task 10/14 相同：沙箱内 workspace 的 exe 无法写 `%LOCALAPPDATA%`（`pnpm tauri dev` os error 5），采用等价方式：

```powershell
pnpm dev                                                        # Vite :5173
cargo build --manifest-path src-tauri/Cargo.toml
Copy-Item src-tauri\target\debug\hangline.exe $env:TEMP\hangline-t15\hangline.exe
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222 --remote-allow-origins=*'
& $env:TEMP\hangline-t15\hangline.exe                           # stdout/stderr 重定向
```

驱动方式：

- **CDP**（Node 22 内置 WebSocket）：`Runtime.evaluate` 调用 `window.__TAURI_INTERNALS__.invoke(...)`、读取 DOM、`Page.captureScreenshot` 截图改键窗口。
- **真实 Win32 托盘菜单**：向本进程的 `tray_icon_app` 窗口投递 `WM_USER_TRAYICON`(6002) + `WM_RBUTTONUP`（tray-icon crate 的真实右键路径），菜单由 `TrackPopupMenu` 弹出；用 `#32768` 窗口 + `MN_GETHMENU`/`GetMenuItemRect` 枚举条目与矩形，用真实鼠标注入点击条目；截图取菜单窗口矩形。测试机存在外部指针漂移，因此点击是“定位后立即 down/up”，并优先在单次脚本调用内完成“打开菜单 → 打开子菜单 → 选择”。
- **热键冲突**：独立 helper 进程 `RegisterHotKey(Ctrl+Alt+X)` 占住组合；另用 `keybd_event` 注入真实全局热键。

**截图范围（隐私约束）**：所有提交的 PNG 只包含 Hangline 自身 UI，不含桌面/其他应用内容：

- 顶层菜单截图严格截取 `#32768` 菜单窗口矩形（+1 px 边距），不截菜单之外的屏幕区域；
- 改键窗口截图来自 CDP `Page.captureScreenshot`，仅包含该 webview 的画面；
- `P1-tray-settings-shortcut-menu-conflict.png` 是在屏幕不重叠的两个弹出菜单（主菜单 + 快捷键子菜单），其联合外接矩形会包含菜单之间的桌面区域，因此改为分别截取两个 `#32768` 窗口矩形后上下拼接（深色背景 + 自绘说明文字），不含任何菜单之外的桌面内容。

## 3. 预期 vs 观察

| # | 检查项 | 预期 | 观察 | 结论 |
| --- | --- | --- | --- | --- |
| 1 | 完整菜单（zh-Hans） | spec §3.5 全部项，系统语言为中文时托盘中文 | 真实菜单 20 项：显示线(Ctrl+Alt+T)、全部取下线、截图…、—、处理系统截图☑、挂起复制的图片☑、挂起录屏☑、线出现方式▸、也监听文件夹▸、保留数量▸、尺寸▸、快捷键▸、打开截图文件夹、—、声音☑、开机启动☐、检查更新…、语言▸、—、退出 Hangline；截图 `P1-tray-settings-menu-zh.png` | PASS |
| 2 | 菜单文案语言（en） | 语言切到 en 后整棵菜单重建为英文 | 点击 语言▸English 后日志 `tray: menu rebuilt for language en`；实际菜单：Show line / Take everything down / Take screenshot… / Handle system screenshots / Hang copied images / Handle recordings / Bring the line down / Also watch folders / Keep on line / Size / Shortcut / Open screenshots folder / Sounds / Open at login / Check for updates… / Language / Quit Hangline；截图 `P1-tray-settings-menu-en.png` | PASS |
| 3 | 勾选态与状态实时同步 | 菜单勾选 = 持久化设置/真实注册；空线时“全部取下线”禁用；面板可见性改变 Show/Hide 文案 | 初始 zh 菜单：处理系统截图/剪贴板/录屏/声音 ☑、开机启动 ☐；点击“处理系统截图”后再次打开菜单该行 ☐（截图 `P1-tray-settings-menu-zh.png`；此时线上有 1 张卡片，因此“全部取下线”为可用态）；空线时“Take everything down”变灰的 disabled 状态见重启后截图 `P1-tray-settings-menu-en-restart.png` | PASS |
| 4 | 真实菜单动作驱动设置 | 点击即调用 `set_setting` 同一路径，并即时生效 | 点击“处理系统截图”：日志 `tray: menu action toggle system screenshots (tray.handle_screenshots)` → `settings: applying restart folder watchers` → `watchSystemScreenshots=false`；CDP 读回 `watchSystemScreenshots=false`、revision+1 | PASS |
| 5 | 目录 watcher 真正重启 | 关闭系统截图目录后只保留其余 watcher；再次开启恢复 | `watchers: folder watchers restarted (3 stopped, 2 active; watchSystemScreenshots=false, watchRecordings=true, extraFolders=0)`；关闭录屏后 `(2 stopped, 1 active; watchRecordings=false)`；重启实例启动日志只启动剩余集合 | PASS |
| 6 | 剪贴板监听启停 | watchClipboard=false 时监听停止，true 时启动 | `watchers: clipboard listener stopped (watchClipboard = false)`；重启实例 `clipboard watching disabled in the settings`、`clipboard listener off`；再开启走 `settings: applying start clipboard listener` | PASS |
| 7 | 额外目录（set_setting 路径） | 新目录先 `allow_asset_dir` 再 watch；新文件上栏 | `settings: applying restart folder watchers` → `watchers: asset protocol allows C:\...\extra-watch (recursive)` → `watchers: hung ...extra-3-copy.png (WatchedFolder) 200x150`；CDP items=2 kinds=`capture,watchedFolder`。移除目录：`folder watchers restarted (2 stopped, 1 active; extraFolders=0)`，scope 不撤销、已上栏卡片保留（Task 10 契约） | PASS（Add Folder… 对话框 MANUAL-PENDING） |
| 8 | Keep on line / Size | 立即生效、重应用窗口几何、重启保留 | 菜单 尺寸▸Large：日志 `panel: moved to \\.\DISPLAY2 at (0,0), 1920x246 px (cardSize Large)`、`set_setting cardSize persisted; panel geometry re-applied (moved=true…)`；保留数量▸25：`keepOnLine persisted; … (moved=false)`；重启后 settings 仍是 `cardSize=large/keepOnLine=25`，前端 `data-card-size=large` | PASS |
| 9 | 声音开关 | 复制/丢弃提示音随 sound 设置 | 菜单点击 Sounds → `sound=false` 持久化（重启后仍 false）；`MessageBeep` 由 `copy_item`/`take_down`/全部取下线调用（无音效客观测量，代码路径 + 设置持久化验证） | PASS（听感 MANUAL） |
| 10 | 开机自启 | 菜单勾选 = 真实注册；启用写 HKCU Run，禁用删除 | 菜单“Open at login”：`settings: applying enable autostart` → `registered Hangline as a login item`；`reg query HKCU\...\Run /v Hangline` → `Hangline REG_SZ C:\...\hangline.exe`；再次点击 → `removed Hangline from the login items`，注册表值消失、菜单勾选回到 ☐ | PASS |
| 11 | 语言切换（前端同步） | 前端 `src/i18n` 随 settings.language 切换 | 点击 语言▸English 后 CDP：`document.documentElement.lang="en"`、卡片 `title="Click to copy, double-click to open"`、取下线 `"Take down"`；切回 zh 时此前为 `"单击复制，双击打开"/"取下线"`；`settings.language` 持久化并在重启后保持 en | PASS |
| 11a | **系统语言解析统一（review fix）** | `language:"system"` 时托盘与面板必须使用同一个 Rust 解析出的语言；快照携带 `resolvedLanguage`，前端不再自己解析 `system` | `settings.language=system` 启动：托盘日志 `tray: icon and full menu created (zh-Hans language)`；CDP `get_app_state` → `{settingsLanguage:"system", resolvedLanguage:"zh-Hans", hasResolvedLanguageKey:true}`，DOM `document.documentElement.lang="zh-Hans"`、卡片 title 中文；切 `language:"en"` → `resolvedLanguage:"en"`、DOM/卡片英文、日志 `tray: menu rebuilt for language en`；切回 `system` → 两者同时回到 `zh-Hans`（`menu rebuilt for language zh-Hans`）。混合“显示语言/区域格式”场景由 `i18n::tests` + `app_state::tests` 单测覆盖（本机无法改 Windows 显示语言） | PASS |
| 12 | 改键窗口（截图/保存） | 托盘 快捷键▸行显示当前键，点击打开 `?window=shortcut`；捕获键 → 保存 → 持久化 + 重注册 + 关闭 | 点击 Capture 行：日志 `change-shortcut window opened for the capture target`，CDP target `window=shortcut&target=capture`；窗口显示 `Current shortcut: Ctrl+Alt+Shift+F9`、Save 禁用；注入 Ctrl+Alt+Y 后预览 `Ctrl+Alt+Y`（截图 `P1-tray-settings-shortcut.png` 为 Ctrl+Alt+Z 预览态），Enter 保存：`hotkeys: registered capture shortcut Ctrl+Alt+Y`、窗口关闭、`captureHotkey=Ctrl+Alt+Y` | PASS |
| 13 | 改键窗口取消 | Esc 关闭且不改设置 | 注入 Escape：窗口关闭、日志 `change-shortcut window closed`、`captureHotkey` 不变（仍 Ctrl+Alt+Y） | PASS |
| 14 | **M15 冲突回滚** | 新键被占用：注册失败 → 旧键保留、设置回滚、窗口内显示错误、托盘提示 | helper 占用 Ctrl+Alt+X 后保存：`hotkeys: failed to register capture shortcut Ctrl+Alt+X: HotKey already registered…`；`settings: captureHotkey rolled back after a failed re-register capture hotkey…`；窗口内联错误（截图 `P1-tray-settings-shortcut-conflict.png`）；CDP `captureHotkey` 仍为 Ctrl+Alt+Z；注入 Ctrl+Alt+Z → `hotkeys: capture requested`（旧键仍有效），注入 Ctrl+Alt+X 无 Hangline 日志；托盘 快捷键▸ 显示 `Capture: Ctrl+Alt+Z ⚠` + 禁用提示行 `⚠ Shortcut unavailable: Capture — HotKey already registered…`（截图 `P1-tray-settings-shortcut-menu-conflict.png`） | PASS |
| 15 | 热键启动来源 | 启动注册持久化 settings 而非默认值 | 预置 `captureHotkey=Ctrl+Alt+Shift+F9` 启动：日志 `hotkeys: registered capture shortcut Ctrl+Alt+Shift+F9`（默认 Ctrl+Alt+S 未注册）；重启后 `registered capture shortcut Ctrl+Alt+Z` | PASS |
| 16 | **READY gating 排队** | 前端 ready 前的 toggle 排队，ready 后按序 flush；`on_ready` 不覆盖 | 无 Vite 运行（前端永远不 ready）：启动 +737 ms 注入 Ctrl+Alt+T → `reveal: queued a Toggle toggle until the panel reports ready (+737 ms; 1 queued so far)` | PASS |
| 17 | **3 s 兜底** | 前端永不 ready 时 3 s 后显示 click-through 面板 | `reveal: panel frontend did not report ready within 3000 ms; showing the click-through panel as the fallback (+3056 ms)` → `panel: shown` → `flushing 1 queued command(s) … the startup reveal is skipped so it cannot clobber them` → `reveal: shown (reason Toggle, peek=false, items=0)` | PASS |
| 18 | 真实 ready 与超时竞态 | 正常 ready 后到达的超时事件被忽略 | 正常启动日志 `panel: frontend reported ready …` 后 `reveal: ready at +313/2586 ms (fallback=false)`；随后 `reveal: READY timeout after the handshake; ignored (+3070 ms)` | PASS |
| 19 | 全部取下线语义 | 自有目录文件进回收站；监听目录文件只下线不删 | 点击菜单“Take everything down”：`take_everything_down: 2 card(s) taken off the line`；`trash: recycled C:\...\Hangline\Screenshots\t15-own.png into the Recycle Bin (FOF_ALLOWUNDO)`；CDP items=0；`Test-Path` 自有文件 false、额外目录文件 true；菜单项随后 disabled | PASS |
| 20 | 检查更新（P1 历史；P5 起改为 `tauri-plugin-updater` 真实检查） | ShellExecute 打开占位 Releases URL，app 自身不出网 | 点击“Check for updates…”：`tray: check for updates requested; opening the placeholder Releases page https://github.com/hangline-app/hangline/releases (Hangline itself performs no network request)` → `shell: opened https://… (ShellExecuteW code 42)` | PASS |
| 21 | Quit 优雅退出 | 停 watchers/clipboard 后退出，无孤儿进程 | 无 Vite 实例与正常实例均点击“Quit Hangline”：`tray: quit requested; stopping the watchers and clipboard listener` → `watchers: stopping N folder watcher(s)…` / `folder watcher(s) stopped` → 进程退出；随后 `RunEvent::Exit` hook 幂等再停一次（0 watchers）；进程列表无残留 hangline | PASS |
| 22 | 重启保留 | 全部设置项跨重启保留 | 重启前 settings：`captureHotkey=Ctrl+Alt+Z, watchSystemScreenshots=false, watchClipboard=false, watchRecordings=false, keepOnLine=25, cardSize=large, sound=false, language=en, startAtLogin=false, extraFolders=[]`；重启后 CDP + `settings.json` 完全一致，托盘 20 项勾选态一致（截图 `P1-tray-settings-menu-en-restart.png`） | PASS |
| 23 | Add Folder… 系统选择器 | 打开系统文件夹选择器，取消 = None，上限 3 | 代码实现（`SHBrowseForFolderW`，无新依赖；取消返回 None）；**review fix**：选择器关闭后重读当前 `extraFolders`，在互斥区内重新校验 `is_dir`/重复/上限并与当前列表合并（`merge_picked_folder` 单测覆盖“stale 快照不复活已移除目录、不丢并发新增、cap/重复/非目录”）；Add/Remove 经同一互斥串行。模态对话框本身无法在无人值守下点击 → **MANUAL-PENDING**；等价路径通过 `set_setting extraFolders` 验证（#7） | MANUAL-PENDING |
| 24 | 卡右键菜单 | spec §3.1 有，但 P1 DoD 未要求 | 未实现，记录为 **V1.1 已知缺口**（Task 15 范围外） | 已知缺口 |
| 25 | 全屏抑制 | P4 项；本任务只接线设置 | 菜单“Over full-screen apps”切换并持久化 `showOverFullScreen`；实际 `SHQueryUserNotificationState` 抑制在 P4 落地 | 仅接线 |
| 26 | 托盘 tooltip | 未读数/0 张 + 冲突后缀 | tooltip 文案由 `tooltip()` 纯函数单测覆盖（`Hangline — 0 card(s) on the line` / zh `Hangline — 线上 0 张` + `— shortcut unavailable`）；托盘图标 tooltip 的可视读取无法自动截图 → MANUAL | PASS（单测）+ MANUAL |

## 4. 证据（节选）

### 4.1 启动：持久化热键 + 语言 + 菜单

```text
[01:37:19][hangline_lib][INFO] hangline: data layer ready with 1 item(s), version 0.1.0
[01:37:20][hangline_lib::hotkeys][INFO] hotkeys: registered capture shortcut Ctrl+Alt+Shift+F9
[01:37:20][hangline_lib::hotkeys][INFO] hotkeys: registered toggle shortcut Ctrl+Alt+T
[01:37:20][hangline_lib::tray][INFO] tray: icon and full menu created (zh-Hans language)
[01:37:20][hangline_lib][INFO] panel: frontend reported ready, showing the panel
[01:37:23][hangline_lib::reveal][DEBUG] reveal: READY timeout after the handshake; ignored (+3070 ms)
```

启动前手工把 `settings.captureHotkey` 设为 `Ctrl+Alt+Shift+F9`，启动注册的是该值而非 Task 3 的内存默认 `Ctrl+Alt+S`，证明热键启动来源已改为持久化 settings。

### 4.2 真实托盘菜单动作（zh → 设置即时生效）

```text
[01:42:23][hangline_lib::tray][INFO] tray: menu action toggle system screenshots (tray.handle_screenshots)
[01:42:23][hangline_lib::actions][INFO] settings: applying restart folder watchers
[01:42:23][hangline_lib::watchers][INFO] watchers: system screenshots disabled in the settings
[01:42:23][hangline_lib::watchers][INFO] watchers: folder watchers restarted (3 stopped, 2 active; watchSystemScreenshots=false, watchRecordings=true, extraFolders=0)
```

### 4.3 语言切换（托盘重建 + 前端 i18n）

```text
[01:44:10][hangline_lib::tray][INFO] tray: menu action set language (tray.language.en)
[01:44:10][hangline_lib::actions][INFO] settings: applying rebuild tray
[01:44:10][hangline_lib::actions][INFO] ipc: set_setting language persisted
[01:44:10][hangline_lib::tray][INFO] tray: language set to "en"
[01:44:10][hangline_lib::tray][INFO] tray: menu rebuilt for language en
```

CDP（切换后）：

```json
{"lang":"en","cards":[{"title":"Click to copy, double-click to open","takeDown":"Take down"}]}
```

切换前同一 DOM 为 `{"lang":"zh-Hans","cards":[{"title":"单击复制，双击打开","takeDown":"取下线"}]}`。

### 4.4 剪贴板/录屏 watcher 重启与开机自启

```text
[01:48:27][hangline_lib::watchers][INFO] watchers: clipboard listener stopped (watchClipboard = false)
[01:48:33][hangline_lib::watchers][INFO] watchers: folder watchers restarted (2 stopped, 1 active; watchSystemScreenshots=false, watchRecordings=false, extraFolders=0)
[01:48:45][hangline_lib::actions][INFO] settings: applying enable autostart
[01:48:45][hangline_lib::actions][INFO] settings: registered Hangline as a login item
[01:49:12][hangline_lib::actions][INFO] settings: applying disable autostart
[01:49:12][hangline_lib::actions][INFO] settings: removed Hangline from the login items
```

注册表（启用后；禁用后值消失）：

```text
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run
    Hangline    REG_SZ    %USERPROFILE%\AppData\Local\Temp\hangline-t15\hangline.exe
```

### 4.5 额外目录：allow_asset_dir → watch → 上栏

```text
[01:50:20][hangline_lib::watchers][INFO] watchers: folder watchers restarted (1 stopped, 2 active; watchSystemScreenshots=false, watchRecordings=false, extraFolders=1)
[01:50:20][hangline_lib::watchers][INFO] watchers: asset protocol allows %USERPROFILE%\AppData\Local\Temp\hangline-t15\extra-watch (recursive)
[01:50:22][hangline_lib::watchers::dirs][INFO] watchers: hung %USERPROFILE%\AppData\Local\Temp\hangline-t15\extra-watch\extra-3-copy.png (WatchedFolder) 200x150 hash ffa221f28d4db150
```

### 4.6 改键窗口与 M15 冲突回滚

CDP 窗口态（保存前）：

```json
{"target":"capture","title":"Change shortcut","current":"Current shortcut: Ctrl+Alt+Z","keys":"Ctrl+Alt+X","saveDisabled":false}
```

冲突保存后（窗口保持打开、内联错误，截图 `P1-tray-settings-shortcut-conflict.png`）：

```text
[01:46:48][hangline_lib::hotkeys][ERROR] hotkeys: failed to register capture shortcut Ctrl+Alt+X: HotKey already registered: HotKey { mods: Modifiers(ALT | CONTROL), key: KeyX, id: 589866 }
[01:46:48][hangline_lib::actions][ERROR] settings: captureHotkey rolled back after a failed re-register capture hotkey: capture shortcut Ctrl+Alt+X: hotkey registration failed: HotKey already registered: …
[01:46:48][hangline_lib][WARN] set_hotkey: capture shortcut rejected: …
```

托盘提示项（真实菜单 dump，截图 `P1-tray-settings-shortcut-menu-conflict.png`）：

```text
Capture: Ctrl+Alt+Z ⚠
Show/hide line: Ctrl+Alt+T
(disabled) ⚠ Shortcut unavailable: Capture — HotKey already registered: HotKey { … key: KeyX … }
```

回滚后旧键仍触发：注入 Ctrl+Alt+Z → `hotkeys: capture requested (capture overlay lands in Task 16)`；注入 Ctrl+Alt+X 无 Hangline 日志（被 helper 占用）。

### 4.7 READY gating（无 Vite：前端永不 ready 的极端场景）

```text
[01:33:50][hangline_lib::hotkeys][INFO] hotkeys: toggle line
[01:33:50][hangline_lib::reveal][INFO] reveal: queued a Toggle toggle until the panel reports ready (+737 ms; 1 queued so far)
[01:33:52][hangline_lib::reveal][WARN] reveal: panel frontend did not report ready within 3000 ms; showing the click-through panel as the fallback (+3056 ms)
[01:33:52][hangline_lib::panel][DEBUG] panel: shown
[01:33:52][hangline_lib::reveal][INFO] reveal: flushing 1 queued command(s) at +3056 ms after the READY timeout fallback (1 toggle(s), 0 new item(s)); the startup reveal is skipped so it cannot clobber them
[01:33:52][hangline_lib::reveal][INFO] reveal: shown (reason Toggle, peek=false, items=0) at +3056 ms
```

正常实例中真实 handshake 先到、超时后到并被忽略（§4.1）。

### 4.8 全部取下线 + Quit

```text
[01:56:19][hangline_lib::tray][INFO] tray: menu action take everything down (tray.take_all_down)
[01:56:19][hangline_lib::trash][INFO] trash: recycled %USERPROFILE%\AppData\Local\Hangline\Screenshots\t15-own.png into the Recycle Bin (FOF_ALLOWUNDO)
[01:56:19][hangline_lib::actions][INFO] take_everything_down: 2 card(s) taken off the line

[02:01:11][hangline_lib::tray][INFO] tray: menu action quit (tray.quit)
[02:01:11][hangline_lib::actions][INFO] tray: quit requested; stopping the watchers and clipboard listener
[02:01:11][hangline_lib::watchers][INFO] watchers: stopping 1 folder watcher(s), clipboard listener off
[02:01:11][hangline_lib::watchers][INFO] watchers: 1 folder watcher(s) stopped
[02:01:11][hangline_lib][INFO] hangline: exit event received; stopping the watchers and clipboard listener
[02:01:11][hangline_lib::watchers][INFO] watchers: stopping 0 folder watcher(s), clipboard listener off
```

### 4.9 重启保留

重启后启动日志：

```text
[02:00:09][hangline_lib::store][INFO] store: loaded 0 item(s) from %USERPROFILE%\AppData\Local\Hangline\state.json
[02:00:10][hangline_lib::hotkeys][INFO] hotkeys: registered capture shortcut Ctrl+Alt+Z
[02:00:10][hangline_lib::tray][INFO] tray: icon and full menu created (en language)
[02:00:10][hangline_lib::watchers][INFO] watchers: system screenshots disabled in the settings
[02:00:10][hangline_lib::watchers][INFO] watchers: recordings disabled in the settings
[02:00:10][hangline_lib::watchers][INFO] watchers: clipboard watching disabled in the settings
[02:00:10][hangline_lib::watchers][INFO] watchers: 1 folder watcher(s) active, clipboard listener off
```

`settings.json`（重启后，CDP 读回一致）：

```json
{
  "captureHotkey": "Ctrl+Alt+Z", "toggleHotkey": "Ctrl+Alt+T",
  "watchSystemScreenshots": false, "watchClipboard": false, "watchRecordings": false,
  "extraFolders": [], "keepOnLine": 25, "cardSize": "large", "sound": false,
  "startAtLogin": false, "language": "en", "theme": "system",
  "revealOnNew": true, "revealPeekMs": 2500, "topEdgeReveal": false,
  "showOverFullScreen": false, "autoCheckUpdates": true, "lastUpdateCheck": ""
}
```

### 4.10 系统语言解析统一（review fix：`resolvedLanguage`）

`settings.language = "system"` 启动（Windows 显示语言 zh-CN）：托盘日志

```text
[02:31:17][hangline_lib::tray][INFO] tray: icon and full menu created (zh-Hans language)
```

同一时刻 CDP 读取 `get_app_state` 与面板 DOM：

```json
{"settingsLanguage":"system","resolvedLanguage":"zh-Hans","hasResolvedLanguageKey":true,
 "documentLang":"zh-Hans","cardTitle":"单击复制，双击打开","cardCount":1}
```

切到显式 `language="en"`（`set_setting`，与托盘 语言▸English 同一路径）：

```json
{"settingsLanguage":"en","resolvedLanguage":"en",
 "documentLang":"en","cardTitle":"Click to copy, double-click to open","cardCount":1}
```

```text
[02:32:21][hangline_lib::tray][INFO] tray: menu rebuilt for language en
```

再切回 `language="system"`，托盘与面板同时回到 zh-Hans：

```json
{"settingsLanguage":"system","resolvedLanguage":"zh-Hans",
 "documentLang":"zh-Hans","cardTitle":"单击复制，双击打开","cardCount":1}
```

```text
[02:32:35][hangline_lib::tray][INFO] tray: menu rebuilt for language zh-Hans
```

Rust 的单次解析链为：`GetUserPreferredUILanguages(MUI_LANGUAGE_NAME)`（Windows 显示语言）→
`GetUserDefaultUILanguage`（LANGID 映射）→ `GetUserDefaultLocaleName`（区域格式 locale）→ `"en"`；
纯函数 `pick_system_language`/`resolved_tag_for` 由 `i18n::tests` 覆盖（含“区域格式 zh-CN 但显示语言
en-US 时必须为 en”的混合场景）。混合 locale 的真实 Windows 设置无法在本机修改，纯逻辑测试覆盖。

## 5. 证据图片

- `P1-tray-settings-menu-zh.png`：zh-Hans 完整顶层菜单（点选“处理系统截图”后：该项 ☐，剪贴板/录屏/声音 ☑；此时线上有 1 张卡片，“全部取下线”为可用态）。
- `P1-tray-settings-menu-en.png`：语言切到 English 后、多次设置切换后的顶层菜单。
- `P1-tray-settings-menu-en-restart.png`：重启后 en 菜单（勾选态与持久化 settings 一致；空线时“Take everything down”为 disabled）。
- `P1-tray-settings-shortcut.png`：改键窗口（目标 Capture，当前 Ctrl+Alt+Y，新捕获 Ctrl+Alt+Z）。
- `P1-tray-settings-shortcut-conflict.png`：M15 冲突内联错误，窗口保持打开，Save 仍可重试。
- `P1-tray-settings-shortcut-menu-conflict.png`：真实托盘 快捷键▸ 子菜单：`Capture: Ctrl+Alt+Z ⚠` + 禁用提示行（两个 `#32768` 菜单区域的紧凑裁剪上下拼接，非桌面截图）。

> 所有截图仅包含 Hangline 自身 UI：菜单截图 = `#32768` 菜单窗口矩形（+1 px 边距）；改键窗口 = CDP webview 画面；子菜单冲突截图 = 两个菜单矩形分别裁剪后拼接。Windows 11 菜单背景为系统合成的半透明材质，菜单矩形内可能透出模糊底色，但不含可读的桌面/其他应用内容。

## 6. 与 spec/plan 的偏差与范围说明

1. **托盘左键 = 单击 toggle**：沿用 Task 3 裁决（左键菜单已禁用；spec §3.5 的“双击=显示线”折叠为单击切换）。
2. **热键冲突提示**：Tauri `TrayIcon` 无 balloon API 且本任务不新增依赖，按 Task 15 控制器注释用「快捷键▸ 内 ⚠ 行 + 禁用状态行 + tooltip（文案单测）」替代 spec §3.11 的“气泡提示”。
3. **`Add Folder…`**：使用 `SHBrowseForFolderW`（spec 允许 IFileOpenDialog/SHBrowseForFolderW 二选一；无新依赖）。模态对话框为 MANUAL-PENDING。
4. **`Check for updates…`**：占位常量 `RELEASES_URL = https://github.com/hangline-app/hangline/releases`（真实仓库建立后替换）；app 自身零网络（仅 ShellExecute 交给浏览器）。
5. **`Take screenshot…`/capture topic**：本轮只提供托盘入口并沿用 Task 3 的临时接线（log + `capture-requested` 事件）；真实截图覆盖层属 Task 16，未伪造截图。
6. **卡右键菜单**：spec §3.1 有但 P1 DoD 未要求，明确记录为 V1.1 已知缺口（不在 Task 15 偷做）。
7. **全屏抑制**：P4 项，仅接线 `showOverFullScreen` 设置（“Over full-screen apps”菜单项）并持久化，未实现 `SHQueryUserNotificationState` 抑制。
8. **移除监听目录不撤销 asset scope**：Task 10 契约；已上栏卡片缩略图继续可读（行为已在 #7 记录）。

## 7. MANUAL-PENDING / 已知限制

1. **Add Folder… 系统文件夹选择器**：模态 `SHBrowseForFolderW` 无法在无人值守桌面点击确定/取消；等价路径（`set_setting extraFolders` 的 allow_asset_dir + watcher 重启 + 上限/重复校验单测）已程序化验证。建议 Task 17 P1 人工验收时点一次选择器（含取消 = None）。
2. **托盘图标 physical 点击**：本机用 `tray_icon_app` 窗口消息（与真实点击同一 tray-icon 路径）打开真实 `TrackPopupMenu` 菜单，并用真实鼠标事件选择菜单项；任务栏溢出区图标的真实人手点击未验证。
3. **真实物理键盘**：热键/改键用 `keybd_event` 注入系统输入队列验证（RegisterHotKey 生效），需人工用真实键盘各按一次。
4. **测试机指针漂移**：远程桌面/其他程序持续移动光标，菜单子级截图（语言▸/文件夹▸）在多次重试后仍无法稳定截取；顶层菜单、快捷键▸ 子菜单与所有动作日志已取证。语言切换本身由真实菜单点击 + 日志 + CDP 双重证明。
5. **tooltip 可视内容**：托盘图标 tooltip 无法自动读取/截图；文案由 `tray::tests::tooltip_reports_item_count_and_hotkey_failure` 覆盖。
6. **声音**：`MessageBeep` 听感未测（无音频观测）；设置门控与调用点已验证。
7. **Check for updates 浏览器落地页**：本次 ShellExecute 成功返回 code 42 并打开默认浏览器；占位 URL 的页面内容属仓库建立后的事项。
8. **混合“显示语言 / 区域格式”的真机验证**：Windows 显示语言与区域格式不同（如显示 en-US、区域格式 zh-CN）的真实场景无法在本测试机修改系统设置复现；解析顺序（显示语言优先、locale 兜底）由 `i18n::tests::system_language_prefers_display_language_then_falls_back` 与 `resolved_tag_for` 单测覆盖。建议 Task 17 人工验收时在混合 locale 机器上确认托盘与面板一致。

## 8. 清理

- 停止测试 app（Quit 菜单 / `Stop-Process` 兜底）与 Vite；确认无 `hangline` 进程残留、无 `tray_icon_app` 残留菜单。
- 还原 `%LOCALAPPDATA%\Hangline\settings.json`（SHA256 与测试前一致：`8E315C7B…A79089`）；删除测试用 `state.json` 与 `state.json.corrupt`；自有测试 PNG 已被“全部取下线”回收；`Screenshots` 目录还原为测试前文件；额外目录 `%TEMP%\hangline-t15\extra-watch` 删除。
- 还原 `%LOCALAPPDATA%\app.hangline.Hangline\logs\` 原日志文件；删除 `%TEMP%\hangline-t15`（exe、CDP/菜单探针脚本、日志/截图副本）。
- Task 15 review fix 复跑使用独立临时目录 `%TEMP%\t15fix`（语言证据 + 截图裁剪），同样删除；应用数据再次还原（settings SHA256 一致、`state.json`/seed PNG 删除）。
- `HKCU\...\Run` 无 Hangline 值（自启测试后已注销）。
- 未提交 `%TEMP%` 产物；提交的仅为 `docs/experiments/P1-tray-settings*.png` 与本文件。

## 9. 关键命令

```powershell
# 质量闸门
cargo test --manifest-path src-tauri/Cargo.toml --lib        # 184 passed / 5 ignored
cargo check --manifest-path src-tauri/Cargo.toml --all-targets   # 0 warnings
pnpm check                                                   # eslint + tsc + 60 vitest + i18n + version

# 运行时（本沙箱等价方式；正常桌面直接 pnpm tauri dev）
pnpm dev
cargo build --manifest-path src-tauri/Cargo.toml
Copy-Item src-tauri\target\debug\hangline.exe $env:TEMP\hangline-t15\hangline.exe
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222 --remote-allow-origins=*'
& $env:TEMP\hangline-t15\hangline.exe
```
