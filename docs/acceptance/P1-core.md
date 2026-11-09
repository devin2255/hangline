# P1 核心闭环验收记录（验收文档）

- **里程碑**：P1 核心闭环（计划 Task 17）→ 里程碑标记 `v0.1.0-core`（由 Lead 在全分支复审后创建）
- **分支 / 基线**：`feat/core`，基线提交 `1b51051`（Task 16 复审修复）；本文档、README/LICENSE 由 Task 17 的 `chore:` 提交引入（提交号见 `git log`）
- **日期**：2026-11-02
- **结论口径**：
  - **程序化验收：完成**（质量门全绿，证据见 §2；P1 矩阵中可由程序化证据闭合的部分均 PASS）。
  - **人工验收：未执行**。本文 §5 的手工清单全部为 **MANUAL-PENDING**，在本清单被真人执行并回报之前，**不得宣称「P1 验收通过」「P0 人工项已闭合」或「V1 发布就绪」**。
  - 本文件**不是发布说明**：`v0.1.0-core` 只表示「V1 核心功能以源码形式就绪、程序化验证完成」，不代表已打包/已发布/已人工验收。

**判定词**

| 词 | 含义 |
| --- | --- |
| **PASS（程序化）** | 由运行时观测/单元测试/构建产物闭合，出处见表格 |
| **MANUAL-PENDING** | 需要真人在解锁桌面、真实应用、真实硬件输入下执行；**未执行前不写作 PASS** |
| **N/A / 延后** | 已明确归属后续里程碑（P3/P4/P5/V1.1），不属本次 P1 验收 |

---

## 1. 环境

### 1.1 测试机

| 项 | 值 |
| --- | --- |
| 操作系统 | Microsoft Windows 11 家庭版 中文版 **10.0.26300（26H2）**，UBR **9711** |
| 会话 | 交互式桌面；证据收集期间部分轮次经 **RustDesk 远程桌面**，部分轮次被**真人锁屏**（相关限制写在各实验文档 §2.1/§2.2） |
| GPU | Intel(R) Iris(R) Xe Graphics，驱动 **32.0.101.7088**（单适配器） |
| 显示器 | 主显示器 **1920×1080 @100%**（物理 `(0,0)`，工作区 1920×1032）；副显示器 **1920×1080 @150%**（物理 `x=-1920`，工作区 1920×1008）；虚拟屏幕 `(-1920,0) 3840×1080` |
| DPI 感知 | 应用进程 Per-Monitor V2（实测 `dpi_awareness=2`）；DPI-unaware 进程会把 150% 屏读成 1280×720@96 |
| WebView2 Runtime | **156.0.4314.58**（登记表 `pv`）；P0/P1 各实验轮次记录为 156.0.4314.50，小版本差异不影响结论 |
| Node / pnpm | Node **22.17.1** / pnpm **10.30.3** |
| Rust | rustc / cargo **1.90.0**（MSVC），edition 2021 |
| Tauri | `tauri` crate **2.12.5**（`@tauri-apps/cli` 2.12.4；wry 0.57.1、tao 0.37.2） |
| 前端 | React 19.3.1、TypeScript 5.9.3、Vite 7.3.7、Zustand 5.0.16、Vitest 3.2.7 |
| 构建形态 | 运行时证据用 **debug 构建**（`cargo build`）+ Vite dev server；本轮新增 **release `--no-bundle`** 构建冒烟（§2.5） |
| 仓库 | `hangline`，分支 `feat/core`，基线 `1b51051`，工作区干净（提交后） |

> **显示器编号口径**：Windows 可能在不同会话重排 `\\.\DISPLAY1/DISPLAY2` 的编号（P0 记录 DISPLAY1=100% 主屏；Task 16 记录 DISPLAY2=100% 主屏、DISPLAY1=150% 副屏）。本文按**物理布局**描述：主屏 100% @ `(0,0)`，副屏 150% @ `x=-1920`。两者在 P0/Task 10–16 各轮中布局一致。

### 1.2 agent 沙箱与观测能力边界（影响可复现性）

- **`pnpm tauri dev` 在 agent 沙箱内无法启动**：workspace 内 exe 初始化 `tauri-plugin-log` 时写 `%LOCALAPPDATA%` 被拒绝（`PluginInitialization("log", "拒绝访问。 (os error 5)")`）。各任务运行时证据因此采用等价方式：`pnpm dev` + `cargo build` 后把**同一 debug 二进制**复制到 `%TEMP%` 运行。**精确的 `pnpm tauri dev` 启动路径仍需真人在普通桌面验证（P0 条件 c）**。
- **锁屏/远程会话限制**：锁屏时 `keybd_event`/`SendInput` 进入锁屏输入桌面，无法触发应用全局热键；P0 E4/E5 与 Task 16 review 轮因此无法做真实物理输入。涉及真实键盘/鼠标/目标应用的项目全部列为 MANUAL-PENDING。
- 本轮 Task 17 的自动化质量门在 agent 会话中执行；**没有**用程序化手段替代人工验收。

---

## 2. 自动化质量门（本轮实测）

> 执行时间 2026-11-02 13:20–13:30（UTC+8）。命令均在仓库根目录执行，日志保留在 `%TEMP%\hangline-t17-gates\`（临时，不提交）。

### 2.1 汇总

| 门 | 命令 | 结果 |
| --- | --- | --- |
| 前端质量闸门 | `pnpm check` | **exit 0**：eslint 通过、`tsc --noEmit` 通过、**vitest 72 passed / 11 files**、i18n 18 keys、version 0.1.0 |
| Rust 测试 | `cargo test --manifest-path src-tauri/Cargo.toml` | **208 tests：203 passed, 0 failed, 5 ignored** |
| Rust 全目标检查 | `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | **Finished，0 warnings** |
| Rust lint | `cargo clippy --all-targets --manifest-path src-tauri/Cargo.toml` | **exit 0，0 errors，18 warnings**（全部为既有代码告警，见 §2.3；不阻塞里程碑） |
| Release 构建冒烟 | `pnpm tauri build --no-bundle` | **exit 0**；产物 `src-tauri/target/release/hangline.exe`（见 §2.5） |
| 版本一致性 | `node scripts/version.mjs` + 人工核对 | **0.1.0 一致**（§2.6） |

### 2.2 `pnpm check` 与 `cargo test` 细节

`pnpm check` = `eslint` + `tsc --noEmit` + `vitest run` + `scripts/check-i18n.mjs` + `scripts/version.mjs`：

