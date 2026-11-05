# P3 标注与录屏验收（v0.3.0-markup）

- **范围**：P3（计划 Task P3-1..P3-8）：长按标注编辑器（画笔/矩形/箭头/文字/裁剪/撤销）、原子写回 + 缩略图/面板刷新、监听目录只读保护；Game Bar 录屏首帧卡片；时间轴裁剪（WinRT `MediaComposition` 导出、原子替换、失败回退）；M12/M13。
- **结论**：**源码里程碑就绪**。自动化验收 PASS（含 100 次写回压测、真实 WinRT 裁剪与打包 CSP 预览），**M12/M13 的人工体验项仍为 MANUAL-PENDING**；没有安装包/发布。
- **版本**：0.3.0；tag `v0.3.0-markup`（P3 源码里程碑，非正式发布）。
- **设计/计划**：[设计文档 §3.7/§3.8](../superpowers/specs/2026-10-28-hangline-windows-design.md)、[P3 实现计划](../superpowers/plans/2026-11-03-hangline-windows-markup-record.md)。

## 1. 环境

Windows 11 26H2 x64；双 1920×1080（100% + 150%）；WebView2 156；Rust 1.90 MSVC；Node 22.17 / pnpm 10；Tauri 2.12.5；ffmpeg/ffprobe 8.0.1（仅测试 fixture）。所有 E2E 使用隔离的数据目录；录屏/裁剪 E2E 另用 **debug-only** `HANG_RECORDINGS_DIR`（release 编译剔除）重定向监听目录，标注/长按 E2E 仅隔离 `LOCALAPPDATA`。真实 `Videos\Captures` 与用户桌面未被触碰。

## 2. 质量门（最终提交复跑）

| 门 | 结果 |
|---|---|
| `pnpm check` | **PASS**：eslint + `tsc --noEmit` + **25 files / 195 tests** + i18n 48 keys + version 0.3.0 |
| `cargo test` | **PASS**：**286 passed / 0 failed / 8 ignored**（ignored 为需要真实 mp4/环境的 WinRT 测试） |
| `cargo check --all-targets` | **PASS**，0 warnings |
| `cargo check --release --all-targets` | **PASS**，0 warnings |
| `pnpm tauri build --no-bundle` | **PASS**（release exe 见 §6） |
| `git diff --check` / 工作树 | clean |

## 3. 自动化证据矩阵

| 证据 | 结果 |
|---|---|
| 100 次写回压测（Rust `save_annotation` 写回循环） | **100/100 成功，0 失败**；无 `.tmp` 残留；原子替换后文件可解码、hash=字节 |
| 标注 E2E（CDP trusted 输入，隔离 profile） | **31/31 PASS**：编辑器打开→画笔/矩形/文字→裁剪→Ctrl+S；原文件尺寸 1600×1000→1216×780，`updatedAt`/hash 变化，缩略图 432×277 字节更新，面板 `<img src>` 切到 `?v=<新>`，state 1 条无重复，无临时文件，编辑器关闭 |
| 长按/只读 E2E | **29/29 PASS**：520 ms 长按 **1116 ms** 内打开编辑器（`editorOpenLatencyMs`）；5 px 移动取消；监听目录项 `owned=false`、Save 禁用、Ctrl+S 不写文件（SHA 不变）；编辑器打开期间线保持展开 busy；Esc/关闭脏确认；单窗口复用/重定向/脏拒绝。注：该构建 `__TAURI_INTERNALS__.invoke` 不可写，“不复制/不打开”的 invoke 拦截检查无效，相关行为仍需人工确认；拖拽未在该 E2E 断言 |
| 录屏首帧 E2E | **19/19 PASS**：3 s mp4（H.264+AAC）入库 **954 ms**（`ingest.ms`），640×360，首帧 432×244/26,517 B，卡片首帧+播放角标，无重复/无 `.tmp`；启动 backfill 自愈 0×0/缺预览（状态证据） |
| 裁剪 E2E | **65/65 PASS + 1 MANUAL-PENDING**（其中 3 项依赖 `__TAURI_INTERNALS__.invoke` 拦截的 recorder 检查在该构建无效，实质为 62 项 + 1 项 open_item 人工）：入库 1181 ms，长按→裁剪窗口 772 ms，时长 3000 ms，选 1000–2500 ms，保存 2269 ms；ffprobe **1.5333 s**、mov/mp4、video+audio、可播放；hash/updatedAt/预览更新，面板 1 条无重复；进度 0.05/0.6/1；Esc/关闭/重定向/导出中拒绝关闭均验证 |
| 裁剪失败回退 | PASS：`FileShare.Read` 锁（允许读、拒绝删除）使**渲染开始后**原子替换失败——观察到 `.hangline-trim-*` 临时文件、错误面板+“用默认播放器打开”、原文件 SHA-256 不变、临时文件清理、解锁后重试成功 |
| 打包 CSP 预览 | **7/7 PASS**：用构建后的 `dist` + 精确配置 CSP 响应头运行，`<video>` `readyState=4` 且无 `media-src` 违规；**旧 CSP 负对照**被拦截（`media-src` violation），证明该修复必要。注：静态服务器不复制 Tauri 的 CSP 注入，正例还会记录 `connect-src http://ipc.localhost` 违规（与 `<video>` 无关） |
| 裁剪单元/环境测试 | `cargo test trim:: -- --ignored`：3 s 源 1.0–2.5 s → 1533 ms / 55,252 B，失败路径原文件字节不变；`video::` ignored 2/2；2 GiB/60 min 精确边界单测 |

