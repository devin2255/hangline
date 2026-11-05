# P1 目录监听实验报告：系统截图 / 录屏 / 额外目录 + 哈希去重

- **任务**：Task 8「目录监听与去重」
- **分支 / 基线**：`feat/core`，父提交 `5c2f99f`
- **日期**：2026-10-29
- **结论**：4 个监听目录全部启动；PNG 落盘后 ~0.3 s 内上栏并广播 `state`（及 `reveal`）事件；内容重复、同路径重复、非候选文件均被拦下；额外目录 / 自建目录 / 录屏目录的 `kind` 分别映射为 `watchedFolder` / `capture` / `recording`。程序化判据全过，真实系统工具的时序类人工项见 §6。

## 1. 目的与范围

验证 Task 8 的实现（`src-tauri/src/watchers/` + `src-tauri/src/dedupe.rs`）在真实 Tauri 进程中的行为：

1. 启动时按 `settings` 监听 `%USERPROFILE%\Pictures\Screenshots`、`%USERPROFILE%\Videos\Captures`、`%LOCALAPPDATA%\Hangline\Screenshots` 与 `extraFolders`（≤3）；
2. 新文件经过 200 ms 防抖、扩展名/隐藏/临时文件过滤、BLAKE3(64bit) 哈希与 20 条 LRU 去重后写入 `AppState`；
3. 每次新增广播全量 `state` 事件，并在 `revealOnNew=true` 时广播 `reveal` 事件；
4. 重复内容 / 已在线路径 / 非图片文件不重复上栏；
5. 目录缺失或不可写只记日志，不 panic、不创建用户目录。

## 2. 环境

| 项 | 值 |
| --- | --- |
| OS | Microsoft Windows 11 家庭版 中文版，10.0.26300 |
| Rust | rustc 1.90.0 / cargo 1.90.0 |
| App | Hangline 0.1.0（debug，`cargo build`） |
| 依赖 | `notify 8.2.1`、`blake3 1.8.7`、`image 0.25.10`、`tauri 2.x` |
| 前端 | Vite 7.3.7 dev server（`http://localhost:5173`） |
| 观测 | `%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log` + WebView2 CDP（`--remote-debugging-port=9222`） |

**运行方式（沿用 Task 2/5/7 的沙箱 workaround）**：workspace 内的 exe 在初始化 `tauri-plugin-log` 时写 `%LOCALAPPDATA%` 被环境 ACL 拒绝（`os error 5`），因此：

