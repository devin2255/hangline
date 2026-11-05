# P0 门禁报告：Hangline Windows 技术验证（E1–E6）

| 项 | 值 |
| --- | --- |
| 日期 | 2026-10-29 |
| 分支 / 基线 | `feat/core`，Task 2–5 提交 `a0bde49` … `8d5efd1` |
| 汇总输入 | `docs/experiments/P0-E1E2.md`、`P0-E3.md`、`P0-E4.md`、`P0-E5.md`；Task 2–5 实现报告（`.superpowers/sdd/2026-10-29-hangline-windows-core/task-N-report.md`，未提交）；controller 评审记录 |
| 评审状态 | Task 2/E1E2、Task 3/E3、Task 5/E5 评审通过；Task 4/E4 经 1 轮修复后通过（3 Important 已修，0 Critical）；均为 controller 裁决，本报告只引用 |
| 判定口径 | **PASS**＝判据由程序化证据闭合；**CONDITIONAL PASS**＝程序化部分通过、但仍有明确 MANUAL-PENDING（不得当作 PASS）；**FAIL**＝判据被证伪 |
| 结论 | **GO for P1 development**（门禁条件 a–e + 延后复核 f，见 §3.2）；**Electron 兜底未触发** |

> 本报告只汇总既有证据，不新增实验。汇总时对已提交的证据文件做了完整性复核：E5 提交的左屏 PNG 与脱敏后的接缝 PNG 的 SHA256 与 `P0-E5.md` 记录一致（接缝图右侧壁纸已脱敏），`P0-E5-app-report.json` 为 3,724 bytes；其余证据为实验文档中的运行时观测与日志摘录。原始 `app*.log`、全屏截图、临时 harness 脚本留在 `%TEMP%`，未提交（体积 + 锁屏/会话隐私），仓库内保留裁剪图、原始 JSON 与文档摘录。**口径说明**：§1 的 GPU 与 DPI-unaware `[System.Windows.Forms.Screen]` 数值是汇总时在本机读取的（实验文档未记录 GPU）；E5 的两张约 5 MB 完整截图原图（`Hangline 2026-10-29 13.12.25.png`、`Hangline 2026-10-29 13.16.00.png`）由 `screenshots_dir()` 写到 `%LOCALAPPDATA%\Hangline\Screenshots`，**未提交**（体积 + 锁屏壁纸内容，见 `P0-E5.md` §5.2/§6）；提交进仓库的是两张裁剪图——`P0-E5-seam-x0.png`（右侧脱敏版，1,490 B，640×360）与 `P0-E5-left-monitor-150pct.png`（42,778 B，1920×1080），由 harness 写到 **`%TEMP%\hangline-e5`**（见 `P0-E5-app-report.json` 的 `crops[].path`）后复制进仓库。逐实验的完整「期望 vs 实际」表见各源文档 §3（`P0-E1E2.md`/`P0-E3.md`/`P0-E4.md`/`P0-E5.md`）；本报告 §2 只做汇总与判定，不复制全表。

## 1. 环境

| 项 | 值 |
| --- | --- |
| 操作系统 | Windows 11 家庭版 中文版 **10.0.26300（26H2）**，UBR **9711** |
| 会话 | **RustDesk 远程桌面**；屏幕顶部 0–24 px 是远端会话标题栏（含会话 ID）。含无关桌面/浏览器内容的证据图已移除或裁剪/脱敏：P0-E1E2 截图已移除、P0-E4 已裁掉导航栏、P0-E5 接缝图右侧已脱敏 |
| 锁屏状态 | E1/E2 与 E3（10:55–11:03）在解锁桌面执行；**E4 自 11:28 起、E5 全程桌面锁屏**（`LockApp` / `LockScreenBackstopFrame`），OS 级鼠标/键盘注入停在锁屏输入桌面 |
| GPU | Intel(R) Iris(R) Xe Graphics，驱动 32.0.101.7088（单适配器；实验文档未记录 GPU，此值于汇总时在本机读取） |
| 主屏 | `\\.\DISPLAY1`：`(0,0)` 1920×1080，96 DPI（100%），工作区 1920×1032（底部任务栏 48 px = 48 dp） |
| 左副屏 | `\\.\DISPLAY2`：`(-1920,0)` 1920×1080，144 DPI（150%），工作区 1920×1008（底部任务栏 72 px = 48 dp） |
| 虚拟屏幕 | `(-1920,0)` **3840×1080**（物理像素） |
| DPI 感知 | 应用进程 Per-Monitor V2（实测 `dpi_awareness=2`）；DPI-unaware 进程会把 150% 屏读成 1280×720@96（E5 §5.6 对照，汇总时用 `[System.Windows.Forms.Screen]` 复现为 1280×720） |
| WebView2 | 156.0.4314.50 |
| Node / pnpm | 22.17.1 / 10.30.3 |
| Rust / Tauri | rustc/cargo 1.90.0（MSVC）/ tauri **2.12.5**（wry 0.57.1、tao 0.37.2） |
| 构建 | **debug（dev profile）**：`pnpm dev`（Vite :5173）+ `cargo build`；**未验证** release / NSIS 安装包 |

### 1.1 运行方式与 agent 沙箱限制（影响所有实验的可复现性）

- **`pnpm tauri dev` 在本 agent 沙箱内无法启动**：workspace 内的 exe 不能写 `%LOCALAPPDATA%`，log 插件初始化即 panic：
  `error while running Hangline: PluginInitialization("log", "拒绝访问。 (os error 5)")`。
  同一二进制复制到 `%TEMP%` 后正常运行（E1/E2 §2.1 的对照表：workspace 内 `.runtime-test`、`src-tauri\target` 均失败；`%TEMP%`、`%LOCALAPPDATA%`、`Documents` 均成功）。这是 **agent 沙箱对 workspace 内可执行文件的写限制**，不是 Hangline 缺陷；但「精确的 `pnpm tauri dev` 启动路径」仍未在普通桌面验证（条件 c）。