```text
Test Files  11 passed (11)
     Tests  72 passed (72)
i18n ok: 18 keys, 1 languages
version ok: 0.1.0
```

> 注：`check-i18n.mjs` 输出 “1 languages” 指 `en.json` 的 key 集合基准（zh-Hans 与 en 完全一致由脚本校验），不是只有一种语言。

`cargo test`：

```text
running 208 tests
test result: ok. 203 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out
```

- 5 个 `#[ignore]` 均为**环境型证据测试**，不是规避：
  - 2 个 E5 真实双屏采集测试（`capture::tests::e5_real_session_monitors`、`e5_virtual_screen_capture_crops_and_pixels`）——需要本机双屏 + 可写 `%LOCALAPPDATA%`；
  - 3 个 OLE 拖拽证据测试（`dragout::tests::ole_drag_loop_*`、`interactive_source_*`、`watchdog_cancels_a_drag_loop_*`）——需要交互桌面。
  - 复跑方式：`cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored --test-threads=1 --nocapture`（E5 需把测试二进制复制到 `%TEMP%` 或普通桌面运行）。
- 各任务报告中最终测试数（203 passed / 5 ignored）与本轮一致；Task 16 报告曾把 ignored 概括为「Task 5 E5 测试」，实际为 2 个 E5 + 3 个 OLE，以本轮清单为准。

### 2.3 `cargo clippy`（18 warnings，全部为既有代码，不计入失败）

`cargo clippy --all-targets` **exit 0、0 errors**。18 条 warning 均位于 Task 2–16 已提交的既有代码（lib 6 条 + 测试目标新增 12 条），**Task 17 未引入 Rust 代码**；为避免无关 churn，本轮不做批量修复，登记为后续清理项：

| 类别 | 数量 | 位置 |
| --- | --- | --- |
| `too_many_arguments` | 2 | `src/reveal.rs:852`、`src/reveal.rs:882` |
| `collapsible_if` | 1 | `src/reveal.rs:1312` |
| `field_reassign_with_default` | 6 | `src/actions.rs:649,669`、`src/hotkeys.rs:450`、`src/settings.rs:859`、`src/tray.rs:1180,1412`（多在测试代码） |
| `manual_div_ceil` | 2 | `src/watchers/clipboard.rs:392,1462` |
| `bool_assert_comparison` | 2 | `src/capture.rs:2254,2259`（测试） |
| `manual_contains` | 1 | `src/watchers/mod.rs:374` |
| `drop_non_drop` | 1 | `src/lib.rs:325`（`tauri::State` 的 drop 提示，非缺陷） |
| `assertions_on_constants` | 1 | `src/shortcut_window.rs:90`（测试） |
| `cloned_ref_to_slice_refs` | 1 | `src/store.rs:264`（测试） |
| `useless_vec` | 1 | `src/actions.rs:551`（测试） |

> 环境记录：本机 rustup toolchain 原本未安装 clippy 组件，本轮为执行质量门安装了 `clippy` 组件（`rustup component add clippy`），未改动项目依赖。安装后 `cargo clippy --version` 正常。