1. `pnpm dev` 启动 Vite；
2. `cargo build --manifest-path src-tauri/Cargo.toml`，把 `src-tauri/target/debug/hangline.exe` 复制到 `%TEMP%\hangline-task8\hangline.exe`（同一 dev 构建、同一 `devUrl`）；
3. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*` 启动该副本；
4. CDP `Runtime.evaluate` 注入 `state` / `reveal` 监听器并读取 `get_app_state`。

为验证 `extraFolders`，本轮启动前把 `settings.json` 的 `extraFolders` 指向 `%TEMP%\hangline-task8\watched`；测试结束后恢复原文件（字节级一致）。

## 3. 预期 vs 观察

| # | 操作 | 预期 | 观察 | 证据 |
| --- | --- | --- | --- | --- |
| 1 | 启动（两个 watch 开关默认 true，1 个额外目录） | 4 个 watcher | `4 folder watcher(s) active`，4 条 `watching ...` | §4.1 |
| 2 | 复制 80×60 PNG 到 `图片\Screenshots` | 新增 `systemScreenshot` 80×60，`state` 事件 | 通过；`stateEvents=8` 中的第 1 条含该 item | §4.2、§4.4 |
| 3 | 新 item 且 `revealOnNew=true` | 广播 `reveal {"reason": ...}` | 收到 `{reason:"newItem"}`，共 5 次（= 前 5 个新 item） | §4.2 |
| 4 | 同内容复制到新文件名 | 哈希去重，不上栏、无事件 | `dedupe hit ... (hash 092bd6ea36e31aaa)`，事件数不变 | §4.4 |
| 5 | 同路径覆盖为不同内容 | 路径已在线，不上栏、无事件 | `ignored ... (already hung on the line)`，事件数不变 | §4.4 |
| 6 | 复制 `.txt` | 候选过滤，不上栏 | 无 watcher 日志、无事件 | §4.4 |
| 7 | 复制第二张不同 PNG | 新增 | `systemScreenshot` 40×30 | §4.4 |
| 8 | 复制 PNG 到额外目录 | 新增 `watchedFolder` 24×18 | 通过 | §4.4 |
| 9 | 复制 PNG 到自建截图目录 | 新增 `capture` 32×24 | 通过 | §4.4 |
| 10 | 复制 `.mp4` 到 `视频\Captures` | 新增 `recording`，宽高 0×0 | 通过（`0x0`） | §4.4 |
| 11 | 复制 `.mp4` 到 `图片\Screenshots` | 录屏只在录屏目录接受 | 被忽略，无事件、无 item | §4.2 |
| 12 | `set_setting revealOnNew=false` 后复制新 PNG | 上栏继续，但无 `reveal` | item 6 出现；`stateEvents` +2（两次 set_setting），`revealEvents` 仍 5 | §4.2 |
| 13 | 目录缺失 | 启动跳过、不 panic、不创建目录 | 单元测试 `missing_folder_is_an_error_not_a_panic` 通过；E2E 中未创建任何缺失目录 | `cargo test` |
| 14 | 一次写入触发多个 OS 事件 | 防抖合并为一次回调 | 临时目录集成测试：5 次连续写入仅 1 次回调，600 ms 内无第二次 | `cargo test watchers::dirs::tests` |

**最终事件计数**：`stateEvents=8` = 6 个新 item + 2 次 `set_setting`（关闭/恢复 `revealOnNew`）；`revealEvents=5` = 前 5 个新 item；`lastStatePaths` 恰好 6 条，无重复、无 `.txt`、无 `图片` 目录里的 `.mp4`。

## 4. 证据

### 4.1 启动：4 个 watcher

```text
[INFO][hangline_lib::watchers::dirs] watchers: watching %USERPROFILE%\Pictures\Screenshots (Opts { kind: SystemScreenshot, accept_recordings: false })
[INFO][hangline_lib::watchers::dirs] watchers: watching %USERPROFILE%\Videos\Captures (Opts { kind: SystemScreenshot, accept_recordings: true })
[INFO][hangline_lib::watchers::dirs] watchers: watching %USERPROFILE%\AppData\Local\Hangline\Screenshots (Opts { kind: Capture, accept_recordings: false })
[INFO][hangline_lib::watchers::dirs] watchers: watching %USERPROFILE%\AppData\Local\Temp\hangline-task8\watched (Opts { kind: WatchedFolder, accept_recordings: false })
[INFO][hangline_lib::watchers] watchers: 4 folder watcher(s) active
```

### 4.2 CDP 最终快照（真实页面上下文）

```json
{
  "stateEvents": 8,
  "revealEvents": [
    { "reason": "newItem" },
    { "reason": "newItem" },
    { "reason": "newItem" },
    { "reason": "newItem" },
    { "reason": "newItem" }
  ],
  "lastStatePaths": [
    "%USERPROFILE%\\Pictures\\Screenshots\\hangline-e2e-a.png",
    "%USERPROFILE%\\Pictures\\Screenshots\\hangline-e2e-b.png",
    "%USERPROFILE%\\AppData\\Local\\Temp\\hangline-task8\\watched\\hangline-e2e-c.png",
    "%USERPROFILE%\\AppData\\Local\\Hangline\\Screenshots\\hangline-e2e-d.png",
    "%USERPROFILE%\\Videos\\Captures\\hangline-e2e-rec.mp4",
    "%USERPROFILE%\\Pictures\\Screenshots\\hangline-e2e-e.png"
  ]
}
```

> 注：CDP 以 JSON number 传输 `u64` 哈希，超过 2^53 会在 JS 侧丢精度，因此报告不引用 JS 的 `hash` 数值；精确值以 `state.json` 与日志十六进制为准（例如 `092bd6ea36e31aaa`）。

### 4.3 落盘的 `state.json`（6 条）

| path | kind | width×height | hash (u64, 十进制) |
| --- | --- | --- | --- |
| `...\Pictures\Screenshots\hangline-e2e-a.png` | systemScreenshot | 80×60 | 660858071771454122 |
| `...\Pictures\Screenshots\hangline-e2e-b.png` | systemScreenshot | 40×30 | 6282401660988911213 |
| `...\Temp\hangline-task8\watched\hangline-e2e-c.png` | watchedFolder | 24×18 | 1730121206174265540 |
| `...\AppData\Local\Hangline\Screenshots\hangline-e2e-d.png` | capture | 32×24 | 2878490259100500397 |
| `...\Videos\Captures\hangline-e2e-rec.mp4` | recording | 0×0 | 799094311167283054 |
| `...\Pictures\Screenshots\hangline-e2e-e.png` | systemScreenshot | 16×12 | 7992008856342364958 |

### 4.4 watcher 日志逐条（同一进程）

```text
[INFO][hangline_lib::watchers::dirs] watchers: hung ...\hangline-e2e-a.png (SystemScreenshot) 80x60 hash 092bd6ea36e31aaa
[INFO][hangline_lib::watchers::dirs] watchers: dedupe hit for ...\hangline-e2e-a-copy.png (hash 092bd6ea36e31aaa)
[INFO][hangline_lib::watchers::dirs] watchers: ignored ...\hangline-e2e-a.png (already hung on the line)
[INFO][hangline_lib::watchers::dirs] watchers: hung ...\hangline-e2e-b.png (SystemScreenshot) 40x30 hash 572f93066b916e6d
[INFO][hangline_lib::watchers::dirs] watchers: hung ...\watched\hangline-e2e-c.png (WatchedFolder) 24x18 hash 1802a012086330c4
[INFO][hangline_lib::watchers::dirs] watchers: hung ...\Hangline\Screenshots\hangline-e2e-d.png (Capture) 32x24 hash 27f273a3c71b81ad
[INFO][hangline_lib::watchers::dirs] watchers: hung ...\Videos\Captures\hangline-e2e-rec.mp4 (Recording) 0x0 hash 0b16f40acf294f6e
[INFO][hangline_lib::watchers::dirs] watchers: hung ...\hangline-e2e-e.png (SystemScreenshot) 16x12 hash 6ee951b61c10031e
```

`.txt`、`图片` 目录里的 `.mp4`、同内容副本与同路径覆盖都没有 `hung` 行；`dedupe hit` / `already hung` 分别对应哈希去重与路径去重。`notify::windows` 的 TRACE 行只作为“事件确实到达”的辅助，不作为产品日志。

### 4.5 自动化测试（临时目录集成测试，无 `#[ignore]`）