- 因此 E1–E5 的运行时证据均采用等价方式：`pnpm dev` + 复制到 `%TEMP%` 的 **同一 debug 二进制**；E5 的 ignored 测试二进制同样复制到 `%TEMP%` 运行。
- **锁屏是 E4/E5 人工项的根因**：锁屏窗口位于同一 Default 桌面最上层，实测即使创建 TopMost 窗口，`WindowFromPoint` 仍返回 `LockScreenBackstopFrame`，`SendInput`/`SetCursorPos` 被锁屏接收。解锁桌面之前，任何「Explorer 收到文件」「副屏有真实内容」的说法都无法诚实成立。

## 2. 六项实验

> 文档映射：4 份实验文档覆盖 6 项判据——E1/E2 合并于 `P0-E1E2.md`，E3 与 E6 合并于 `P0-E3.md`（spec §7.3 单列 E6，计划 Task 3 标题只写「E3」，没有独立的 P0-E6 文档）。本报告按 spec 的 E1–E6 拆分汇总。

### 2.0 汇总表

| 实验 | 判据（spec §7.3） | 程序化已验证 | 仍 MANUAL-PENDING | 判定 |
| --- | --- | --- | --- | --- |
| E1 透明置顶面板 | 无黑底/白底；卡片可点，其余区域穿透到桌面 | 窗口形态、透明（像素差分 0）、卡片点击不抢焦点、透明区命中路由到下层、隐藏无残留 | 真实点击穿透；首帧白闪；任务栏不显示；150% 主屏 / 生产构建 | **CONDITIONAL PASS** |
| E2 点击穿透轮询 | 进出卡片切换 `ignore_cursor_events` 无可感延迟/闪烁 | 进入 25–41 ms、移出 29–36 ms；仅状态变化时调用；4 dp 外扩单测 | 肉眼闪烁/延迟；150% 屏手感 | **CONDITIONAL PASS** |
| E3 全局热键 | 注册/注销/改键/冲突回滚正常；不抢焦点 | 默认热键、注入触发、冲突保留旧键、改键回滚、ready 后显示且不抢焦点、隐藏时 CPU≈0 | 真实物理键盘；真实任务栏托盘点击；冲突气泡；真实冲突软件；release | **CONDITIONAL PASS** |
| E4 原生拖出 | 拖到资源管理器/微信得到副本；取消时飞回 | OLE `DoDragDrop` DROP/CANCEL、`CF_HDROP` 精确路径、源文件不变、`drag-ended`、看门狗 | 真实物理鼠标捕获；Explorer/桌面/微信/Figma/Chrome 投递；解锁桌面取消 | **CONDITIONAL PASS** |
| E5 自建截屏 | 多屏 + 150% 框选正确；PNG alpha 正确 | 双屏 3840×1080、8/8 像素、跨进程字节一致、跨接缝裁剪、原子写盘、4 dp 换算 | 解锁后副屏真实内容；交互式跨屏框选（Task 16）；DRM；负 y 屏 | **CONDITIONAL PASS** |
| E6 托盘与单实例 | 二级启动不产生第二个进程 | 第二次启动进程立即退出、仅 1 个实例、首实例 toggle + `second-instance` | 无（单实例判据）；托盘真实图标点击计入 E3（条件 e） | **PASS（单实例判据）** |

> 表内「仍 MANUAL-PENDING」列只列未闭合项（门禁闭合项与延后项混排，未逐项区分）；每项归属见 §2.x 的「门禁闭合项 / 延后项」与 §3.2(f)。

**结论速览**：6/6 的程序化判据通过，**0 FAIL**；5 组人工项（E1/E2/E3/E4/E5）未闭环，但已按「**门禁闭合项**」（条件 a–e，映射到 §5 步骤 1–10）与「**延后项**」（归属 Task 10/15/16/17、P4、P5，见 §3.2(f)）分类登记——不存在「全部人工项都在 §5 清单里」的说法。人工项未闭环不阻塞 Task 7（GO 语义见 §3.1）。

### 2.1 E1 透明置顶面板

**程序化已验证**

- **窗口形态**：`GetWindowRect = 0,0,1440,210`（DPI 96、`scale_factor=1.0`），客户区即全窗口、截图无标题栏；初始/穿透态 exstyle `0x080C0138`（TOPMOST + NOACTIVATE + TRANSPARENT + LAYERED + WINDOWEDGE + ACCEPTFILES），交互态 `0x08040118`。tao 为无边框窗口保留 `WS_CAPTION` 样式位、通过 `WM_NCCALCSIZE` 去掉非客户区（tao 0.37.2 `event_loop.rs:2074`），不能单看 `GWL_STYLE` 判断边框。
- **无黑底/白底**：面板可见 vs `ShowWindow(SW_HIDE)` 的截图像素差分——未被卡片阴影覆盖的区域 **0 像素变化**（x=0..400, y=100..180 diff=0），卡片内 16894/16895 像素变化，卡片外仅阴影光晕区变化；对照（可见→再次可见）diff=0。
- **卡片可点且不抢焦点**：卡片中心 (720,100) 按下出现 `:active` 白色高亮环；`GetForegroundWindow` 前后均为 `0x110A24`，面板 `0x3211BC` 从未成为前台（`WS_EX_NOACTIVATE`）。
- **透明区命中路由**：交互态 `WindowFromPoint(720,100)` → `Chrome_RenderWidgetHostHWND`（root 为面板）；穿透态 `WindowFromPoint(200,100)`/`(720,100)` → RustDesk 窗口（root 不是面板）。这是 OS 用于点击路由的同一命中测试。
- **几何一致**：IPC 上报 `set_hit_rects: [PhysicalRect { x: 645, y: 48, w: 150, h: 104 }]`，与截图卡片像素范围 `x=645..795, y=48..152` 一致。
- **隐藏无残留**；首次 2160×315（1.5×）问题已由 `panel::create` 的 `set_size(LogicalSize(1440,210))` 修正（tao 多屏 DPI 边界 bug）。