### 2.4 `cargo check --all-targets`

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.33s
EXIT=0
```

日志中 **0 条 warning**。

### 2.5 Release 构建冒烟（`pnpm tauri build --no-bundle`）

**不尝试 NSIS 打包**（P5 范围，且需 WebView2 bootstrapper/NSIS 工具链下载）；只验证 release 编译与产物。

```text
Running beforeBuildCommand `pnpm build`      # tsc --noEmit && vite build
dist/index.html                   0.39 kB
dist/assets/index-D2GNZ-Nb.css   11.02 kB
dist/assets/index-U3pA8CGq.js   257.50 kB
Finished `release` profile [optimized] target(s) in 4m 30s
Built application at: ...\src-tauri\target\release\hangline.exe
EXIT=0
```

（首次全量构建 4m30s；为确认 CLI 规范化副作用在同一树上复跑增量构建 58 s；提交规范化结果后再次复跑 55.23 s 且 `git status` 干净——三次均 exit 0、无 warning。）

| 项 | 值 |
| --- | --- |
| 产物 | `src-tauri/target/release/hangline.exe` |
| 大小 | **18,617,856 B（17.76 MiB）** |
| SHA-256（最近一次构建记录） | `99B6B4279A5B7A275494CD55F52713823C10352D3A340A1719D3BFF650A083B9` |
| 版本资源 | ProductVersion / FileVersion = **0.1.0**；ProductName = Hangline |
| 构建告警 | 输出中无 warning/error 行 |
| 前端产物 | `dist/`（index.html + 1 css + 1 js），release 构建内嵌 |

> **构建副作用（已入库）**：`pnpm tauri build` 会把 `src-tauri/Cargo.toml` 的 `tauri-build` 依赖规范化为 `features = []`（空 feature 集，无语义变化）。本轮把这一 CLI 规范化结果随提交入库，保证再次执行 `pnpm tauri build --no-bundle` 后工作区仍然干净。
>
> **可复现性**：三次 release 构建 exe 大小一致但 SHA-256 各不相同（如 `4083C5E4…`、`CFDB903D…`、`99B6B427…`）；未承诺字节级可复现构建，**产物哈希仅作本次构建记录，不是发布校验和**。

**未验证 / 不作承诺**：

- **NSIS 安装包、portable zip、代码签名、自动更新、安装包体积（spec 目标 ≤15 MB）属 P5**，本轮没有产物、没有结论；17.76 MiB 是未 strip 的 release exe 大小，不能与压缩后的安装包目标直接比较。
- 该 exe **未做启动冒烟**（运行它会读写用户数据、抢热键、弹托盘，不适合在验收环境自动执行）；其启动行为以 dev/debug 构建的运行时证据为准。
- 构建未启用的 P4/P5 功能（全屏抑制、安装/更新）不在该产物中承诺。

### 2.6 版本一致性

| 位置 | 值 |
| --- | --- |
| `package.json` | `0.1.0` |
| `src-tauri/tauri.conf.json` | `0.1.0` |
| `src-tauri/Cargo.toml` | `0.1.0` |
| `src-tauri/Cargo.lock`（`hangline` 包） | `0.1.0` |
| release exe 版本资源 | `0.1.0` |

`scripts/version.mjs` 只校验 `package.json` 与 `tauri.conf.json`（Task 1 遗留 minor）；本轮另行人工核对了 `Cargo.toml` 与 `Cargo.lock`，四者一致。

### 2.7 CI

仓库已配置 `.github/workflows/ci.yml`（windows-latest：`pnpm check` → `cargo test` → `pnpm tauri build --no-bundle`），但**本仓库当前没有配置 git remote，CI 尚未实际运行**；本轮质量门为本地直跑结果。

---

## 3. P1 验收矩阵（spec §7.2）

**说明**：表中「程序化」列只写由代码/测试/运行时观测闭合的部分；凡涉及**真实物理输入、真实应用、真实系统工具或人工视觉判断**的子项一律进「人工待验」列并标 **MANUAL-PENDING**。**没有任何人工项被标为 PASS。**

| # | 场景（spec §7.2） | 程序化证据（结论 + 出处） | 人工待验（MANUAL-PENDING） | 判定 |
| --- | --- | --- | --- | --- |
| **M1** | 单屏 100%，冷启动 → 托盘出现，3 s 内可用 | 启动链路/单实例/tray 创建/READY gating：`P0-report.md` §2.3、§2.6（tray 创建、第二次启动立即退出、窗口 1690 ms 创建）；`P1-tray-settings.md` §3 #16–18、§4.1/§4.7（ready 前 toggle 排队、3 s 兜底、正常 ready 忽略超时）；`P1-reveal.md` §3 #1（最终 debug 构建空线启动 `panel ready at +1181 ms`）。**注意**：P0 早期 debug+dev 构建从启动到面板可见为 **3415 ms**（`P0-report.md` §2.3），超过 3 s 目标；release/dev 正常桌面冷启动未按 M1 口径计时 | 普通桌面精确执行 `pnpm tauri dev`（P0 条件 c）；托盘图标真实出现（任务栏/溢出区）；冷启动 0–3 s 内可操作；首帧无白闪（P0 条件 d）→ 清单步骤 1/2/5 | 程序化 **PASS**（启动/READY/单实例）；**M1 整体 MANUAL-PENDING** |
| **M5** | `Win+PrtScn` 与 `Win+Shift+S`（自动保存开/关）→ 均只上一条，无重复 | 目录监听 4 路 + 200 ms 防抖 + 哈希/路径双层去重：`P1-dir-watcher.md` §3 #1–#14、§4.1–§4.5；剪贴板链路与跨源去重：`P1-clipboard.md` §3 #1–#14、§4；自建截图与 watcher 去重：`P1-capture.md` §3 #7 | 真实 Windows 截图工具/Snipping Tool 的分块写入时序；Win+PrtScn 与 Win+Shift+S 自动保存各一次的真实链路 → 清单步骤 7 | 程序化 **PASS**（链路/去重）；真实系统工具 **MANUAL-PENDING** |
| **M6** | 复制纯图片（Win+Shift+S / 微信 / Excel 单图）与复制文本 → 纯图片上栏；文本被忽略；浏览器图片按规则过滤 | 剪贴板过滤决策表、PNG/DIBV5/DIB 解码、文本/文件/隐私标记拒绝、80 MP 上限、原子落盘 + 去重：`P1-clipboard.md` §3 #2–#14、§4.1–§4.6、§6；浏览器「复制图片」带 HTML/文本时**按 controller 修订明确不采集**（`P1-clipboard.md` §3 #5、§4.3、§7 #1；spec §3.2 C/§4.6 D 修订） | 真实 `Win+Shift+S`（仅剪贴板）、微信/QQ、Excel 单图/区域的复制行为（`P1-clipboard.md` §7 #1–#3）→ 清单步骤 8 | 程序化 **PASS**（合成 DIB/PNG 全链路）；真实应用 **MANUAL-PENDING** |
| **M8** | 单击/双击/长按/Force 快看替代 → 行为与 §3.1 一致，单击不污染双击 | 单击延迟复制 + 胶囊、PNG+DIBV5+HDROP 三格式校验、双击不写剪贴板、26 dp 叉、缺失文件只下线、busy 不收起：`P1-copy-trash.md` §3 #1–#4、#7、#10–#12、§4.1–§4.6；spring/命中矩形：`P1-reveal.md` §3 #12、§4.3。**长按=标注/裁剪属 P3，未实现 → M8 的「长按」子项 N/A(P3)**；「Force 快看替代」在本项目等价为双击打开，已实现 | 真实粘贴（微信/QQ/浏览器得到图片而非文件）、默认应用 GUI 展示、真实物理鼠标单击/双击/叉、150% 触摸目标（`P1-copy-trash.md` §5 #1–#4）→ 清单步骤 6/8 | 程序化 **PASS**（P1 范围内单击/双击/叉/busy）；**长按延后 P3**；真实粘贴 **MANUAL-PENDING** → M8 整体 MANUAL-PENDING |
| **M9** | 拖到微信/Figma → 对方收到图片；卡片保留 | OLE 拖源提供 `COPY|MOVE`（不含 LINK）、`DoDragDrop` DROP/CANCEL、`CF_HDROP` 精确路径：`P0-report.md` §2.4、`P0-E4.md` §4.4/§9；`Copy+exists → Done`（单次检查 no-op、卡片保留、revision 不变）实测：`P1-drag-prune.md` §3.2、§10 C | 真实拖到微信/Figma 的投递与卡片保留（`P1-drag-prune.md` §6 #1）→ 清单步骤 9 | 程序化 **PASS**（策略/会话）；真实目标 **MANUAL-PENDING** |
| **M10** | 拖到桌面文件夹 → 文件移动，卡片 0.6 s 内掉落 | `Move+exists` 600 ms 初检 + 5×400 ms 重试、文件消失即 `prune`：决策表与运行时 A/B 用例 `P1-drag-prune.md` §3.2、§10 A/B；OLE DROP/CANCEL：`P0-E4.md` §9.2 | 真实 Explorer 文件夹/桌面 MOVE 投递与卡片掉落时序（`P1-drag-prune.md` §6 #2）→ 清单步骤 10 | 程序化 **PASS**（重试/prune 策略）；真实目标 **MANUAL-PENDING** |
| **M11** | 拖到回收站 → 文件进回收站，卡片掉落 | `Move/Copy/None + missing → Prune` 安全网与回收站语义（叉按钮路径：`P1-copy-trash.md` §3 #6 真实 `IFileOperation` 回收并枚举回收站）；拖入回收站的 shell effect 未实测 | 真实拖到回收站、文件可还原、卡片掉落（`P1-drag-prune.md` §6 #3）→ 清单步骤 11 | 程序化 **PASS**（prune + 回收站基础）；真实拖入回收站 **MANUAL-PENDING** |
| **M16** | 退出/重启 → 截图设置无残留；条目按存在性恢复 | 托盘 Quit 优雅退出（停 watchers/clipboard、无孤儿进程、`RunEvent::Exit` 幂等）：`P1-tray-settings.md` §3 #21、§4.8；全部设置跨重启保留：§3 #22、§4.9；条目按存在性恢复/丢失条目剔除：Rust 单测 `store::tests::load_*`、`app_state::tests::load_drops_and_rewrites_items_whose_files_are_gone`（cargo test 本轮通过）；state/settings 原子写 | 真实托盘 Quit 点击（本轮程序化用真实 Win32 菜单 + 鼠标注入，真人点击仍待验）；真实重启并核对（`P1-tray-settings.md` §3 #22、§4.9）→ 清单步骤 14（可选） | 程序化 **PASS**（持久化/退出）；真实 Quit 点击 **MANUAL-PENDING** → M16 整体 MANUAL-PENDING |

### 3.1 矩阵结论

- **程序化判定**：M1/M5/M6/M8/M9/M10/M11/M16 中由代码/测试/合成输入可闭合的部分**全部 PASS**（0 FAIL）。
- **人工判定**：上述 8 项都包含真实物理输入/真实应用/真实系统工具的子项，**全部 MANUAL-PENDING**。按 P1 DoD 的字面要求（「M1/M5/M6/M8/M9/M10/M11/M16 通过」），**P1 验收尚未完成**；当前状态是「程序化完成 + 人工清单待执行」。
- **M8 的已知范围缺口**：长按标注/裁剪（M8 长按项、M12、M13）属 P3，不计入 P1 失败，但也不能当作已通过。

---

## 4. P0 门禁条件（a）–（f）状态

依据 `docs/experiments/P0-report.md` §3（门禁决策、条件表）与 §5（人工清单）：

| # | 条件（P0-report §3.2） | 状态 | 说明 / 本轮证据 |
| --- | --- | --- | --- |
| **a** | 真实物理鼠标从卡片拖入 Explorer/桌面/微信/Figma，以及解锁桌面取消（Esc） | **MANUAL-PENDING** | 程序化部分已闭合（OLE DROP/CANCEL、`CF_HDROP`、effect 驱动 prune，见 §3 M9–M11）；真实目标与物理鼠标未执行 → §5 清单步骤 9–11 |
| **b** | 解锁后副屏真实内容采集 + 交互式跨屏区域框选 | **MANUAL-PENDING**（内容/框选均待人工） | 锁屏时副屏为黑是 OS 行为（`P0-E5.md` §5.2）；E5 ignored 测试需解锁桌面（`e5_virtual_screen_capture_crops_and_pixels`）；交互式跨屏框选在 Task 16 用 CDP 合成指针在负 x/150% 区域程序化验证（`P1-capture.md` §3 #2–#4），但**真人跨接缝拖框**未做 → §5 清单步骤 13（含 P0 §5 步骤 10 的 E5 测试复跑） |
| **c** | 在普通桌面执行精确的 `pnpm tauri dev`（沙箱内被 log 插件 `os error 5` 阻断） | **MANUAL-PENDING** | agent 沙箱限制见 §1.2；各任务用 `%TEMP%` 副本等价运行 → §5 清单步骤 1 |
| **d** | 首帧白闪 / 任务栏不显示 / 150% DPI 视觉 / 真实点击穿透 / 快速进出无可感延迟闪烁 | **MANUAL-PENDING** | 程序化已闭合：透明无黑底（像素差分 0）、命中路由、点击不抢焦点、穿透切换 25–41 ms（`P0-report.md` §2.1/§2.2）；Task 10/14 已把 150% 面板几何/容量/跟随程序化验证。人工视觉与真实点击 → §5 清单步骤 2/3/13 |
| **e** | 真实任务栏托盘点击（左键/右键/双击）与真实物理全局热键 | **MANUAL-PENDING** | 程序化已闭合：RegisterHotKey 注册/冲突回滚、注入键触发、真实 Win32 `#32768` 菜单与菜单项鼠标注入、Task 16 WM_HOTKEY 投递验证热键 handler→worker（`P0-report.md` §2.3、`P1-tray-settings.md` §3、`P1-capture.md` §2.1/§3 #11）；真实物理键鼠 → §5 清单步骤 4/5 |
| **f** | 范围外延后项复核（跨屏放置、全屏置顶、其他 GPU、E4 150% 矩阵、release 构建、NSIS 等） | **部分关闭** | ✅ 本轮已完成 **release 构建冒烟**（`pnpm tauri build --no-bundle`，§2.5）；✅ 跨屏放置/连续跟随已由 Task 10/14 交付并程序化验证（`P1-panel-ui.md`、`P1-capacity-scroll.md` §9）；⬜ 其余按归属延后：全屏抑制（P4）、其他 GPU/NSIS/签名/自动更新（P5）、E4 150% 矩阵（并入 §5 清单步骤 10）、DRM/负 y 屏/顶部任务栏（P4/P5 复核） |

