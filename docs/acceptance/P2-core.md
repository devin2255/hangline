# P2 视觉灵魂验收记录（验收文档）

- **里程碑**：P2 视觉灵魂（P2-1…P2-9）→ 里程碑标记 `v0.2.0-visual`（由 Lead 在全分支复审后创建）；包版本 **0.2.0**
- **分支 / 基线**：`feat/core`；P1 里程碑标签 `v0.1.0-core`，P2 起点 `d17d419`（P2-1 首个提交，基于 `v0.1.0-core`），其后 P2-6…P2-9 提交完成视觉里程碑；本文与 README 截图由 P2-9/P2-9-review 的 `docs:`/`fix:` 提交引入（具体提交见 `git log`）。
- **日期**：2026-11-03
- **结论口径**：
  - **程序化验收：P2 范围内完成**（质量门全绿；帧对比数值与 60 fps 有实测证据，见 §2/§3）。
  - **人工验收：未执行**。§5 的手工清单全部为 **MANUAL-PENDING**；在真人执行并回报之前，不得宣称「P2 验收通过」「视觉验收完成」或「v0.2.0-visual 发布就绪」。
  - 本文件**不是发布说明**：`v0.2.0-visual` 只表示「P2 视觉（玻璃卡片/绳子/别针、飞入/坠落、light/dark）以源码形式就绪、程序化验证完成」，不代表已打包/已发布/已人工验收。

**判定词**

| 词 | 含义 |
| --- | --- |
| **PASS（程序化）** | 由运行时观测/单元测试/构建产物闭合，出处见表格 |
| **MANUAL-PENDING** | 需要真人在真实鼠标/键盘、真实应用窗口、真实硬件下执行；未执行前不写作 PASS |
| **N/A / 延后** | 已明确归属后续里程碑（P3/P4/P5/V1.1） |

---

## 1. 环境与证据

| 项 | 值 |
| --- | --- |
| 操作系统 | Windows 11 家庭版 中文版 10.0.26300（26H2），交互式桌面 |
| 显示器 | 主屏 1920×1080 @100%（物理 `(0,0)`，工作区 1920×1032）；副屏 1920×1080 @150%（物理 `x=-1920`）；虚拟屏 `(-1920,0) 3840×1080` |
| GPU | Intel Iris Xe Graphics（单适配器） |
| WebView2 | 156.0.4314.58 |
| Node / pnpm / Rust | Node 22.17.1 / pnpm 10.30.3 / rustc 1.90.0 |
| Tauri | tauri crate 2.12.5（wry 0.57.1、tao 0.37.2） |
| 前端 | React 19.3.1 / TS 5.9.3 / Vite 7.3.7 / Vitest 3.2.7 |
| 构建形态 | 运行时证据用 **debug 构建 + `pnpm dev`**；本轮新增 `pnpm tauri build --no-bundle` 冒烟（见 §2.1） |
| 证据目录 | `docs/experiments/P2-*`（数值/表格/仅应用 UI 截图）；临时原始证据在 `.superpowers/p2-evidence/`（git-ignored） |

**P2-8 证据引用**：light/dark 面板、fly overlay、shortcut 对话框、hint、对比度表见 `.superpowers/p2-evidence/p2-8-*` 与 `task-8-report.md`；其中面板 dark/light 截图已脱敏后提交为 `docs/experiments/P2-panel-dark.png` / `P2-panel-light.png`。

---

## 2. 自动化质量门（本轮实测）

### 2.1 汇总

| 门 | 命令 | 结果 |
| --- | --- | --- |
| 前端质量闸门 | `pnpm check` | **exit 0**：eslint / `tsc --noEmit` / **vitest 130 passed / 20 files** / i18n / version |
| Rust 测试 | `cargo test --manifest-path src-tauri/Cargo.toml` | **240 passed / 0 failed / 5 ignored**（多屏顶边修复后；最终提交为准） |
| Rust 全目标检查 | `cargo check --all-targets` | **Finished，0 warnings** |
| Release 检查 | `cargo check --release --all-targets` | **Finished** |
| Release 构建冒烟 | `pnpm tauri build --no-bundle` | **exit 0**（本机未生成安装包；安装包属 P5） |
| 工作区卫生 | `git diff --check` | **clean**（无 whitespace/EOF 问题） |

### 2.2 运行时证据