**证据**：`docs/experiments/P0-E1E2.md` §3 表、§4 与运行时探针日志（`GetWindowRect`/`GetWindowLongPtr`/`WindowFromPoint`/`GetForegroundWindow` + Pillow/NumPy 差分）。两张桌面截图因含私人浏览器内容已移除（见 `P0-E1E2.md` 顶部说明）。

**MANUAL-PENDING**

- **门禁闭合项**（条件 a–e / §5 清单）：真实点击穿透到桌面/下层应用（当前只验证路由，未发真实点击）→ §5 步骤 4、条件 d；首帧白闪 → 步骤 2、条件 d；任务栏不显示（`skip_taskbar` 由 `ITaskbarList::DeleteTab` 实现，未截任务栏核实）→ 步骤 2、条件 d；150% 主屏 DPI 视觉（卡片位置/命中）→ 步骤 3、条件 d。
- **延后项**（**不属 P0 门禁**，归属后续里程碑，见 §3.2(f)）：跨屏面板放置 → Task 10；全屏应用之上置顶/全屏抑制 → P4（M4）；其他 GPU（本机仅 Intel Iris Xe 单适配器，无硬件可测）→ 发布前目标机复核（P5）；NSIS 生产构建 → release 冒烟 Task 17、NSIS 打包 P5。

**判定：CONDITIONAL PASS**。

### 2.2 E2 点击穿透轮询

**程序化已验证**

- 30 Hz（33 ms）轮询、`cursor_position()` + `outer_position()` 换算面板局部物理坐标、`any_hit(..., pad)`、仅在 `should_ignore != ignoring` 时调用 `set_ignore_cursor_events`。
- 切换时延（3 轮）：进入卡片 **25/25/41 ms**，移出卡片 **36/30/29 ms**，点击前测得 45 ms；`WS_EX_TRANSPARENT` 位同步切换。
- 4 px 外扩：纯函数单测 `any_hit((98,80), pad=4)=true`、`(95,80)=false`；运行时在卡片左边缘外 4 px 处为交互态（该边界测量受远程光标抖动影响，`P0-E1E2.md` 中 641/642/644 的数字略有出入，结论不变）。**Task 5 已把 4 物理 px 改为 `HIT_PADDING_DP = 4.0`**（100/125/150% → 4/5/6 px，单测 `hittest::tests::hit_padding_follows_the_panel_scale`），E2 的时延数字是在 100% 主屏、4 物理 px 下测得的。
- 隐藏时停止轮询：可见 4 s `TotalProcessorTime` +93.8 ms，隐藏 +0.0 ms（E3 实测）。

**证据**：`P0-E1E2.md` §3 行 5/6/8、§4；`src-tauri/src/hittest.rs` 与 `geometry::tests::hit_inside_and_outside`、`edges_are_half_open_and_negative_origins_work`、`no_rects_never_hits`。

**MANUAL-PENDING**

- **门禁闭合项**：肉眼是否可感延迟/闪烁 → §5 步骤 5、条件 d；150% 屏上 4 dp→6 px 的真实手感（可在步骤 3/5 一并观察）→ 条件 d。
- **延后项**：拖拽/右键菜单期间强制交互 → Task 10（**不属 P0 门禁**，见 §3.2(f)）。

**判定：CONDITIONAL PASS**。

### 2.3 E3 全局热键 + 托盘 + 单实例

**程序化已验证**

- **热键**：默认注册 `Ctrl+Alt+S`（capture）/`Ctrl+Alt+T`（toggle）；注入 `Ctrl+Alt+T` → `hotkeys: toggle line` + 面板隐藏/显示；注入 `Ctrl+Alt+S` → 只记 `capture requested` 并发 `capture-requested`，**无截图/无伪覆盖层**。
- **冲突与回滚**：helper 进程先占用 `Ctrl+Alt+S` → 启动日志 `[ERROR] failed to register capture shortcut ... HotKey already registered`，toggle 仍注册、应用正常启动、旧键保留、无 panic；临时 harness（已删除）验证「成功改到 Ctrl+Alt+Y → 再改 Ctrl+Alt+T 冲突 → Y 仍有效、S 失效」。
- **启动时序与焦点**：窗口 1690 ms 创建且 `IsWindowVisible=false`，前端 ready 后 3415 ms 才可见；热键/二次启动 toggle 前后 `GetForegroundWindow` 始终是启动终端，面板从未成为前台。
- **托盘**：日志 `tray: icon and menu created`；进程内存在 `tray_icon_app` 隐藏窗口；投递 `WM_USER_TRAYICON` + `WM_RBUTTONUP` 弹出**真实 Win32 菜单**（`#32768`）；左键路径 `WM_LBUTTONUP` → `tray: left click toggle line`；菜单首项 toggle、`Quit` 退出。
- **单实例**：第二次启动的进程立即退出（进程列表仅剩 1 个 hangline），首实例日志 `single-instance: second launch detected, toggling the panel` 并发出 `second-instance` 事件。