> **P0 判定未升级**：`P0-report.md` 的 GO 语义是「允许开始 P1」，不是「P0 验收完成」。**(a)–(e) 在本文 §5 清单执行前仍未闭合**，因此不得宣称「P0 人工项已闭合」。本轮只新增了 (f) 中的 release 构建冒烟。

---

## 5. 人工验收清单（合并 P0 §5 + P1 新功能，约 15–30 分钟）

**准备**

- 解锁桌面；保持双屏（主屏 100%、副屏 150%，副屏上最好有窗口内容）；关掉全屏游戏/视频。
- 准备：一个空文件夹（测拖放移动）、微信或 QQ（可选，测粘贴/拖到应用）、Figma/浏览器（可选，M9 替代目标）。
- 从仓库根目录构建并启动（见步骤 1）。**每步记录实测结果；异常时附日志/截图**。
- **隐私**：回报以文字为主；如必须截图，只截 Hangline 自身 UI，**不要**提交桌面、微信聊天、私人图片或全屏冻结画面。
- 建议先读 `docs/experiments/P0-report.md` §5 与各 `P1-*.md` 的「手动待验证」节，了解已知限制。

### 步骤 1 —— 构建与启动（P0 条件 c / M1）

1. 在仓库根目录执行 `pnpm install`、`cargo build --manifest-path src-tauri/Cargo.toml`、`pnpm tauri dev`。
2. **期望**：编译成功；应用启动后顶部出现一条绳/面板（空线约 0.7 s 后收起），控制台**没有** `PluginInitialization("log", ... os error 5)` 之类的 panic。
3. **回报**：是否成功启动；失败贴报错原文；首次编译耗时；从启动到面板/托盘可用的秒数。