`cargo test --manifest-path src-tauri/Cargo.toml` → **74 passed, 0 failed, 5 ignored**（5 个 ignored 来自既有 Task 5 的 E5 环境测试）。其中 Task 8 新增 12 个：

- `dedupe`（4）：首次/重复、LRU 淘汰与 recency、`remember` 认领、BLAKE3 64bit 内容寻址 + 文件缺失报错；
- `watchers::dirs`（6）：候选规则表 21 例、kind 映射、临时目录 PNG 2 s 内回调、`.txt`/`.tmp` 忽略且 watcher 仍存活、5 次连续写入只回调 1 次、缺失目录返回错误且不创建；
- `watchers`（1）：`%USERPROFILE%\Pictures\Screenshots` 路径拼装；
- `app_state`（1）：`contains_path` 大小写不敏感、只匹配在线 item。

集成测试直接使用真实 `notify`（Windows `ReadDirectoryChangesW`）+ 真实临时目录 + 真实 1×1 PNG；没有 mock 文件系统。

## 5. 关键命令

```powershell
cargo test  --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml     # 0 warnings
cargo build --manifest-path src-tauri/Cargo.toml
pnpm dev                                             # Vite :5173
# 复制 target/debug/hangline.exe 到 %TEMP%\hangline-task8 后：
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222 --remote-allow-origins=*'
Start-Process .\hangline.exe
# CDP: Runtime.evaluate 注册 plugin:event|listen('state'/'reveal')，再复制 PNG 到被监听目录
```