**证据**：`docs/experiments/P0-E3-tray-menu-visible.png`（Hide line）、`P0-E3-tray-menu-hidden.png`（Show line）；`P0-E3.md` §3（15 行预期 vs 观察）、§4.1–4.7（`app.log`/`app2.log`/`app3.log`/`app4.log` 摘录）、§4.6 CPU 采样。

**MANUAL-PENDING**

- **门禁闭合项**：真实物理键盘 → §5 步骤 6、条件 e；真实任务栏图标点击/双击/溢出区位置 → 步骤 7、条件 e；`pnpm tauri dev` 精确启动路径 → 步骤 1、条件 c；首帧白闪、150% DPI 视觉 → 步骤 2–3、条件 d（与 E1 共用观察）；真实冲突源（QQ/微信/Snipaste 等）的可观察行为 → 步骤 6 顺带记录（不要求全部安装）。
- **延后项**（**不属 P0 门禁**，见 §3.2(f)）：冲突气泡 UI（尚未实现）→ Task 15；显示后首击最多约 233 ms（200 ms idle + 33 ms 轮询）→ Task 10 门控；release/NSIS 生产构建 → release 冒烟 Task 17、NSIS 打包 P5。

**判定：CONDITIONAL PASS**。

### 2.4 E4 原生文件拖出

**程序化已验证**

- **API 事实**：Tauri 2.12.5 无出站文件拖拽 API——`start_dragging()` 无参且语义为「移动窗口」（`tao window.rs:493` → `WM_NCLBUTTONDOWN`+`HTCAPTION`）；`tauri`/`tauri-runtime`/`tauri-runtime-wry` 中 `CF_HDROP`/`IDropSource`/`IDataObject`/`DoDragDrop` **0 命中**。主路径改为**仓库内 OLE 拖源**，未新增 crate（仅显式化 3 个既有 `windows` feature）。
- **真实 `DoDragDrop` 模态循环**：专用 STA 线程 `hangline-ole-drag` 执行；断言 `QueryContinueDrag ≥ 1`、`GiveFeedback ≥ 1`；`DRAGDROP_S_DROP`（0x00040100）→ `dropped=true`；`DRAGDROP_S_CANCEL`（0x00040101）→ `dropped=false`、`effect="none"`；生产拖源（Interactive）按住左键 `S_OK`、松开 `DRAGDROP_S_DROP`。
- **`CF_HDROP`**：shell `IDataObject`（`SHCreateItemFromParsingName` → `BindToHandler(BHID_DataObject)`）与手写回退对象均被 `DragQueryFileW` 读到**精确源路径**；手写对象另做 `DROPFILES` 字节级断言（`pFiles=20`、`fWide=TRUE`、双 NUL）。
- **应用内端到端**：CDP 可信输入触发 >4dp 手势 → IPC → OLE；日志同时打印 `ipc thread ThreadId(1)` 与 `drag thread ThreadId(22|23)`，证明 `DoDragDrop` 不在 IPC/主线程；拖拽期间卡片 class 为 `panel__card--dragging`，`drag-ended` 后恢复。
- **源文件与窗口**：2303 bytes、320×200 PNG、SHA256 `ED18DC01…AF98E`、mtime 11:27:11 前后一致；面板 rect 始终 `(0,0)-(1440,210)`（未误用窗口移动 API）。
- **看门狗**：8 s 无任何 `QueryContinueDrag` → `WM_CANCELMODE` + Esc + 合成 `WM_MOUSEMOVE` 打断；grace 1 s 后 `force_end_session` 兜底；真实无输入回归测试 `watchdog_cancels_a_drag_loop_that_never_receives_input` 锁屏下通过（400 ms，`forced == false`，约 0.46 s）。

**证据**：`docs/experiments/P0-E4.md` §4.4/§9.2/§9.3/§10.3；`P0-E4-drop-target-empty.png`（已裁掉私人导航栏；Explorer 目标文件夹仍为 0 项——**诚实的负证据**，证明没有发生真实投递，见 §4.4）；测试名：`dragout::tests::ole_drag_loop_reports_drop_cancel_and_keeps_the_source`、`interactive_source_continues_then_drops_when_the_button_is_released`、`shell_data_object_serves_cf_hdrop`、`builtin_data_object_serves_cf_hdrop`、`watchdog_cancels_only_when_no_input_arrives_before_the_timeout`、`watchdog_wait_stands_down_on_input_and_cancels_without_it`、`drag_session_allows_one_drag_at_a_time_and_ignores_stale_guards`。

**MANUAL-PENDING**（`P0-E4.md` §5）

- **门禁闭合项**（条件 a / §5 步骤 8–9）：真实物理鼠标捕获（WebView2 → 专用 STA 线程 `DoDragDrop`，应出现系统拖拽光标、面板不跟随、卡片保持按下）；真实目标投递（Explorer 文件夹 / 桌面 / 微信 / Figma / Chrome）；解锁桌面的取消路径（**Esc 为主**；无接收处松手改用任务栏等已知非投放目标，桌面本身是有效投放目标）。**若物理捕获失败，回退为面板主线程 `DoDragDrop`（`run_on_main_thread`），该回退尚未实现**。
- **延后项**：150% DPI 矩阵行（`P0-E4.md` §5 表末行）→ Task 17 复测（**不属 P0 门禁**，见 §3.2(f)）。

**判定：CONDITIONAL PASS**（程序化 PASS；真实目标投递只到 MANUAL-PENDING，未升级为 PASS）。「取消时飞回」的**飞回验证属 Task 13**（弹簧基础在 Task 11），P0 只验证取消后卡片状态复位。