| 证据 | 内容 |
| --- | --- |
| `docs/experiments/P2-perf.json` | **12 张 item / 同时可见 9（capacity 9，medium、keep=fit、1920 dp 面板）**，空闲 10 s：fpsMean **60.0006**、p5 fps **59.17**、worst **17 ms**、samples **600（≥590）**、longFrames（>20 ms）**0**、测量窗口内 `data-revealed=true`（240 次 monitor 采样）；面板在 fly 期间采样 60 fps（overlay 自身帧率未 instrumentation）；breeze 采样 60 fps / worst 16.9 ms。**仅 dark 主题**，light 主题未测帧率 |
| `docs/experiments/P2-visual-raw.json` | 逐帧 trace 原始数值（reveal/arrival/hover/press/fly/fall 等） |
| `docs/experiments/P2-visual.md` | 帧对比数值表、接触表、方法与限制 |
| `docs/experiments/P2-contact-sheet.png` | 本项目自有帧拼图（无 Tendedero 帧） |
| `docs/experiments/P2-panel-dark.png` / `P2-panel-light.png` | light/dark 面板截图（应用 UI，仅自生成测试图；用于 README） |

**60 fps 判定**：在 12 item（9 可见）空闲 10 s 窗口内 avg **60.0006 fps ≥ 58**、p95 帧 **17 ms ≤ 20 ms**、最差 **17 ms ≤ 50 ms**、样本 600（≥590）且全程 `revealed` → **PASS（程序化，dark 主题）**。长任务/Tendedero 逐帧录屏对比不在本机能力范围（无 macOS 参考录屏），见 §6。

---

## 3. P2 DoD 矩阵

| DoD 项 | 证据 | 判定 |
| --- | --- | --- |
| 玻璃卡片 / 别针 / 绳子材质（含 light/dark） | P2-1…P2-3 测试 + P2-8 截图/对比度表 + 本文截图 | **PASS（程序化）**；人工视觉复核对齐参考 → **MANUAL-PENDING** |
| 入场摆动 16°→0、−46→0 | `P2-visual.md`：起始 16°/−46.0，峰值 16° | **PASS（程序化）** |
| 微风/轻摆 1.6–3.4°、7–16 s | P2-4 测试 + perf breeze 观察（60 fps）；本次 compare trace 未捕获振幅 | **PASS（程序化，帧率/逻辑）**；振幅手感 → **MANUAL-PENDING** |
| 收起 tuck 220 ± 60 ms | `P2-visual.md`：**234 ms**（CSS ease-in 逐帧 trace，15 samples） | **PASS（程序化）**；真实指针移开手感 → **MANUAL-PENDING** |
| 飞入 650 ms、30 dp 弧、chrome 淡入、160 ms handoff | `P2-visual.md`：657 ms、30.0 dp、chrome→1；P2-5 证据 | **PASS（程序化）**；真实捕获手感 → **MANUAL-PENDING** |
| 坠落 550 ms / 520 dp / k³ / tilt+fade | `P2-visual.md`：564 ms、520 dp、opacity→0 | **PASS（程序化）**；真实点击手感 → **MANUAL-PENDING** |
| light/dark 材质与可读性 | P2-8：真实 CSS token worst-case 对比度（最低 4.93）、四窗口主题接线；light/dark 面板截图（中性渐变合成、纯应用 UI） | **PASS（程序化）**；人工视觉复核 light/dark 观感 → **MANUAL-PENDING** |
| 帧对比（数值） | `P2-visual.md` §1：reveal 383、tuck 234、arrival 16°/−46、hover 1.037、press 0.95、fly 657/30.0 dp、fall 564/520 dp 均 PASS；容差带出处为实现计划 `2026-11-02-hangline-windows-visual-soul.md`（`P2-reference.md` 提供 token/演示值） | **PASS（程序化）**；Copied nudge、breeze 振幅、真实光标手感 **MANUAL-PENDING** |
| 帧对比（与参考并排人工观感） | 本机无 macOS 参考录屏；参考素材本地 fair-use、未提交 | **MANUAL-PENDING** |
| 60 fps | 见 §2.2：60.0006 fps、worst 17 ms | **PASS（程序化）**；真机多显示器/150% 组合 → **MANUAL-PENDING** |
| P2-7 缩略图缓存（4K） | P2-7 测试 + `task-7-report.md` 证据（432 px、backfill、删除清理） | **PASS（程序化）**；capture/clipboard/recording 运行时链路为代码级（隐私限制，见 §6） |
| P2-8 light/dark | 同上 | **PASS（程序化）** |

---

## 4. 自动化测试覆盖（P2 相关）