## 6. 手动待验证（P0/P1 真机条件，agent 无法在解锁桌面上完成）

1. **真实系统截图链路**：按 `Win+PrtScn`，或用「截图工具」开启自动保存后截一张图，确认真实工具的分块写入时序下 200 ms 防抖仍只产生一条卡片（本轮用资源管理器复制真实 PNG 模拟落盘）。
2. **真实 Xbox Game Bar 录屏**：`Win+Alt+R` 录制/停止，确认真实 MP4 首帧、时长与播放角标（本轮用 512 B 伪 MP4 验证目录/扩展名/`recording`/0×0；首帧解码属 Task 10/P3）。
3. **面板视觉**：当前前端仍是 Task 7 的占位（只显示数量），卡片渲染与「飞入」属 Task 10/11；本轮以 CDP 的 store/事件数据为判据。
4. **展开动画**：`reveal` 事件已被真实广播，但消费方状态机与动画在 Task 11；需在 Task 11 验收动画。
5. **托盘退出时的优雅停机**：agent 会话无法点击托盘菜单；`DirWatcher::drop` 的 join 行为由全部集成测试（每条测试都 drop watcher）覆盖，真实桌面「Quit」路径待 Task 15/17 人工确认。
6. **权限拒绝目录**：缺失目录路径已由单测覆盖；「存在但拒绝访问」的真实 ACL 场景需普通用户账户手工确认。

## 7. 清理与恢复

- 删除本轮创建的 6 个测试文件（`图片\Screenshots`、`视频\Captures`、`Hangline\Screenshots`、临时 watched 目录）；
- `settings.json` 恢复为测试前原文件（字节级一致，`extraFolders` 回到 `[]`）；
- 删除测试产生的 `state.json`（测试前不存在）；
- 恢复测试前的 `Hangline.log`；停止 app 与 Vite；删除 `%TEMP%\hangline-task8`；
- 测试文件仅写入上述用户目录与 `%TEMP%`，未触碰桌面/下载等其它目录。

## 8. 结论与偏差

- 4 条监听链路、候选过滤、防抖、哈希/路径双层去重、`state`/`reveal` 事件与 `kind`/尺寸映射都在真实进程中成立；`cargo test`、`cargo check`、`pnpm check` 全绿。
- **偏差 1（`reveal` 事件）**：Task 8 按 plan Step 3 广播 `reveal {reason:"newItem"}`（仅 `revealOnNew=true`），但不实现展开状态机/动画（Task 11 负责）。payload 是 `{reason}` 的最小契约，Task 11 如需要更细的 reason 可扩展。
- **偏差 2（自建认领）**：spec §3.2 规则 2 的「5 s 待认领哈希」在 `Deduper::remember` 中实现为普通 LRU 条目（无 TTL）；自建截图的持久防重由「路径已在 AppState」兜底，Task 16 需在存盘前调用 `remember(hash)`。
- **偏差 3（运行时配置）**：watch 开关与 `extraFolders` 只在启动时读取；Task 15 若允许在托盘修改，需要重启 watcher（本任务未实现重启 API）。
- **偏差 4（HEIC）**：`image` crate 无 HEIC 解码器，`.heic` 文件会以 0×0 上栏（扩展名白名单要求接受它）；PNG/JPEG/GIF/WebP/TIFF 正常读尺寸。
- 未新增依赖；未修改 docs/superpowers 下的 spec/plan。