### 2.5 E5 自建截屏

**程序化已验证**

- **双屏枚举**（应用进程实测）：`DISPLAY2 (-1920,0) 1920×1080 scale=1.5 work=1920×1008`；`DISPLAY1 (0,0) 1920×1080 scale=1.0 work=1920×1032`；虚拟屏幕 `(-1920,0) 3840×1080`；`dpi_awareness=2`。
- **截屏**：PNG **3840×1080** RGBA；8 个采样点（每屏 4 点：角/边/中心 + 接缝两侧）与独立 `GetPixel` **8/8 逐通道相等**；alpha 全 255；`crop` 支持负坐标/越界钳制，左屏/主屏各 1920×1080、跨接缝 640×360。
- **跨进程确定性**：应用 13.16.00 与测试 13.12.25 写出的 PNG **逐字节相同**（5,117,272 bytes，SHA256 `2d16890c…d1330`，BLAKE3 `a722003e…9859`），三张裁剪哈希亦相同。
- **写盘**：`%LOCALAPPDATA%\Hangline\Screenshots\Hangline yyyy-MM-dd HH.mm.ss.png`，`.png.tmp` + `sync_all` + `rename`，失败清理；`dp_to_px` 4/5/6 @100/125/150%；PMv2 对照（unaware 进程读到 1280×720）。
- **汇总复核**：提交的 `P0-E5-seam-x0.png`（右侧脱敏版）SHA256 = `cab350c4…d376`、`P0-E5-left-monitor-150pct.png` = `564fe643…10add`，与 `P0-E5.md` §5.4 记录一致；`P0-E5-app-report.json` 3,724 bytes 与文档一致。

**证据**：`docs/experiments/P0-E5-app-report.json`（监视器、8 探针、PNG/裁剪哈希、`elapsed_ms=5111`）；`P0-E5-seam-x0.png`（左黑；右侧壁纸已脱敏为中性灰，接缝在图像 x=320）；`P0-E5-left-monitor-150pct.png`（锁屏副屏全黑 = 真实内容）；测试 `capture::tests::e5_real_session_monitors`、`e5_virtual_screen_capture_crops_and_pixels`、`save_png_writes_atomically_and_reloads`、`screenshots_dir_is_localappdata_hangline_screenshots`、`geometry::tests::dp_to_px_rounds_with_the_monitor_scale`、`converts_physical_to_logical_with_scale`。

**MANUAL-PENDING**

- **门禁闭合项**：解锁桌面后副屏的真实桌面内容 → §5 步骤 10、条件 b（当前纯黑是锁屏只在主屏绘制的真实结果，不是采集失败）；交互式跨屏框选/预览/Esc → 条件 b，Task 16 验收（不在 5 分钟清单内）。
- **延后项**（**不属 P0 门禁**，见 §3.2(f)）：DRM/受保护窗口（OS 行为；代码按契约返回图像、不把黑当错）→ P5 发布前以已知限制复核；负 y 显示器与顶部/侧边任务栏（仅合成单测）→ P4 多屏（M2/M3）；150% 屏上的面板放置 → Task 10；同秒连续截图覆盖 → Task 16 命名策略；asset protocol 加载自建截图 → Task 10/16。

**判定：CONDITIONAL PASS**。

### 2.6 E6 托盘与单实例

**程序化已验证**

- 第二次启动的进程立即退出，进程列表仅剩 1 个 hangline；首实例日志 `single-instance: second launch detected, toggling the panel` + 面板隐藏；同时发出 `second-instance` 事件；toggle 前后前台窗口不变。
- 托盘图标与菜单创建成功（`tray: icon and menu created`），真实 Win32 菜单可弹出并操作（与 E3 共用证据）。

**证据**：`P0-E3.md` §3 行 11、§4.5；`P0-E3.md` §3 行 7–10、§4.4 与两张菜单截图。

**MANUAL-PENDING**：无（E6 的 spec 判据是「二级启动不产生第二个进程」，已闭合）。E6 名称中的「托盘」部分，真实任务栏图标点击/双击/外观仍是人工项，计入 E3（§5 步骤 7、条件 e），不是 E6 的延后项。

**判定：PASS（单实例判据）**。

## 3. 门禁决策

### 3.1 决策：GO for P1 development

**口径来源（与 spec 对齐）**：spec §7.3 标题与 §8 P0 DoD 已由提交 `7cc8068` 修订为「程序化判据必须通过，人工项在 P1 验收前闭合」。§8 原文：「E1–E6 程序化判据全过；真实物理拖拽/解锁副屏/生产构建等人工项写入 P0 报告 §3.2 条件，在 P1 验收（Task 17）前闭合；任一程序化判据失败则改方案（兜底 Electron）。**GO 仅表示允许开始 P1 开发，不等于 P0 验收完成。**」计划 Task 6 Step 2 仍把 Electron 的触发条件收窄为 **E1/E2/E4 任一 FAIL**。因此 **MANUAL-PENDING ≠ FAIL**：人工项未做不等于程序化判据失败。当前 **0 FAIL**：

