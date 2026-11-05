# P3 标注与录屏：方法、实测与复现

- **任务**：P3（计划 Task P3-1..P3-8）；验收矩阵见 [`docs/acceptance/P3-core.md`](../acceptance/P3-core.md)。
- **产物**：长按标注编辑器、标注原子写回/缩略图刷新、监听目录只读、录屏首帧卡片、WinRT 时间轴裁剪与失败回退；版本 0.3.0，tag `v0.3.0-markup`。
- **结论**：M12/M13 的程序化项 PASS（含 100 次写回压测、真实 WinRT 导出、打包 CSP 预览）；人工体验项 MANUAL-PENDING。

## 1. 方法

- **真实输入**：标注/长按/裁剪 E2E 全部用 CDP `Input.dispatchMouseEvent`/`dispatchKeyEvent` 驱动真实 React 处理链；不是组件单测替代。
- **隔离环境**：`LOCALAPPDATA` 指向 `%TEMP%\hangline-p3-*`；录屏监听通过 **debug-only** `HANG_RECORDINGS_DIR` 指向隔离目录（不覆盖 `USERPROFILE`，避免 tauri-plugin-log 已知文件夹 panic）；真实 `Videos\Captures` 未触碰；fixture 在 app 启动后复制。
- **neutral fixtures**：标注用 1600×1000 合成 PNG；录屏用 ffmpeg 生成的 3 s 640×360 H.264 + 440 Hz AAC mp4（`scripts/p3/fixture-video.ps1`，生成物在 git-ignored `target/`）。
- **WinRT**：`MediaComposition`/`MediaClip` 取首帧与时长；裁剪用 `TrimTimeFromStart`/`TrimTimeFromEnd` + `RenderToFileAsync` 到同目录隐藏临时文件，再原子替换；每次调用在 worker 线程做 MTA 初始化/反初始化。
- **失败注入**：`FileShare.Read` 锁原文件（读允许、删除/重命名拒绝）→ 渲染能开始并产生 `.hangline-trim-*`，替换失败；断言原文件 SHA 不变、临时文件被清理、错误 UI + 回退按钮、解锁后重试成功。
- **打包 CSP**：`pnpm build` 后用临时静态服务器在 5173 提供 `dist` 并发送 `tauri.conf.json` 的精确 CSP；正例要求 `<video>` `readyState>=2` 且无 `media-src` violation；负例使用去掉 `media-src` 的旧 CSP，要求被拦截。
- **隐私**：提交截图来自 `Page.captureScreenshot`（仅 webview），面板透明图合成到中性渐变；未提交含本机路径的 JSON/日志。

## 2. 实测数字（当前提交）

| 项 | 结果 | 来源 |
|---|---|---|
| 写回压测 | 100/100 成功、0 失败、无 `.tmp` | `cargo test`（1047 行附近 `write_back` 循环） |
| 标注保存 | 1600×1000 → 1216×780（裁剪偏移 160×119），`updatedAt`/hash 变化，缩略图 432×277 新字节 | `markup-e2e.json` 31/31 |
| 面板刷新 | `<img src>` 从 `?v=<old>` → `?v=<new>`；state 1 条；飞入层同版本 | 同上 |
| 长按 | 520 ms → 编辑器 **1116 ms**；5 px 移动取消；no-copy/no-open 的 invoke 拦截在该构建不可用（见 P3-core §4） | `longpress-e2e.json` 29/29（`editorOpenLatencyMs`） |
| 只读 | `owned=false`、Save 禁用、Ctrl+S 后原文件 SHA 不变 | 同上 |
| 录屏入库 | **954 ms**；640×360；首帧 432×244/26,517 B；play badge | `recording-e2e.json` 19/19（`ingest.ms`） |
| 裁剪打开 | 长按 → 窗口 772 ms；目标时长 3000 ms | `trim-e2e.json` 65/65 |
| 裁剪导出 | 1000–2500 ms → **1.5333 s**、mov/mp4、video+audio、可播放（44,895→37,196 B） | ffprobe（同上） |
| 裁剪失败 | 渲染后替换失败：原文件 SHA 不变、临时文件清理、错误+回退、重试成功 | 同上 |
| 进度 | `[0.05, 0.6, 1.0]` 单调 | 同上 |
| 打包 CSP | 正例 readyState=4、duration=3、无 media 违规；旧 CSP 负例 `media-src` violation | `trim-csp-e2e.json` 7/7 |
| Rust 裁剪环境测试 | 3 s → 1.0–2.5 s = 1533 ms / 55,252 B；失败路径原文件不变 | `cargo test trim:: -- --ignored` |

## 3. 复现命令（PowerShell 7；隔离目录，勿指向真实资料）

前置：`pnpm install`、debug exe 已构建（`cargo build --manifest-path src-tauri/Cargo.toml`）、`pnpm dev` 在 5173 运行。

```powershell
$base = "$env:TEMP\hangline-p3"
node scripts/p3/fixture.mjs --profile "$base\localappdata\Hangline" --recordings
pwsh -NoProfile -File scripts/p3/fixture-video.ps1 -Out "$base\fixture.mp4"
New-Item -ItemType Directory -Force "$base\userprofile\Videos\Captures" | Out-Null
Copy-Item src-tauri\target\debug\hangline.exe "$base\hangline.exe" -Force

$env:LOCALAPPDATA = "$base\localappdata"
$env:HANG_RECORDINGS_DIR = "$base\userprofile\Videos\Captures"   # debug-only hook
$env:HANG_CDP_PORT = "9225"
$env:HANG_E2E_DIR = $base
$env:HANG_APP_EXE = "$base\hangline.exe"
# 不要设置 USERPROFILE（tauri-plugin-log 会 panic）；也不要触碰真实 Videos\Captures
$app = Start-Process "$base\hangline.exe" -PassThru
Start-Sleep 5
node scripts/p3/trim-e2e.mjs          # 65 checks + 1 MANUAL-PENDING
Stop-Process -Id $app.Id
```

标注/长按/录屏/打包 CSP 的复现分别使用 `scripts/p3/{markup,longpress,recording}-e2e.mjs` 和 `scripts/p3/trim-csp-e2e.mjs`（各文件头有完整 runbook；CSP 脚本会临时占用 5173，先停 `pnpm dev`）。裁剪环境测试：

```powershell
.\scripts\p3\fixture-video.ps1
$env:HANG_TEST_VIDEO = "$PWD\src-tauri\target\p3-fixtures\p3-recording-fixture.mp4"
cargo test --manifest-path src-tauri/Cargo.toml trim:: -- --ignored --nocapture
```

## 4. 证据文件

已提交（只含应用 UI/中性内容）：`P3-annotate.png`、`P3-annotate-crop.png`、`P3-trim.png`、`P3-trim-error.png`、`P3-trim-csp-positive.png`、`P3-panel-before-markup.png`、`P3-panel-after-markup.png`、`P3-panel-after-trim.png`、`P3-recording-card.png`。

git-ignored 原始证据：`.superpowers/p3-evidence/{markup,longpress,recording,trim,trim-csp}-e2e.json`、`recording-backfill.json`、运行日志与截图。这些 JSON 含本机临时路径，**不提交**；发布前可删除整个 `.superpowers/`。

## 5. 已知限制

见 [`P3-core.md` §7](../acceptance/P3-core.md)；核心是 WinRT codec/DRM/旋转未覆盖、输出重编码、进度粗粒度、2 GiB/60 min 运行时未构造真实超大文件、真实光标/Game Bar/播放器/150% 需人工。