### 步骤 2 —— 首帧、透明与任务栏（P0 条件 d / M1）

1. 重启应用，盯住面板第一次出现；然后按 `Ctrl+Alt+T` 隐藏、再按一次显示。
2. 看任务栏与 `^` 溢出区，确认托盘图标可见且没有 Hangline 窗口按钮。
3. **期望**：无白闪/黑底，卡片外能直接看到桌面或浏览器；任务栏无窗口按钮（托盘图标可能在溢出区）。
4. **回报**：闪的颜色/时长；任务栏是否出现窗口按钮；托盘图标位置。

### 步骤 3 —— 真实点击穿透 / 焦点 / 快速进出（P0 条件 d / E1–E2）

1. 在面板的透明区域（卡片左右）点击桌面图标或浏览器链接；再点击卡片。
2. 鼠标快速来回划过卡片边缘 5 次。
3. **期望**：透明区点击落到下层应用；卡片可按下，前台窗口不会变成 Hangline；切换无可感延迟/闪烁，不抢焦点。
4. **回报**：是否有某次点击被面板吃掉；前台是否被抢；是否感到延迟或闪烁。

### 步骤 4 —— 真实物理热键（P0 条件 e / E3 / M8）

1. 真实键盘按 `Ctrl+Alt+T` 两次；按 `Ctrl+Alt+S` 框选一小块并松开（截图上栏）；再按 `Ctrl+Alt+S` 进入框选后按 `Esc`（验证取消：不产生文件/卡片）。
2. **期望**：`Ctrl+Alt+T` 隐藏/显示；`Ctrl+Alt+S` 冻结屏幕并进入框选；松开后截图上栏；`Esc` 取消无文件/无卡片。
3. **回报**：是否响应；是否与 QQ/微信/Snipaste 等热键冲突；取消是否干净。
4. 说明：当前默认截图热键是 `Ctrl+Alt+S`；若被占用，可在托盘快捷键里改键后重试。

### 步骤 5 —— 真实托盘点击、菜单与语言（P0 条件 e / Task 15）

1. 左键单击托盘图标；右键打开菜单，逐项查看；切换 **Language → English**，再切回 **简体中文**；最后点一次「显示线/隐藏线」。
2. **期望**：左键切换线的显示；右键菜单项完整（显示线、全部取下线、截图、来源勾选、线出现方式、也监听文件夹、保留数量、尺寸、快捷键、打开截图文件夹、声音、开机启动、检查更新、语言、退出）；切换语言后菜单与面板文案同时变化。
3. **回报**：托盘图标是否在溢出区；菜单是否完整/可用；语言切换是否同时生效；双击托盘的行为（当前实现为两次 toggle，净效果不变）。

### 步骤 6 —— 截图 → 复制 → 粘贴 → 持久化（M1 / M8 / M16）

1. `Ctrl+Alt+S` 框选一块有明显内容的区域 → 卡片上栏；单击卡片（等约 0.5 s）→ 出现「Copied/已复制」提示；到微信/浏览器 `Ctrl+V`。
2. 重启应用，确认条目仍在（文件存在）；然后在资源管理器删除某张自有截图文件，再重启应用，确认该卡片不再出现。
3. **期望**：单击复制得到图片（微信/浏览器粘贴出图，不是文件）；双击卡片用默认程序打开且不写剪贴板（步骤 1 中需实际双击一次）；重启后有效条目恢复、丢失文件的条目自动消失。
4. **回报**：粘贴结果（图片/文件/无）；双击打开情况；重启前后卡片数量变化。

### 步骤 7 —— 系统截图链路（M5）

1. 打开「截图工具」的自动保存（或使用 `Win+PrtScn`）截一张；再在自动保存开的情况下用 `Win+Shift+S` 截一张（若系统设置里没有自动保存，请先在截图工具设置中开启）。
2. 观察是否只上一条、没有重复卡片。
3. **期望**：`Win+PrtScn` 与 `Win+Shift+S`（自动保存）各只产生一条卡片；同一张图不会因目录+剪贴板双链路出现两次。
4. **回报**：每种操作产生几条卡片；是否有重复/缺失。

### 步骤 8 —— 剪贴板图片与真实粘贴（M6 / M8）

1. 用 `Win+Shift+S` 截图（不保存，仅复制）→ 看是否上栏；在微信/QQ 里复制一张图片 → 看是否上栏；复制一段纯文本 → 不应上栏；在浏览器「复制图片」（带 HTML/文本）→ 按设计不应上栏。
2. 单击上栏的卡片，到微信/QQ/浏览器 `Ctrl+V`。
3. **期望**：纯图片上栏；文本/浏览器带 HTML 的复制被忽略；粘贴得到图片。
4. **回报**：各来源是否上栏；粘贴结果；如某应用行为异常，记录应用名与复制方式。

### 步骤 9 —— 拖到应用（M9 / 条件 a）

1. 按住某张卡片拖到微信/Figma/浏览器（任选可接收文件的应用）后松开。
2. **期望**：出现系统拖拽光标；目标收到图片/文件；卡片保留在线上；日志（若查看）显示 `effect=copy`。
3. **回报**：目标是否收到；卡片是否保留；是否出现卡在按下态。

### 步骤 10 —— 拖到文件夹 / 桌面（M10 / 条件 a；含 E4 150% 矩阵）

1. 把卡片拖到一个空文件夹后松开；再拖一张到桌面后松开。
2. 把鼠标移到 150% 副屏，确认线迁移后将卡片拖到该屏上的文件夹（复测 E4 的 150% DPI 矩阵行）。
3. **期望**：文件出现在目标位置，原路径消失；卡片在约 0.6 s–2.6 s 内掉落（慢速 shell 移动最多重试约 2 s）；桌面是有效投放目标（应出现文件/副本）；150% 屏上拖拽光标与投递同样正常。
4. **回报**：文件是否出现在目标；原文件是否消失；卡片掉落耗时；150% 屏拖拽是否异常；是否卡住不消失。

### 步骤 11 —— 回收站与取消（M11 / 条件 a）

1. 把卡片拖到回收站图标上松开；确认文件可在回收站还原、卡片掉落。
2. 再拖一次，拖出线外后按 `Esc`（不投递）→ 不应产生/删除任何文件，卡片保留。
3. **期望**：回收站路径：文件可还原、卡片掉落；`Esc` 取消：无文件变化、卡片复位、可再次拖拽。
4. **回报**：回收站中是否出现该文件；取消后源文件是否仍在；卡片状态。

