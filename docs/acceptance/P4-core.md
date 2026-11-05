# P4 多屏与设置验收（v0.4.0-settings）

- **范围**：路线图 P4“多屏与设置”：M2 双屏/DPI、M3 任务栏位置、M4 全屏抑制、M14 保留 100 张+滚轮、M15 热键冲突；以及 P4 交付的容量/滚动/尺寸、改键窗口、开机自启、声音。
- **结论**：源码里程碑就绪。程序化验收 PASS（M2/M3 多屏放置/DPI **85/85（干净解锁版）**、M4 全屏抑制 42/42、M14 保留/100 卡 60 fps、M15 热键冲突 59/59、自启 55/55+55/55+15/15）；人工体验项仍为 **MANUAL-PENDING**。
- **版本**：0.4.0；tag `v0.4.0-settings`（P4 源码里程碑，非正式发布）。
- **设计/计划**：[设计文档 §3.2/§3.5/§3.6/§3.9/§4.6/§4.8](../superpowers/specs/2026-10-28-hangline-windows-design.md)、[P4 实现计划](../superpowers/plans/2026-11-04-hangline-windows-p4-multimon-settings.md)、[P4 方法与证据](../experiments/P4-multimon-settings.md)。

## 1. 环境与质量门

Windows 11 Home 10.0.26300 x64；双 1920×1080（`DISPLAY2` 主屏 100%，`DISPLAY1` 左屏 150%），两块屏任务栏均在底部；WebView2 156；Rust 1.90；Node 22.17 / pnpm 10；Tauri 2.12.5；ffmpeg 8.0.1（仅测试 fixture）。

| 门 | 结果（最终提交） |
|---|---|
| `pnpm check` | **PASS**：eslint + tsc + **26 files / 210 tests** + i18n 48 + version 0.4.0 |
| `cargo test` | **PASS**：**341 passed / 0 failed / 8 ignored** |
| `cargo check --all-targets` | **PASS**，0 warnings |
| `cargo check --release --all-targets` | **PASS**，0 warnings |
| `pnpm tauri build --no-bundle` | **PASS**（release exe 见 §6） |
| `pnpm build` release-instrumentation check | **PASS**：dist 中 15 个探针标记 **0/15**，self-test 通过 |
| `git diff --check` / 工作树 | clean |

## 2. M2 / M3：多屏、DPI 与任务栏

**结论：programmatic PASS（干净解锁桌面，`workstationLocked=false`，85/85）**；M2 真实截图与 M3 真实顶部/左侧任务栏为 MANUAL-PENDING。

| 项 | 证据 | 判定 |
|---|---|---|
| 双屏各 100%/150% 下窗口贴工作区、尺寸=dp×scale | `display-e2e` **85/85**（2026-11-04 15:0x，解锁桌面）：100% 屏 (0,0) 1920×210 dpi96 / CSS 1920×210；150% 屏 (−1920,0) 1920×315 dpi144 / CSS 1280×210；主题切换不变 | **PASS（程序化）** |
| 卡片物理位置/不越界 | 100% 屏卡片 (798,30)150×99、(972,30)；150% 屏 (−1203,35)225×147、(−942,35)；slot 公式误差 ≤0.011 CSS px（实测最大 |dy| 0.010076）；卡+删除热区在面板内 | **PASS（程序化）** |
| app `get_monitors` 与独立 Win32 枚举一致 | id/rect/work area/scale 全等 | **PASS** |
| 任务栏顶部/左侧、125%、显式移除显示器 | Rust 单测：work-area 原点/尺寸、负原点、125/150%、signature 变化、面板所在屏消失迁移 | **PASS（单测）** |
| 真实双屏各截一张图（M2） | — | **MANUAL-PENDING** |
| 真实任务栏移到顶部/左侧（M3） | — | **MANUAL-PENDING** |
| 干净解锁桌面运行 | 2026-11-04 15:0x 解锁桌面 `workstationLocked=false` 的 85/85 PASS 证据已保留；此前锁屏 run 亦为 85/85 | **PASS（程序化）** |

## 3. M4：全屏抑制