1. **E1/E2/E4 的程序化判据全部通过**。E4 唯一的「失败」是 Tauri 2.12.5 缺少出站文件拖拽 API——这是**实现层缺口**，已由仓库内 OLE 拖源替代并通过程序化验证（DROP/CANCEL、`CF_HDROP`、源文件保留）；平台本身支持 OLE/`CF_HDROP`，不构成 Windows 可行性失败。
2. **剩余缺口不是单一类型**：一部分是 agent 会话的观测能力边界（锁屏：E4/E5 的真实目标与副屏内容；RustDesk：无法做真实人眼视觉判断；沙箱：`pnpm tauri dev` 无法在 workspace 内启动），构成门禁闭合项 (a)–(e)；另一部分是明确归属后续里程碑的延后项（(f)：跨屏放置、全屏置顶、其他 GPU、150% E4 矩阵、release/NSIS 等）。两者都不构成对 Tauri 架构的反证；门禁闭合项 (a)–(e) 中，§5 清单覆盖的视觉/交互项可由用户在解锁桌面按清单在约 5 分钟（另加首次编译时间）内闭环——但 (b) 的交互式跨屏框选不在 §5 清单内，另依赖 Task 16 的交互式选择验收。
3. **门禁闭合项可在 P1 期间闭环**，不阻塞 Task 7 开工；但 (a)–(e) 必须在 **Task 17（P1 验收）前**全部关闭，(f) 的延后项在各自归属里程碑复核。

> 语义边界：**GO 只表示允许开始 P1 开发（Task 7），不等于 P0 验收完成**。在 (a)–(f) 全部处理完毕前，不存在「P0 验收完成」的完整声明：门禁闭合项 (a)–(e) 最迟 Task 17 前关闭，release 构建冒烟随 Task 17，延后项 (f) 其余部分在归属里程碑复核。在此之前不得对外宣称「P0 全过」「P0 人工项已全数覆盖」或「发布就绪」。

### 3.2 门禁条件与人工覆盖（(a)–(e) 在 Task 17 前闭环；(f) 为范围外延后项复核，含 P5 生产构建冒烟）

| # | 条件 | 关联实验 | 关闭方式 | 建议时点 |
| --- | --- | --- | --- | --- |
| a | **真实物理鼠标从卡片拖入 Explorer / 桌面 / 微信 / Figma，以及在解锁桌面取消**（**Esc 为主**；无接收处松手改用任务栏等已知非投放目标）；若物理捕获失败，先改面板主线程 `DoDragDrop` 再复测 | E4 | 用户按 §5 步骤 8–9 验收；证据写回 `P0-E4.md` §5 矩阵 | 尽快；最迟 Task 13 完成前 |
| b | **解锁后副屏真实内容采集** + **交互式跨屏区域框选** | E5 | 内容：§5 步骤 10（E5 ignored 测试）；框选：Task 16 验收 | 内容尽快；框选 Task 16 |
| c | **在普通桌面执行精确的 `pnpm tauri dev`**（沙箱内被 log 插件 `os error 5` 阻断） | E1/E2/E3 | 用户按 §5 步骤 1 验收，回报控制台输出 | Task 7 开工前（影响开发工作流） |
| d | **首帧白闪 / 任务栏不显示 / 150% 主屏 DPI 视觉检查 / 真实点击穿透 / 快速进出无可感延迟/闪烁**（§5 步骤 2–5） | E1/E2 | 用户按 §5 步骤 2–5 验收 | Task 10 前（面板几何基线） |
| e | **真实任务栏托盘点击（左键/右键/双击）与真实物理全局热键**（§5 步骤 6–7） | E3/E6 | 用户按 §5 步骤 6–7 验收 | Task 15 前 |
| f | **范围外延后项复核**（不阻塞 GO；其中 release 构建冒烟按 spec §8 随 Task 17）：见 §2 各实验「延后项」——E1 跨屏面板放置→Task 10、全屏应用之上置顶→P4、其他 GPU→P5；E2 拖拽/右键菜单期间强制交互→Task 10；E3 冲突气泡→Task 15、显示后首击约 233 ms→Task 10；E4 150% DPI 矩阵→Task 17；E5 DRM/负 y 多屏/同秒覆盖/asset protocol、150% 屏面板放置→Task 10；NSIS 生产构建（release 冒烟 Task 17、NSIS 打包/签名 P5） | E1–E5 | 不阻塞 GO；release 冒烟随 Task 17，其余在归属里程碑（Task 10/15/16/17、P4、P5）复核并写回源实验文档 | 各归属里程碑 |

**步骤 → 条件映射**：步骤 1→(c)，2–5→(d)，6–7→(e)，8–9→(a)，10→(b)；交互式框选按 Task 16 验收计入 (b)。§5 的 10 个步骤没有「无归属」的步骤。

**硬性要求**：**(a)–(e) 在 Task 17（P1 验收）前全部闭环**（spec §8 P0 DoD）；其中 (a)(c) 建议尽早，避免到 Task 13 才发现 OLE 物理捕获不可用而被迫换方案。**(f) 不阻塞 GO**；其中 spec §8 点名的「生产构建」以 **release 构建冒烟**形式随 Task 17 完成（不要求 P0 产出 NSIS 安装包，NSIS 打包/签名属 P5），其余项在归属里程碑复核并回写，否则不得宣称「P0 人工项已全数覆盖」。

### 3.3 兜底决策：Electron 未触发

**不切换 Electron。** 理由：E1/E2/E4 没有任何程序化 FAIL；E4 的 API 缺口已由仓库内 OLE 实现补齐并通过程序化证据（`DRAGDROP_S_DROP`/`DRAGDROP_S_CANCEL`、`CF_HDROP` 精确路径、源文件不变）。Electron 兜底仅在 §6 的条件下才重新评估。

## 4. 已知偏差与风险（carried forward）