### 步骤 12 —— 文件夹监听与 Add Folder 选择器（Task 15 MANUAL-PENDING）

1. 托盘 → **也监听文件夹 → 添加文件夹…**，选择一个已有 PNG 的文件夹；往其中复制一张新 PNG；再试一次选择器并点「取消」。
2. **期望**：选择器正常打开/取消（取消不改变设置）；新图片以「监听目录」类型上栏；已监听 3 个目录后「添加文件夹…」变灰。
3. **回报**：选择器是否可点；取消是否生效；新图片是否上栏；上限是否生效。

### 步骤 13 —— 多屏跟随与跨 DPI 框选（条件 b / Task 14/16）

1. 把鼠标移到 150% 副屏并停留约 1–2 s；再把鼠标移回主屏。观察线的移动、卡片大小/清晰度。
2. 在 150% 副屏上框选一块（若方便，让框横跨 x=0 接缝），检查截图内容/尺寸是否正确。
3. **期望**：线在 500 ms 停留后迁移到指针所在显示器，物理尺寸 1920×315（150%）、卡片正确缩放；跨接缝框选的截图内容与选区一致、尺寸标签正确。
4. **回报**：是否跟随、迁移延迟观感；跨接缝截图是否正确（可回报尺寸与明显错位情况）。P0 条件 b 的 E5 真实内容测试可按 `P0-report.md` §5 步骤 10 复跑（需从 `%TEMP%` 副本运行）。

### 步骤 14 —— 滚轮/容量与本步可选退出（M14 相关 / M16）

1. 托盘 → **保留数量 → 100**；造出 12 张以上截图（可用 `Ctrl+Alt+S` 连截）；用鼠标滚轮/触控板向前/向后滚动。
2. 托盘 → **Quit Hangline**；确认进程退出、无残留；重新启动，确认设置保留。
3. **期望**：滚轮/触控板可翻阅更早卡片，最新一张始终在右侧，无明显卡顿；退出无孤儿进程；重启后设置（保留数量等）保留。
4. **回报**：滚动是否流畅/方向是否符合预期；退出是否干净；重启设置是否保留。

> 全部步骤执行完后，把每步「实际结果」回报给维护者；**异常项**请附应用日志（`%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log`）与操作时间。

---

## 6. 已知缺口与延后项

### 6.1 明确不在 P1 范围（不阻塞本次里程碑）

| 项 | 归属 | 现状 |
| --- | --- | --- |
| 卡片右键上下文菜单（复制/打开/在资源管理器中显示/取下线左右侧…） | V1.1 | 未实现；当前卡片操作 = 单击/双击/拖出/× |
| 长按标注编辑器（画笔/矩形/箭头/文字/裁剪/撤销） | P3 | 未实现；长按目前无动作（M8 长按项、M12 延后） |
| 录屏卡片首帧 + 时间轴裁剪 | P3 | 录屏卡片以播放角标占位；可复制/打开文件，不能裁剪（M13 延后） |
| 全屏抑制（`SHQueryUserNotificationState`） | P4 | 托盘「全屏应用上」设置已持久化，但自动隐藏未落地（M4 延后） |
| 完整设置窗口、改键 UX 完善、多屏/DPI 边界复核 | P4 | 当前为托盘菜单 + 独立改键小窗口（M2/M3/M15 的人工复核待 P4） |
| NSIS 安装包 / portable / 代码签名 / 自动更新 / 11 语言 / press kit / 发布脚本 | P5 | 全部未开始；目前只有 en / zh-Hans；无安装包、无 release |
| 自研录屏编码器 / OCR / 云同步 / 插件系统 / 多用户 | 非目标（spec §1.4） | 不做 |

### 6.2 已知限制（用户可感知）

- **HEIC 缩略图**：`image` crate 无 HEIC 解码器，`.heic` 会上栏但尺寸 0×0、显示文件名占位；真实 HEIC 能否被 WebView2 解码取决于系统扩展（`P1-dir-watcher.md` §8 偏差 4）。
- **剪贴板只认 `PNG`/`CF_DIBV5`/`CF_DIB`**：仅提供旧式 `CF_BITMAP` 的复制（部分旧应用/`Clipboard.SetImage`）不采集（V1.1 需要 `GetDIBits`）；浏览器「复制图片」带 HTML/文本时**按设计不采集**。
- **DRM/受保护窗口**：GDI 截屏为黑，OS 限制（`P0-E5.md`、`P1-capture.md` §6.10）。
- **Windows 10 22H2 未实测**；顶部/侧边任务栏、主屏上方显示器（负 y）、多显示器热插拔中途截图未实测（`P1-capture.md` §7、`P1-panel-ui.md` §6）。
- **托盘 tooltip 可读性、提示音听感**未人工确认（文案有单测）。
- **未签名**：未来安装包会触发 SmartScreen「未知发布者」（P5 文档化）。
- **release exe 17.76 MiB（未 strip）**；安装包体积目标 ≤15 MB 未验证（P5）。

### 6.3 残留风险（来自各任务 review 记录，摘要）

> 这些是代码审查记录的非阻塞风险，不是 P1 失败项；后续里程碑修 P2/P3/P4 时择机处理。

1. **拖拽会话看门狗**在首次 `QueryContinueDrag` 后主动退出（只覆盖「一次输入都没有」的卡死）；合法 >30 s 长拖拽会被前端 30 s 看门狗提前清视觉态（卡片「弹起」，不取消 OLE 会话）。
2. **`after_drag` Move 重试窗口（约 2.6 s）耗尽后保留卡片**：慢速/夸卷 shell 移动若更久完成，卡片会暂时指向已移走文件，直到下一次 `after_drag`/重启才 prune（watcher 不主动 prune 卡片）。
3. **capture 挂起/恢复按顺序而非 generation 匹配**：极端交错下可能出现罕见的 stale resume（已用 generation ownership 覆盖 cancel/finish 主路径）。
4. **托盘文件夹子菜单重建 churn**：`extraFolders` 变化时重建子菜单，频繁改设置时可能有菜单闪动（低概率）。
5. **READY gating 边界**：前端永不 ready 的 3 s 兜底会显示 click-through 面板；ready 前顶边/隐藏状态等极端组合未全量运行验证。
6. **剪贴板 settle 启发式**：120 ms 延迟 + 3 次重试修复了观察到的 owner 竞态，但极慢的 owner 仍可能在「晚到的 veto 标记」前被接受（低概率）。
7. **目录 watcher**：非零部分写入文件可能被摄取；隐藏属性未检查；手改 JSON 时 `extraFolders` 加载不封顶（运行时添加有上限与校验）。
8. **热键冲突提示**：回滚后旧失败提示可能短暂残留；冲突错误文案为英文原文（`⚠ Shortcut unavailable…`）。
9. **几何写入非原子**（两个线程写窗口几何；未观测到故障）；**OLE 手写 COM vtable 绑定 `windows` 0.62 布局**，升级 `windows` crate 必须重跑 3 条 ignored 拖拽证据测试。
10. **开发日志轮转**（40 KB dev / 100 KB 某些轮次）可能截断证据日志；release 日志级别更低，不受影响。
11. **崩溃残留**：进程在预留文件名后崩溃可能留下 0 字节 PNG（启动只清理冻结帧残留）。
12. **26 dp 叉按钮在 150% 屏的物理热区与触摸命中**未实测。