## 4. M12 / M13 判定

### M12 标注保存

| 检查 | 证据 | 判定 |
|---|---|---|
| 长按 0.45 s 打开标注器 | 长按 E2E 29/29（1116 ms；按下 0.95 反馈由 P2 press 单测覆盖） | **PASS（程序化）** |
| 画笔 3 粗细/6 色、矩形、箭头、多行文字、裁剪 | annotation 19 tests + 标注 E2E | **PASS（程序化）** |
| 撤销/重做 ≥20 步；Ctrl+Z/Y/S、Esc 脏确认 | 单测 + E2E（含裁剪撤销） | **PASS（程序化）** |
| 保存 → 原文件原子替换、缩略图立即刷新、面板刷新 | 31/31 E2E、100/100 压测 | **PASS（程序化）** |
| 监听目录只读、保存失败原图不变 | 只读 E2E + 失败单测 | **PASS（程序化）** |
| 真实鼠标/150% DPI/HEIC/原生确认框手感 | — | **MANUAL-PENDING** |

### M13 录屏录入与裁剪

| 检查 | 证据 | 判定 |
|---|---|---|
| Game Bar 目录新 mp4 上栏、首帧+播放角标 | 首帧 E2E 19/19 | **PASS（程序化）** |
| 单击复制文件（CF_HDROP）/双击默认程序打开 | Rust `clipboard_io::recording_only_ships_file_drop` + P1 的 `open_item` 路径；本 E2E 未直接驱动 | **PASS（既有单测/P1 路径）** |
| 长按进入裁剪，双滑块从头到尾，时长正确 | 裁剪 E2E 65/65 | **PASS（程序化）** |
| 导出时长正确（1.5 s±0.2）、可播放、音频保留 | ffprobe 1.5333 s + audio stream | **PASS（程序化）** |
| 原子替换、失败回退原文件不变、无临时残留、可重试 | 渲染后失败注入 PASS | **PASS（程序化）** |
| >2 GiB / >60 min 拒绝 | 单测精确边界；运行时未构造真实超大文件 | **程序化边界；真实超大文件 MANUAL-PENDING** |
| 真实 Game Bar 录屏（写入中/HEVC/DRM/旋转元数据）、真实播放器、150% 副屏 | — | **MANUAL-PENDING** |
| “用默认播放器打开”确实执行 `open_item` | 自动化无法拦截不可写的 `__TAURI_INTERNALS__.invoke`；按钮可见/可用/不关窗已验证 | **MANUAL-PENDING** |

## 5. 人工验收清单（用户执行）

> **用户人工验收：2026-11-04，用户回报“跑完了 没问题”**（用户口述；清单 1–11 视为用户确认通过。自动化数据仍以 §3 的证据为准，本节不回溯修改任何程序化结果）。
>
> 原始说明：清单项在自动化阶段为 MANUAL-PENDING；请在普通交互桌面、真实鼠标/键盘下完成并回报。截图/录屏请只截 Hangline UI 或中性内容。