| 偏差/风险 | 说明 | 带到 | 若判断错误的后果 |
| --- | --- | --- | --- |
| **OLE 只允许 `DROPEFFECT_COPY`** | 拖出恒为「复制」，源文件保留；移动/删除语义未实现（spec M10 期望拖到文件夹=移动 + 卡片 0.6 s 掉落） | **Task 13**（`after_drag` + `store::prune`） | 用户拖到文件夹后源文件仍在晾图绳上，M10 不通过；若让 Explorer 执行 MOVE，则与 prune/回收站语义冲突 |
| **拖拽看门狗在首次 `QueryContinueDrag` 后主动退出** | 只覆盖「一次输入都没有」的卡死；已收到输入后「按住不动」与「输入再次丢失」无法区分 | Task 13 / 产品决策（输入静默超时） | 拖拽线程与 Rust 会话可能挂到线程返回；`hittest` 暂停、新拖拽被拒；前端 30 s 兜底只恢复 UI，不取消 OLE 会话 |
| **前端 30 s 看门狗是表现层兜底** | 超时只清 `--dragging`，不取消拖拽 | 保留；Task 17 复核 | 合法的 >30 s 长拖拽会提前「弹起」卡片，纯视觉问题 |
| **面板固定 1440×210 dp** | 未按工作区宽度/多屏放置；E5 已交付 `monitors()`/`work_area_of()`/`to_logical()` | **Task 10** | spec §3.3 的工作区宽度/多屏放置不成立；150% 屏上面板宽 2160 px，超出 1920 屏宽 |
| **`GetDpiForMonitor` 依赖 PMv2** | 非 PMv2 调用者会拿到虚拟化 DPI（对照：unaware 进程把 150% 屏读成 1280×720@96） | **Task 10**（调用线程守卫） | 150% 屏几何/命中偏移、截图裁剪错位 |
| **`BitBlt(SRCCOPY\|CAPTUREBLT)` 会抓到面板自身** | 面板是 layered window，`CAPTUREBLT` 会把它包含进虚拟屏幕位图 | **Task 16**（截图前 `SW_HIDE` 面板，抓完恢复） | 自建截图把晾图绳/卡片拍进图里，用户看到「线」在截图里 |
| **E5 `screen_pixel` 把 `CLR_INVALID`/白色当失败** | 诊断辅助逻辑的边界：真实白色像素可能被误判为采集失败；`CLR_INVALID` 未单独区分 | Task 16（测试/诊断） | 仅误报，不影响截屏产物本身 |
| **`crop` 只接受图像相对坐标** | `crop()` 的 `rect` 是「图像相对像素」：虚拟屏全局物理坐标须先减 `virtual_screen_rect()` 的原点（`capture.rs` 文档注释）；E5 只在测试/应用 harness 里做过全图换算，截图浮层的框选坐标将直接来自虚拟屏全局坐标 | **Task 16**（新增 `crop_virtual` 或先减虚拟原点） | 双屏/负原点时裁剪错位：截到相邻屏内容或黑边 |
| **PNG 文件名精确到秒** | 同秒两次截图按规格原子覆盖（`.tmp` + rename） | **Task 16**（burst 命名策略） | 连拍丢帧，只剩最后一张 |
| **手写 COM vtable 绑定 `windows` 0.62 布局** | 为不新增 `windows-core` 依赖，`IDataObject`/`IDropSource` 手写 vtable | 升级 `windows` crate 时 | ABI 漂移 → 崩溃/未定义行为；升级必须重跑 3 条 ignored 拖拽证据测试 |
| **无拖拽图像；托盘双击 toggle 两次；显示后首击 ≤233 ms** | 体验项：未实现 `IDragSourceHelper`；双击=两次 button-up（净效果不变）；隐藏门控使 show 后最多约 233 ms 才恢复命中测试 | Task 13（拖拽图像）/ Task 15（托盘手势）/ Task 10（门控） | 拖拽只有系统文件图标；双击托盘无可见变化；面板刚显示时首次点击可能穿透 |
| **托盘/热键/二次启动早于前端 ready** | 窗口 1690 ms 创建、前端 ready 3415 ms；托盘与热键在窗口创建后即可触发，`second-instance` 事件当前无监听者；冷启动期间的 early toggle 可能丢失或与前端状态不同步 | **Task 15**（事件排队/ready 后 flush） | 冷启动时热键或二次启动的 toggle 丢失，面板已显示而前端仍认为隐藏 |

## 5. 人工验收清单（解锁桌面，约 5 分钟（另加首次编译时间））

**准备**：解锁桌面并保持双屏（主屏 100%、左副屏 150% 且有窗口内容）；关闭全屏游戏/视频；准备一个空文件夹用于拖放；微信/Figma 可选。每步记录「实际结果」，异常时附截图。

1. **启动（条件 c）**：在仓库根目录执行 `pnpm tauri dev`。
   *期望*：编译后出现顶部面板（1440×210 dp），控制台无 `PluginInitialization("log", ... os error 5)` panic。
   *回报*：是否成功启动；失败则贴报错原文。
2. **首帧 / 透明 / 任务栏（条件 d、E1）**：重启应用并盯住面板首次出现的瞬间；随后用 `Ctrl+Alt+T` 隐藏再显示；看任务栏与 `^` 溢出区。
   *期望*：无白闪/黑底，卡片外直接看到桌面或浏览器；任务栏没有 Hangline 窗口按钮（托盘图标可能在溢出区）。
   *回报*：闪的颜色/时长；任务栏是否出现窗口按钮；托盘图标是否可见。