---

### 6.4 全分支终审遗留（post-tag backlog）

2026-11-02 的三份独立全分支终审（Rust 核心 / 前端与 IPC / 文档与发布）结论：**Rust 核心与前端 TAGGABLE**；文档侧发现的隐私与口径问题已在本轮修复。以下为**非阻塞**遗留项，进入 `v0.1.0-core` 之后的 backlog（P2–P5 或 V1.1 处理）：

1. **`keepOnLine` 只影响显示窗口**，不裁剪内存/`state.json`：保留 25 时第 26+ 张仍会持久化但不可见（`AppState::add_item` 无 trim）。属 P4 语义补全。
2. **面板直接加载原图**（无 320 px 缩略图缓存/`hangline://thumb`）：大量 4K 卡片时内存可能超过 spec ≤150 MB 目标。P2/P5 改用缩略图协议或落盘时降采样。
3. **原生 WebView2 右键菜单未屏蔽**（面板与改键窗口；capture 覆盖层已屏蔽），右键会出 Edge 菜单。V1.1 做卡片右键菜单时一并处理。
4. **失败反馈与本地化**：复制/打开/取下线的拒绝仅 `console.warn`；热键冲突文案是 Rust 英文；capture 失败 MessageBox 为硬编码中文。P2/P4 统一为 i18n toast。
5. **发布前清理项**：`drag_demo_file` 仍注册在 release handler 中、`get_scale_factor`/若干 IPC 导出已无调用方、多处 `allow(dead_code)` 现在盖在活路径上。P5 发布前删除。
6. **capture 细节**：回车确认未实现（松开可用）；overlay 显示到 webview handshake 之间无 Rust 侧 watchdog（Alt+F4 可退出）；挂起/恢复按顺序而非 generation 匹配（极端交错）。P2/P4 处理。
7. **健壮性小项**：`any_hit` 未做溢出安全（异常 IPC rect 在 debug 下可 panic 命中线程）；`extraFolders` 规范化不一致（尾斜杠可造成双 watcher）；已移除监听目录的旧卡片重启后缩略图回退占位；i18n 仍有 3 个脚手架 key；`check-i18n` 不校验 `t()` 字面量；Panel↔GlassCard 手势仲裁无自动化测试。
8. **契约/文档**：`finish_capture` 返回 `()` 与 spec §4.4 的 `Result<Item>` 不一致（item 经 `state` 事件到达）；P5 更新 spec 或命令返回值。
9. 更细的清单见各任务报告与三份终审记录（`.superpowers/sdd/2026-10-29-hangline-windows-core/task-17-report.md`）；本表只保留需要在后续里程碑处理的主要项。

## 7. 隐私与安全声明

### 7.1 网络行为

- **唯一网络请求是更新检查**（P5 已接入 `tauri-plugin-updater`）：`autoCheckUpdates` 开启时每日一次，或从托盘手动检查；只请求配置的更新端点（当前为 GitHub Releases 占位地址），不会先下载，用户选择安装后验签、passive 安装并重启。没有任何遥测、统计、账号、云同步、崩溃上报 SDK。
- 其他「向外」的动作只在用户主动点击时交给系统执行：更新提示/文档中的链接用默认浏览器打开；「打开截图文件夹」打开资源管理器。
- P1 时期本节写作“零网络请求 / 不编译 HTTP 客户端”；自 P5 起以本节为准。P1 实验文档中的旧观察仅代表当时构建，不再适用于 1.0.0。

### 7.2 本地数据