1. 长按自有截图卡片 0.45 s：标注编辑器打开；松手不应触发复制/双击/拖拽。
2. 依次画画笔（3 粗细/6 色）、矩形、箭头、多行文字；Ctrl+Z / Ctrl+Y 撤销重做；Esc 有改动时二次确认。
3. 裁剪工具框选后 Ctrl+S：编辑器关闭，卡片立即显示新图、缩略图同步更新（无需重启）。
4. 长按 **监听目录**（额外目录）图片：编辑器只读、保存禁用并提示。
5. 制造保存失败（如把原图设为只读）：提示“保存失败”、原图不变；恢复后重试成功。
6. 用 `Win+Alt+R` 录制几秒 → 卡片显示首帧 + 播放角标；单击复制文件、双击用默认播放器打开。
7. 长按录屏卡片 → 裁剪窗口；拖动起止滑杆到约 1/3–2/3，保存；用播放器确认时长正确、可播放、有声音。
8. 裁剪中锁定原文件或拔掉磁盘 → 错误面板 + “用默认播放器打开”；点击应打开播放器；原文件不变。
9. 超过 2 GiB 或 60 分钟的录屏：裁剪窗口提示不支持并禁用保存。
10. 150% 副屏 + 亮/暗主题：编辑器/裁剪窗口布局、清晰度、视频预览正常（打包版 exe，不是 dev 页面）。
11. 取消/关闭编辑器或裁剪窗口后，线恢复正常交互，无残留窗口；导出中按 Alt+F4 应被阻止。

## 6. 构建产物

`pnpm tauri build --no-bundle` → `src-tauri/target/release/hangline.exe`（版本 0.3.0）。无安装包/签名/自动更新（P5）。

## 7. 已知限制与 backlog

- **WinRT 覆盖**：仅验证 H.264/yuv420p 合成 mp4；HEVC/AV1/DRM/旋转元数据、真实 Game Bar 写入中的文件未验证；失败时安全回退（首帧 None / 裁剪报错原文件不动）。
- **裁剪输出是重编码**：`MediaComposition.RenderToFileAsync` 使用媒体运行时默认 `MediaEncodingProfile`（保持分辨率、保留音频轨；大小可变）。E2E fixture：44,895 B → **37,196 B**（变小）；ignored Rust 测试：5,615 B → **55,252 B**（变大）。V1 不使用 passthrough。
- **首帧 1 px 舍入**：432 请求实得 432×244（0.4% 宽高比误差），卡片布局使用自然 640×360。
- **进度是粗粒度**：保证 0.05/0.6/1.0 里程碑；WinRT 不提供细粒度回调。
- **调试钩子**：`HANG_RECORDINGS_DIR` 仅 debug 构建生效（隔离 E2E 用；release 忽略）。本机曾观察到只覆盖 `USERPROFILE`/`LOCALAPPDATA` 时 tauri-plugin-log 的已知文件夹解析导致启动 panic（不同 APP DATA 组合可能不同），为避免环境耦合，隔离 E2E 使用该显式钩子而不是覆盖 profile。
- **编辑器视觉**：窗口为深色自绘标题栏，但工具栏/文本输入跟随亮/暗主题（规格 §3.7 写“深色窗口”）；如需纯深色可在 P4/P5 调整。
- **长按是唯一入口**：右键“标记/裁剪”菜单未实现（P1 规格的右键菜单属于 V1.1 backlog）；4 dp 阈值在 `longPress.ts` 与 `dragThreshold.ts` 各有一份（有注释，改动需同步）。
- **自动化限制**：真实光标 hittest、真实 Game Bar、真实播放器、原生确认框、150%/混合 DPI 窗口放置、剪贴板读取（本会话 Windows 剪贴板访问被拒）仍需人工；`open_item` 回退按钮的命令拦截见 §4。
- **既有 P1/P2 backlog 仍有效**：真实桌面 capture overlay 证据（隐私）、HEIC/EXIF/GIF 保真（P3/P4）、大文件缩略图缓存 URL 哈希、P2 已知 render 边角等。

## 8. 证据文件

- 本文档、[P3 实验报告](../experiments/P3-markup-record.md)、[P3 参考数据](../experiments/P3-reference.md)。
- 已提交截图（均只含 Hangline UI 或中性合成 fixture；面板透明截图合成到中性渐变上）：`P3-annotate.png`、`P3-annotate-crop.png`、`P3-trim.png`、`P3-trim-error.png`、`P3-trim-csp-positive.png`、`P3-panel-before-markup.png`、`P3-panel-after-markup.png`、`P3-panel-after-trim.png`、`P3-recording-card.png`。
- 原始 JSON/运行日志在 git-ignored `.superpowers/p3-evidence/`（含本机临时路径，未提交；发布前无需清理仓库）。