3. **150% 主屏 DPI（条件 d）**：临时把主屏缩放改为 150%（设置 → 系统 → 显示 → 缩放），重启应用；看完改回 100%，**再重启应用一次**。
   *期望*：面板物理尺寸变为约 2160×315 px——可观察到面板**右侧约 240 px 超出 1920 屏宽、被屏幕右缘裁掉**（卡片/阴影可能显示不全），属 Task 10 前的已知限制；但无黑底/白底、卡片位置与点击命中正确。
   *回报*：是否有黑底/白底、命中错位、异常闪烁；右侧是否被裁切；恢复 100% 后是否已重启应用。
4. **点击穿透与焦点（条件 d、E1）**：在面板透明区（卡片左右）点击桌面图标或浏览器链接，再点卡片。
   *期望*：透明区点击落到下层应用；卡片可按下；前台窗口不会变成 Hangline。
   *回报*：是否有某次点击被面板吃掉；前台是否被抢。
5. **快速进出（条件 d、E2）**：鼠标快速来回划过卡片边缘 5 次。
   *期望*：按下态/穿透切换无可感延迟、不闪烁。
   *回报*：是否感到延迟或闪烁。
6. **真实热键（条件 e、E3）**：真实键盘按 `Ctrl+Alt+T` 两次，再按 `Ctrl+Alt+S` 一次。
   *期望*：第一次隐藏、第二次显示；`Ctrl+Alt+S` 不产生截图（当前只记日志，Task 16 接入）。
   *回报*：是否响应；是否被 QQ/微信/Snipaste 等占用。
7. **真实托盘（条件 e、E3）**：左键单击托盘图标；右键看菜单；再双击一次。
   *期望*：单击 toggle 面板；菜单为 `Show line`/`Hide line` + 分隔线 + `Quit`；双击净效果不变（会 toggle 两次）。
   *回报*：图标是否可见（溢出区）、菜单文案、双击是否异常。
8. **真实拖出（条件 a、E4）**：把卡片拖到资源管理器文件夹、再拖到桌面。
   *期望*：出现系统拖拽光标；目标出现 `hangline-e4-demo.png` **副本**（COPY-only）；源文件仍在；卡片恢复。
   *回报*：目标是否收到文件、源文件是否保留、卡片是否卡在按下态。
9. **取消（条件 a、E4）**：再拖一次，拖出面板后**按 `Esc`**（主路径）→ 不产生任何文件、源文件不变。若还要测「无目标松手」，请拖到**任务栏或一个已知不接受拖放的窗口**；**桌面是有效的 shell 投放目标**（步骤 8 就要求桌面出现副本），不能把桌面当「无接收处」。
   *期望*：不产生任何文件；卡片恢复；再次拖拽仍可启动。
   *回报*：是否有残留文件、卡片状态是否复位。
10. **副屏截屏（条件 b、E5）**：保持左副屏有窗口内容且桌面解锁，在仓库根目录运行：
    ```powershell
    cargo test --manifest-path src-tauri/Cargo.toml --lib capture::tests::e5_virtual_screen_capture_crops_and_pixels -- --ignored --nocapture
    ```
    *注意*：**agent 沙箱内的测试二进制写不了 `%LOCALAPPDATA%`（`os error 5`）**，需用复制到 `%TEMP%` 的二进制运行（见 §1.1）；每次运行会往 `%LOCALAPPDATA%\Hangline\Screenshots\` 写一张约 5 MB 的 PNG（3840×1080 RGBA），验收后按需清理。
    *期望*：生成 3840×1080 PNG；左半屏是真实桌面内容（不是黑）；接缝在 x=1920。
    *回报*：测试是否通过、PNG 路径、左屏是否有内容；失败贴输出。

## 6. 若门禁为 NO-GO

本门禁转为 NO-GO 的唯一触发条件是 **E1/E2/E4 在真实桌面复核中被证伪**（例如目标 GPU 上透明窗口黑底、点击穿透失效，或 OLE 物理捕获/真实投递无法工作且主线程 `DoDragDrop` 回退仍失败）。此时按计划 Task 6 Step 2 切换 Electron 兜底：保留现有 React/TS 前端（面板 UI、命中矩形上报、拖拽手势、`drag-ended` 状态机基本原样），把 Rust 侧服务改为 Node 侧实现——`BrowserWindow` 的 `setIgnoreMouseEvents` + 轮询替代 hittest，`screen` API + `desktopCapturer`（或保留 GDI 截屏的 native addon）替代 `capture.rs`，`globalShortcut`/`Tray` 替代热键与托盘，OLE 拖出（`IDataObject`/`IDropSource`/`DoDragDrop`）移植为 N-API 原生插件或 `koffi`/`node-ffi-napi` FFI 调用（仍需专用 STA 线程与模态循环）；随后停止本计划、向用户汇报，不进入 Task 7。若只有 E3/E5/E6 的个别项失败，**不触发** Electron，按具体缺陷修复或换实现处理。

## 附录 A：程序化证据复跑命令

```powershell
cargo test  --manifest-path src-tauri/Cargo.toml        # 34 passed, 5 ignored（Task 5 后）
cargo test  --manifest-path src-tauri/Cargo.toml --lib -- --ignored --test-threads=1 --nocapture
                                                        # 3 条拖拽/OLE 证据 + 2 条 E5 环境测试（E5 需解锁桌面/%LOCALAPPDATA% 可写）
cargo check --manifest-path src-tauri/Cargo.toml        # 0 warnings
pnpm check                                              # eslint / tsc / vitest(7) / i18n / version
```

> ignored 测试不是规避：拖拽的 3 条在锁屏桌面即可通过；E5 两条依赖真实双屏与 `%LOCALAPPDATA%` 写权限，需在普通桌面运行。测试数量为 Task 2–5 报告中的最终快照（Task 4 修复前为 18 passed/2 ignored，修复后 20/3，Task 5 后 34/5）。