| 项 | 证据 | 判定 |
|---|---|---|
| 全屏应用时线自动隐藏 | `fullscreen-e2e` **42/42**（解锁）：tuck 643 ms、窗口隐藏 995 ms；日志 reason/quns=2/foreground_covers=true | **PASS（程序化）** |
| 全屏期间新截图不弹出 | 新条目入库（webview 2 卡）但面板保持隐藏 | **PASS（程序化）** |
| 退出全屏 ≤1 s 恢复 | 恢复 **779 ms**（精确重跑 835/838 ms），poller 100 ms + 稳定 800 ms，最坏 900 ms | **PASS（程序化）** |
| `showOverFullScreen` + pinned 覆盖 | 覆盖 110/82 ms；pinned toggle 124 ms | **PASS（程序化）** |
| 普通最大化窗口不误判 | 有边框最大化（主/副屏）、有边框精确覆盖副屏、无边框最大化覆盖副屏均不触发；无 `WS_CAPTION/WS_THICKFRAME` 且非 `SW_SHOWMAXIMIZED` 才算全屏 | **PASS（程序化）** |
| 截图覆盖层恢复时重新判定 | 捕获期间保留 gate + 1 s 控制器 recheck；单测矩阵 | **PASS（单测）** |
| 真实 D3D 独占游戏/演示模式/DRM/Space-AltTab、覆盖层叠在游戏上 | — | **MANUAL-PENDING** |

## 4. M14：保留 100 张 + 滚轮

| 项 | 证据 | 判定 |
|---|---|---|
| 保留上限为硬状态上限（25/50/100），最旧先下线，`Fit` 无上限 | 6 个 Rust 单测：101 张→100、降档立即裁剪、启动强制、持久化/rev+1、文件不删、顺序/ID 不变 | **PASS（程序化）** |
| 100 张滚轮不卡顿、最新在右 | `perf-100` 新版 harness **PASS**：302 样本、mean **60.00 fps**、p5 59.52、worst **17.0 ms**、长帧 **0**、120 次真实移动（四区间 29/30/30/31）；滚回最新在右 | **PASS（程序化）** |
| 降到 25：状态 25、75 个文件仍在、revision +1 | E2E：100→25、revision 1→2、75/75 文件存在、两个 JSON 持久化、面板仍可滚动 | **PASS（程序化）** |
| 真实触控板手感 | — | **MANUAL-PENDING** |

## 5. M15 与设置交付

| 项 | 证据 | 判定 |
|---|---|---|
| 真实热键冲突：提示、回滚、可改键 | `hotkey-conflict-e2e` **59/59**：启动冲突保留设置/捕获仍注册；冲突修改 105 ms 拒绝且旧键仍可用、settings 不变；释放后重试 42 ms 成功、旧键释放/新键唯一 | **PASS（程序化）** |
| 改键窗口显示当前键/冲突/保存 | CDP 驱动真实 shortcut 页面：显示 Ctrl+Alt+S、捕获 Ctrl+Alt+Shift+F5、行内冲突、保存后更新 toggle 页 Ctrl+Alt+T | **PASS（程序化）** |
| 开机自启（HKCU Run、带引号、空格路径、reconcile、Task Manager 禁用） | `autostart-e2e` 55/55 + spaced 55/55 + HKCU 非提权 15/15；`"<exe>" <args>` 经 `CommandLineToArgvW` 校验；stale 路径修复；StartupApproved 0x03 保持禁用；registry 快照恢复、独立 reg query 无残留 | **PASS（程序化）** |
| 声音开关 | 纯 `should_play` 门 + `sound=false` 跳过日志 + `sound=true` played 日志；无 WAV，`MessageBeep` 设计 | **PASS（门控程序化）**；可听声音 **MANUAL-PENDING** |
| 尺寸小/中/大、滚动、改键窗口/自启/声音的托盘入口 | Rust/前端单测 + CDP shortcut 页面 + tray 单测；真实托盘点击 **MANUAL-PENDING** | **PASS（程序化）** / 托盘人工 |
| 真实托盘点击、真实物理热键、注销/重启自启、460×240 窗口焦点 | — | **MANUAL-PENDING** |

## 6. 发布卫生与 0.4.0 构建

- 版本 0.4.0（package/tauri/Cargo/Cargo.lock）；tag `v0.4.0-settings`。
- `pnpm tauri build --no-bundle` → `src-tauri/target/release/hangline.exe`（版本 0.4.0，**20,358,656 bytes**，FileVersion 0.4.0，2026-11-04 构建）。
- release bundle 不含调试探针：15 标记 0/15；`HANG_CDP_PORT`/`HANG_RECORDINGS_DIR` 等 debug 钩子编译剔除。
- 无安装包/签名/自动更新（P5）。