- `fallPlan`（顺序/60 ms 阶梯/幂等）、`perf`（ring/percentile/long-frame）、`color`/`panelContrast`（真实 CSS token worst-case 对比度）、`theme`（system/light/dark 与 URL 解析）。
- Rust：`thumbnails`（432 px、幂等、stale 重建、孤儿清扫、tmp 年龄、捕获内存 crop、测试 cache-dir seam）、`Theme::as_str` 与 capture/shortcut URL 契约、fly watchdog kind、fall 相关单元测试。
- 全量：`pnpm check` **20 files / 130 tests**；`cargo test` **240 passed / 5 ignored**（2026-11-03 多屏顶边修复后；以最终提交为准）。

---

## 5. 人工验收清单（用户执行）

> 全部 **MANUAL-PENDING**；请在普通交互桌面、真实鼠标/键盘下完成并回报。任一项失败请附截图/录屏与日志路径。

1. 冷启动（≥3 张截图）：卡片落下并摆一次（16°→0），线展开约 0.4 s。
2. 等 7–16 s：线出现微风（1.6–3.4°）并回正。
3. **真实鼠标**悬停卡片：hover 阴影 + ~3.5% 放大（软过冲）；点击复制可用；按住缩到 0.95，松开弹回；在卡片摆动时重复，命中区域不应闪烁。
4. 复制卡片：3° 轻摆 + 玻璃 `Copied` 胶囊出现。
5. `Ctrl+Alt+S` 真实框选截图：截图从选区飞到卡片（0.65 s 弧线），落地轻摆。
6. 剪贴板/系统截图（无 capture rect）：从屏幕中心飞入。
7. 点 × 取下卡片：520 dp / 0.55 s 下坠、倾斜淡出；自有目录文件进回收站，监听目录文件仅下线。
8. 托盘「Take everything down」：最多两张以 60 ms 阶梯坠落，其余直接消失。
9. Windows 亮/暗切换或托盘主题设置：玻璃、绳、别针、胶囊、hint 均清晰可读；shortcut 对话框跟随主题。
10. **把指针移开面板/让线为空：Rust 先经约 500 ms 指针离开判定（或约 700 ms 空线延迟），随后线在 ~220 ms 内收起；再移回/触发 reveal，展开约 0.42 s。**
11. **多屏顶边悬停（2026-11-03 修复）：** 线收起后，分别把鼠标停在**每一块**屏幕工作区的上沿（即面板挂载的那条边；任务栏在顶部时是任务栏下方）约 0.5 s：线应出现在**该屏**顶部（不是上次所在屏）；两块屏交替悬停都应生效。
12. 150% 副屏/混合 DPI：面板迁移后卡片与飞入/坠落 chrome 尺寸正确；系统「减少动画」开启：无摆动/飞入/坠落，状态立即 snap；12 张时滚轮平滑。

---

## 6. 已知限制与 backlog（本机无法闭合/后续里程碑）

- **真实光标命中测试**（P2-4 起）：CDP 输入不是 OS 指针，真实点击/悬停命中与点击穿透仍为 MANUAL-PENDING。tuck 的 220 ms 动画已由 CSS 逐帧 trace 实测（234 ms，见 `P2-visual.md`），但真实指针移开的手感仍需人工。
- **真实 150% / 混合 DPI chrome**：本轮 60 fps 与帧对比均在主屏 100% 环境测得；150% 屏的卡片/飞入尺寸需人工复核。
- **参考帧对比**：本机无 macOS 参考录屏；Tendedero 素材仅本地 fair-use、未提交，帧对比为「源码 token/演示测量 vs 本机 trace」数值比较。
- **capture overlay 证据**：会在冻结的真实桌面上显示，受隐私约束未截图；仅代码/token 覆盖。
- **Copied/pill、recording play glyph 运行时**：CDP 单击未触发复制；recording 避免触碰真实 `Videos\Captures`；均由单测/token 覆盖。
- **缩略图 byte-only 缓存**：同一 `thumbPath` URL 的 WebView 缓存可能展示旧字节，`backfill refreshed` 已在代码/报告中标注为已知限制；无内容哈希 URL。
- **keepOnLine 显示-only 25/50/100 的持久保留语义**属 P4；0×0 recording/HEIC 尺寸、EXIF/GIF 缩略图保真属 P3/P4。
- **capture.rs 文档/函数拆分、sheen/pin 的 z-order 细节、shortcut hover 对比度**已在 P2-8/P2-9 修复（见 P2-8 report §G、P2-9 report）；如后续复审发现新问题按 backlog 记录。
- **安装包/发布**：`pnpm tauri build --no-bundle` 已通过；NSIS 安装包、签名、README 发布截图属于 P5。