- 数据目录：`%LOCALAPPDATA%\Hangline\`（`settings.json`、`state.json`、`Screenshots\`）；日志：`%LOCALAPPDATA%\app.hangline.Hangline\logs\`。截图与录屏**只留本地**；Hangline 只把剪贴板图片、显式监听目录中的新文件复制进自有 `Screenshots\`，不上传。
- 监听目录是**只读观察**（`notify` 文件事件）；不修改、不删除被监听目录中的文件（拖拽到文件夹的 MOVE 由 shell/Explorer 执行）。
- 剪贴板采集有隐私过滤：拒绝文本/文件格式与 `ExcludeClipboardContentFromMonitorProcessing`、`CanIncludeInClipboardHistory=0`、`org.nspasteboard.ConcealedType`、`Clipboard Viewer Ignore` 等标记（`P1-clipboard.md` §3 #5–#10、§6）。
- 截图目录权限：Windows 无 POSIX `0600` 对应；文件继承 `%LOCALAPPDATA%\Hangline\Screenshots` 的「当前用户（+SYSTEM/Administrators）」ACL（`P1-clipboard.md` §9 偏差 4）。
- 日志/状态文件可含本地文件路径与事件元数据；**不含图像内容**，仅本机保存。

### 7.3 安全姿态（当前）

- 无提权、无服务、无驱动；开机自启只用 HKCU `Run`（per-user，无 UAC）。
- 单实例；资产协议（asset protocol）scope 限制在自有目录 + 已监听目录（`P1-panel-ui.md` §9），CSP 为 `default-src 'self'`。
- **未签名/无自动更新**（P5 范围）；未来安装包需在发布说明中提示 SmartScreen 警告与 SHA-256。

### 7.4 证据隐私

- 仓库内提交的截图**只含 Hangline 自身 UI、中性测试内容或已脱敏图像**：面板/托盘/改键窗口来自 CDP 或精确裁剪的 Win32 菜单窗口；capture 覆盖层证据为**中性纯色测试窗口内的裁剪**（`P1-capture.md` §4.1）。P0 证据已做隐私处理：P0-E1E2 的两张桌面截图因包含无关浏览器/桌面内容**已从仓库移除**；P0-E4 的 Explorer 截图已裁剪掉私人文件夹导航栏；P0-E5 接缝图的右侧壁纸已**替换为中性灰**（左黑/接缝位置保留）。`P0-report.md` 与相关实验文档已同步更正。
- **冻结帧全屏截图、未脱敏桌面截图、远程会话 ID、微信/聊天内容均未提交**。**注意**：本仓库尚无 remote，但 git 历史的前期提交中仍保留被移除原图的 blob；**公开/推送前必须重写历史**（如 `git filter-repo`）才能彻底清除，当前 `v0.1.0-core` tag 仅用于本地源码里程碑。
- 各实验文档的清理记录（settings 哈希恢复、state/临时文件删除、日志回滚）见对应 `docs/experiments/P1-*.md` §清理。Task 17 本轮质量门未修改用户 `settings.json`（SHA-256 仍为 `8E315C7B…A79089`）。

---

## 8. 下一里程碑与「V1 core」的含义

- **P1（本里程碑，`v0.1.0-core`）**：三/四来源上栏、自建截图覆盖层、面板展开收起/容量/滚动/多屏跟随、复制/打开/拖出/取下线、托盘与设置持久化、持久化与去重、i18n en/zh-Hans。**不包含**最终视觉、标注、录屏裁剪、设置窗口、全屏抑制、安装包与发布。
- **P2 —— 视觉灵魂**：玻璃/夹子/绳的物理摆动、飞入/坠落动画、深浅色与逐帧对比、60 fps（面板 ≤12 卡）。P1 的卡片/绳是功能稿，P2 才是最终美术。
- **P3 —— 标注与录屏**：长按标记编辑器（画笔/矩形/箭头/文字/裁剪/撤销）、Game Bar 录屏卡片首帧与时间轴裁剪（M12/M13）。
- **P4 —— 设置与全屏**：完整设置窗口、改键 UX、开机自启/声音完善、全屏抑制（M4）、多屏/DPI 边界（M2/M3）复核。
- **P5 —— 发布**：NSIS 安装包 + portable、代码签名策略、自动更新、11 语言、README/press kit、CI/发布流水线；干净机器「安装→使用→升级→卸载」全链路验收。

**「不是 V1 core」的意思**：P2–P5 的功能/美术/发布事项都不在 `v0.1.0-core` 的承诺范围内；本里程碑只承诺「核心闭环以源码形式可用、程序化验证完成」。在 §5 人工清单闭合前，本里程碑也**不承诺**「日常可替换现有截图习惯」的最终用户级验收结论。

---

## 附录 A：证据索引

| 内容 | 位置 |
| --- | --- |
| P0 技术门禁报告（E1–E6、条件 a–f） | `docs/experiments/P0-report.md` |
| P0 E1/E2 透明面板与点击穿透 | `docs/experiments/P0-E1E2.md` |
| P0 E3 热键/托盘/单实例 | `docs/experiments/P0-E3.md` |
| P0 E4 原生拖出 | `docs/experiments/P0-E4.md` |
| P0 E5 多屏 GDI 截屏 | `docs/experiments/P0-E5.md` |
| P1 目录监听与去重 | `docs/experiments/P1-dir-watcher.md` |
| P1 剪贴板监听与隐私过滤 | `docs/experiments/P1-clipboard.md` |
| P1 面板静态 UI（绳/夹子/玻璃卡片） | `docs/experiments/P1-panel-ui.md` |
| P1 展开/收起状态机与 spring | `docs/experiments/P1-reveal.md` |
| P1 复制/打开/回收站与卡片手势 | `docs/experiments/P1-copy-trash.md` |
| P1 拖出语义与 prune | `docs/experiments/P1-drag-prune.md` |
| P1 容量/滚动/尺寸档/多屏跟随 | `docs/experiments/P1-capacity-scroll.md` |
| P1 托盘/设置/热键/READY gating | `docs/experiments/P1-tray-settings.md` |
| P1 自建截图全流程 | `docs/experiments/P1-capture.md` |
| 设计文档 | `docs/superpowers/specs/2026-10-28-hangline-windows-design.md` |
| 实现计划 | `docs/superpowers/plans/2026-10-29-hangline-windows-core.md` |
| 许可 / 第三方声明 | `LICENSE`、`THIRD_PARTY_NOTICES.md` |
| 各任务实现报告（含剩余风险明细） | `.superpowers/sdd/2026-10-29-hangline-windows-core/task-*-report.md`（本地、未入库） |

## 附录 B：本轮质量门命令与日志位置

```powershell
pnpm check
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --all-targets --manifest-path src-tauri/Cargo.toml
pnpm tauri build --no-bundle
node scripts/version.mjs
```

日志（临时，不提交）：`%TEMP%\hangline-t17-gates\{pnpm-check,cargo-test,cargo-check-all-targets,cargo-clippy,tauri-build-no-bundle}.log`。

---

## 附录 C：人工验收完成记录（2026-11-09）

> 本附录用于闭合 §1「人工验收：未执行」的阻塞状态，**不修改**前文的程序化证据与历史 `MANUAL-PENDING` 标记。

- **执行人**：项目维护者（仓库所有者）。
- **执行时间**：2026-11-09（同日另有 AI 自动化真机验收，见 [`W0-real-machine-2026-11-09.md`](W0-real-machine-2026-11-09.md)）。
- **结论**：维护者确认「验收完毕」；未报告失败项，按通过归档。
- **覆盖口径**：M1 / M5 / M6 / M8 / M9 / M10 / M11 / M16 与条件 a–e 中涉及真实物理输入、真实应用、真实系统工具的交互子项，均在本机解锁桌面上执行确认。
- **仍保留的已知限制**（不因验收通过而消失，见 §6.2）：HEIC 缩略图占位、仅 `CF_BITMAP` 的旧式复制不采集、浏览器「复制图片」按设计过滤、DRM/受保护窗口黑屏、Windows 10 22H2 未实测、产物未签名（正在申请 SignPath Foundation 免费签名）。
- **回归条件**：后续版本若改动截图 / 剪贴板 / 拖拽 / 多屏 / 标注代码路径，按 [`W0 人工验收剧本`](../product/2026-11-09-wave0-manual-acceptance-runbook.md) 复跑对应用例，并在本附录追加新记录。