## 7. 人工验收清单（用户执行）

> 全部 MANUAL-PENDING；请在普通交互桌面、真实鼠标/键盘下完成并回报。

1. 双屏（100%+150%）各截一张图：截图出现在对应屏幕、卡片无错位；150% 屏卡片/夹子/文字清晰。
2. 把任务栏移到屏幕顶部或左侧：线贴住工作区顶、不被任务栏遮挡；重启后仍正确。
3. 打开全屏游戏/视频演示：线 1 s 内隐藏且不被盖住；退出 1 s 内恢复；托盘勾「全屏应用上」+ 手动唤出时线保持。
4. 保留数量 25/50/100 后连续截图：线只保留最新 N 张，旧卡片下线但**文件仍在截图目录**；滚轮翻阅不卡、最新在右。
5. 先启动占用 `Ctrl+Alt+T` 的程序（如 QQ）再启动/改键：提示冲突、旧键仍可用、可重试改键；释放后成功。
6. 托盘勾「开机启动」→ 注销/重启自动启动；关闭后不自启；任务管理器禁用启动项后不被应用强行恢复。
7. 声音开/关：复制/取下线提示音符合设置（`MessageBeep`）。
8. 改尺寸小/中/大、亮/暗、换分辨率/缩放：面板与卡片无裁剪、无抖动；改键窗口（460×240）可聚焦。
9. 拔掉/扩展显示器或改主屏：线迁移、卡片不丢；任务栏顶部/左侧布局正确。
10. 退出重启：保留数量/布局/自启/声音/热键设置按预期恢复；P3 标注/裁剪后的文件与缩略图仍正确。

## 8. 已知限制与 backlog

- **display E2E**：最终证据为解锁桌面 85/85 PASS（`workstationLocked=false`）；运行期间会移动真实指针约 20 秒，需在用户空闲时执行。
- **全屏**：仅用合成无边框/覆盖窗口验证 foreground/QUNS 分支；真实 D3D 独占、演示模式、DRM、Space/Alt+Tab 交接、覆盖层叠在真实游戏上未验证；启动时处于全屏的“愿望恢复”只做单测。
- **保留语义与规格偏差**：25/50/100 现在硬性裁剪状态（最旧下线、文件不删）；规格 §4.8 提到“超出容量删除自有文件（可配置保留文件）”，V1 不启用该删除路径（无 keepFiles 设置），避免静默删文件。
- **M14 环境噪声**：同一 harness 在系统负载高的首次运行 fps 不达标（55 fps），干净重跑 60.00；测量对共享桌面负载敏感。
- **自启**：`auto-launch` 插件原值无引号，已由 `autostart.rs` 规范化为 `"<exe>" <args>`；exe 路径含空格/移动后 stale 值可修复；Task Manager 禁用状态（StartupApproved 0x03）保持禁用并记录日志，不覆盖。
- **声音**：`MessageBeep` 系统提示音，无 WAV 资产；可听效果人工。
- **冲突提示**：启动时热键冲突不使用气泡通知，而是 shortcut 窗口行内错误 + 托盘提示行（规格 §3.11 的偏差，记录在此）。
- **性能**：100 张启动 backfill 现在会对每个新鲜缩略图做解码自愈检查（100 张约 13 s），已记录；后续可加“仅在可疑文件时解码”。
- **调试钩子**：`HANG_CDP_PORT`、`HANG_RECORDINGS_DIR`、perf/trace 探针均 debug-only，release 编译/构建检查剔除。
- **改键窗口截图**：提交版暂无真实 460×240 shortcut 窗口截图（早先误用的 1920×210 图已移除），后续补拍；功能证据见 hotkey E2E 59/59（git-ignored JSON）。

## 9. 证据文件

- 本文档、[P4 方法与证据](../experiments/P4-multimon-settings.md)、[P4 参考/计划](../superpowers/plans/2026-11-04-hangline-windows-p4-multimon-settings.md)。
- 已提交截图（仅应用 UI/中性合成）：`P4-panel-100pct.png`、`P4-panel-150pct.png`。
- 原始 JSON/日志/面板截图在 git-ignored `.superpowers/p4-evidence/`（含本机临时路径，未提交）。
