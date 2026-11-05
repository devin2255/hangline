# P1 拖出语义与拖拽后 prune 实验报告

- **任务**：Task 13「拖出语义与 prune 规则」（`COPY|MOVE` 拖源、`after_drag`、按 effect + 文件存在性决定卡片去留）
- **分支 / 基线**：`feat/core`，父提交 `30ebcf9`
- **日期**：2026-10-30
- **结论**：程序化判据全部通过。`DoDragDrop` 的允许 effect 为 `COPY|MOVE`（不含 `LINK`）；`drag-ended` 到达后前端调用 `after_drag(id, effect)`，Rust 在 600 ms 后按 `after_drag_action(effect, exists)` 决策：`move` 且文件仍在时每 400 ms 重试、最多 5 次（约 2 s）以容纳慢速/跨卷 shell 移动，仍存在则保留卡片；任何 effect 下文件已消失即 `prune`（由 `prune` 自行持久化/递增 revision 后广播）；`copy`/`link`/`none` 且文件仍在为 no-op。受控轮次（单实例、无二次启动）验证：拖拽期间 `data-dragging=true`；拖拽中从外部删除源文件 → 看门狗结束 OLE 会话（`effect=none, ok=false`）→ 前端自动 `after_drag` → revision 1→2、卡片与 DOM 同时消失、`state.json` 为 `[]`；对仍存在文件的 `after_drag` 为 no-op（revision 不变）；未知 id 安全返回；外部删除 watched 文件本身不会自动掉落，必须经 `after_drag` 后 revision 3→4。复核轮次（2026-10-30 11:40–11:42）另以程序化 effect 验证 move 重试、重试窗口内删除后 prune、copy no-op、none+missing 安全网（§10）。**真实微信/文件夹/桌面/回收站投递（M9/M10/M11）仍未人工验证，保持 MANUAL-PENDING，由 Task 17（P1 验收）负责执行；本任务不以程序化 effect 冒充真实投递，见 §6。**

> **证据口径**：本文件区分【运行观察】与【单元测试/接线】。真实 OLE 投递目标（Explorer 的 MOVE、回收站的删除、微信的 COPY）没有在本轮跑过，因此不写成 PASS；§6 明确 MANUAL-PENDING，并由 Task 17（P1 验收）负责真实目标验证。日志时间为 UTC，本地时间 = UTC+8。

## 1. 目的与范围

验证 Task 13 的实现与语义（设计 §4.6 / 2026-10-29 修订；2026-10-30 复核修复为 effect 驱动 + 慢移动重试）：

1. `dragout.rs` 对 drop target 同时提供 `DROPEFFECT_COPY | DROPEFFECT_MOVE`，且不提供 `LINK`；
2. 最终 effect 映射为 `DragEffect::{Copy,Move,Link,None}` 并继续放在既有 `drag-ended { ok, effect, sessionId, dropped, error? }` 中；
3. 纯决策表 `after_drag_action(effect, file_exists) -> AfterDragAction { Prune, KeepWaiting, Done }`；
4. `#[tauri::command] after_drag(id, effect)`：立即返回、约 600 ms 后按决策表执行——`Prune` 调 `AppState::prune()`，有移除才 `emit_state`；`KeepWaiting` 每 400 ms 重试、最多 5 次（约 2 s），耗尽后保留并记日志；`Done` no-op；未知 id 为 no-op（不启动线程）；未知 effect 记警告并按 `none` 处理；
5. 前端 `drag-ended`（取消与失败也包含）调用 `after_drag(draggedItemId, effect)`，id/effect 保留到事件被处理；30 s 看门狗在重置前也用最后一次 id/effect 结算；不因 pointerdown 或拖拽中 `state` 广播丢失；
6. `set_panel_busy` 与 `data-dragging` 在拖拽结束/看门狗超时后清理；`draggingId` 变化会触发 hit-rect 重报，避免 0.97 按下态留下过期点击矩形。

不在本任务：真实目标投递的 M9/M10/M11 人工验收、`take_down`/回收站语义（Task 12 未改动）、容量/滚动（Task 14）。

## 2. 环境与运行方式

| 项 | 值 |
| --- | --- |
| OS | Microsoft Windows 11 家庭版中文版，10.0.26300 |
| Rust / Node | rustc 1.90.0 / cargo 1.90.0；Node 22.17.1、pnpm 10 |
| App | Hangline 0.1.0（debug，`cargo build`），复制到 `%TEMP%\hangline-task13\hangline.exe` 运行 |
| 前端 | Vite 7.3.7 dev server（`http://localhost:5173`） |
| 显示器 | `\\.\DISPLAY1` 1920×1080 @100%，工作区 1920×1032，主屏 |
| 观测 | `%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log`、WebView2 CDP :9222、`get_app_state`/`after_drag` IPC、`state.json`、DOM `[data-item-id]` / `[data-dragging]` |
| 条目 | 自有目录 80×60 PNG（红/绿/蓝/白四象限，hash `f61282136532ebae`）；watched folder 40×30 teal PNG（hash `a9733f3cf5e651ed`） |

**沙箱 workaround（沿用 T2/T5/T7/T8/T10/T11/T12）**：

1. 备份 `%LOCALAPPDATA%\Hangline\settings.json`（SHA256 `8E315C7B…A79089`），把 `extraFolders` 临时指向 `%TEMP%\hangline-task13\watched`；
2. `pnpm dev`；`cargo build --manifest-path src-tauri/Cargo.toml`；exe 复制到 `%TEMP%\hangline-task13\hangline.exe`；
3. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*` 启动**单个**实例；
4. 目录 watcher 只处理启动后的新事件，因此启动后再把两张 PNG 复制进各自目录，得到 `Capture` 与 `WatchedFolder` 两条 item；
5. 面板由 watcher 的「新条目 peek」自动展开（2.5 s），CDP 在此窗口内产生按下 + >4 dp 移动以启动真实 OLE 拖拽；拖拽期间 Rust 的 `dragout::is_dragging()` 让 reveal 控制器不收起；
6. 关键节点（拖拽启动后）由 Node 脚本从外部删除源文件，脚本**不**调用 `after_drag`，以证明 prune 由前端 `drag-ended` 监听触发；
7. 所有状态判据使用 CDP `get_app_state` 与 DOM，不依赖截图人工判读。

**受控轮次**：

- **原始实现轮次** 2026-10-30 11:08:20–11:09:42（单实例；无 second-instance，日志连续）。更早一轮（10:55–11:01）用于取消清理观察，日志被后续启动截断，仅引用当时抓取的尾部；§4–§5 主表全部来自该轮次，记录的是修复前的 existence-based `after_drag(id)`。
- **复核修复轮次** 2026-10-30 11:40:33–11:42:17（单实例；`cargo build` 后的 exe，Vite 5173，CDP :9222）。该轮只向自有 `Screenshots` 目录写入新建 PNG，用 `get_app_state` 取得 item id，再经 CDP `invoke('after_drag', { id, effect })` 直接验证 effect 驱动策略；未改 `settings.json`、未做真实目标投递。证据见 §10。

## 3. 语义与决策表

### 3.1 OLE effect

`dragout::allowed_effects()` 返回 `DROPEFFECT_COPY | DROPEFFECT_MOVE`；`run_drag` 用它调用 `DoDragDrop`。drop target 自行选择：

| 目标 | 目标侧行为 | 源文件 | `drag-ended.effect` |
| --- | --- | --- | --- |
| 微信/浏览器等应用 | 取 `COPY` 数据（`CF_HDROP`） | 保留 | `copy` |
| 文件夹 / 桌面 | Explorer 执行 `MOVE` | 原路径消失 | `move` |
| 回收站 | shell 以删除语义完成（可还原） | 原路径消失 | 通常为 `move`，也可能由 shell 直接删除 |
| 取消 / 无目标 | 不投递 | 保留 | `none` |

app 本身不复制、不移动、不删除被拖出的文件；`take_down(deleteFile=true)` 的 `IFileOperation` 回收仍只属于叉按钮（Task 12）。

### 3.2 决策表（`dragout::after_drag_action`）

| effect | file_exists | after_drag_action | `after_drag` 命令行为 |
| --- | --- | --- | --- |
| Move | true | **KeepWaiting** | 600 ms 初检后每 400 ms 重试，最多 5 次（约 2 s）；文件消失则 prune，仍存在则保留并记 info（shell move 仍 pending） |
| Move | false | **Prune** | 调 `AppState::prune()`；有移除才持久化/bump revision/`emit_state` |
| Copy | true | **Done** | 一次存在性检查后 no-op，源文件与卡片保留 |
| Copy | false | **Prune** | 安全兜底：源文件已消失，卡片不能指向缺失文件 |
| Link | true | **Done** | 本拖源不提供 LINK，仅为完整性映射 |
| Link | false | **Prune** | 安全兜底（同上） |
| None | true | **Done** | 取消/无目标不构成 prune 信号 |
| None | false | **Prune** | 安全网：文件损失可能来自其他路径，卡片仍需下线 |

**命令与前端执行路径**：

- `after_drag(id, effect)` 立即返回，在 `hangline-after-drag` 线程里先 sleep 600 ms；effect 由 `drag-ended` payload 提供，未知 effect 记警告并按 `none` 处理；未知 id 在启动线程前 no-op。
- 决策表是纯函数（`dragout::after_drag_action`，8 组断言单测锁定），命令直接执行它，不再有「策略表 + existence 兜底」的双轨。
- `Move + exists` 的重试是**有界**的：耗尽后保留可读文件对应的卡片并记 info（明确说明 shell move 仍 pending，后续 watcher 事件会再 prune），不承诺更长的重试。
- `Prune` 调用作用于当前全表（brief 要求 `AppState::prune()`）；空 prune 不写盘、不 bump revision、不广播。
- 前端在拖拽开始时记录 `{id, effect:"none"}`，`drag-ended` 到达时补上 effect 再结算；30 s 看门狗在下一次 pointerdown 之前用最后已知 id/effect 结算（幂等），因此丢失 `drag-ended` 后文件消失仍会被 prune。
- 复核修复前，命令只按 existence 执行，旧策略表与命令行为的两处保守差异记录在**本节 §3.2**（不是 §7）；修复后命令直接执行上表，该差异已消除。

### 3.3 修复前的历史行为（§4–§5 证据口径）

§4–§5 的下表、时间线与日志来自**修复前**的受控轮次（`after_drag(id)` 仅 existence-based）。它们仍是有效的运行时证据（前端接线、看门狗取消、prune 链路），但 effect 驱动与慢移动重试的行为以 §10 的复核轮次为准。

## 4. 预期 vs 观察（修复前受控轮次，2026-10-30 11:08）

| # | 项 | 预期 | 观察 | 结论 |
| --- | --- | --- | --- | --- |
| 1 | 允许 effect | COPY + MOVE，无 LINK | `allowed_effects()` 单测通过；源码仅 `DROPEFFECT_COPY \| DROPEFFECT_MOVE` | PASS（单测/源码） |
| 2 | effect 名称映射 | copy/move/link/none 四值 | 既有 `drop_effect_names_cover_every_ole_bit` 通过；`DragEnded.effect` 仍为小写字符串 | PASS（单测） |
| 3 | 拖拽启动 + 按下视觉 | CDP 按下并移动 >4 dp 后 `start_drag`，`data-dragging=true` | 11:09:04 日志 `OLE drag start requested … session=1`；同秒 DOM `data-dragging="true"`、1 张卡；截图 `P1-drag-prune-dragging.png` | PASS |
| 4 | 拖拽中外部删除文件 | 不自动掉落 | 11:09:04 删除后至 11:09:12，`get_app_state` 仍 revision 1、items 1 | PASS |
| 5 | 拖拽结束（看门狗取消）→ 前端自动 `after_drag` → prune | 无需外部调用；卡片消失、revision +1、state.json 更新 | 11:09:12 `OLE drag ended after 8024 ms … effect=none … ok=false`；11:09:13 日志 `after_drag: dropped 1 card(s) … ["4d8e55…"]`；revision 1→2、items 0、DOM cards 0、`state.json` = `[]` | PASS |
| 6 | 文件仍存在时 `after_drag`（COPY 语义） | no-op，不删除文件、不递增 revision | 11:09:28 `after_drag(id)` 返回 **5 ms**；revision 3→3、items 1、cards 1；日志 `after_drag: no card dropped; … still has its file` | PASS |
| 7 | 未知 id | 安全返回，无状态变更 | 11:09:28 `after_drag("not-on-the-line")` 返回；revision 3→3、items 1；日志 `after_drag: item not-on-the-line is no longer on the line` | PASS |
| 8 | 外部删除 watched 文件 | 不自动掉落，需 `after_drag` | 删除后 2 s：revision 3、items 1；`state.json` 仍含该路径 | PASS |
| 9 | 文件缺失时 `after_drag` | 卡片掉落、revision +1、state.json 重写 | 11:09:42 命令返回 **4 ms**；revision 3→4、items 0、cards 0、`state.json` = `[]`；日志 `dropped 1 card(s) … ["46c66b…"]` | PASS |
| 10 | IPC 不阻塞 | 命令立即返回（等待在后台线程） | 5 ms / 4 ms 返回；对应 prune 日志在 ~600 ms 后出现 | PASS |
| 11 | 只广播新 revision | 先 prune（持久化/bump）再 `emit_state` | 每次 prune 只观察到 revision +1；无重复/回退 revision；DOM 与 `get_app_state` 一致 | PASS |
| 12 | `data-dragging` / busy 清理 | 拖拽结束清除，不永久占住面板 | 取消轮次（11:00:20 启动、11:00:28 看门狗结束）后 `data-dragging` 消失、卡片保留；随后第二次启动 toggle 日志 `tuck (Toggle) … busy=false` | PASS（补充轮次） |

## 5. 证据（修复前受控轮次）

### 5.1 受控轮次时间线（CDP 脚本输出，本地时间）

```json
{"dragStart":1,
 "probe":{"revealed":true,"top":27,"x":960,"y":83,"id":"4d8e55cfdc32aec5341aa46fa575bce9"},
 "timeline":[
   {"at":1793329743881,"step":"copied own image"},
   {"at":1793329744319,"step":"card visible","probe":{"revealed":true,"top":27,"x":960,"y":83,"id":"4d8e55cfdc32aec5341aa46fa575bce9"}},
   {"at":1793329744329,"step":"state before drag","revision":1,"items":1},
   {"at":1793329744974,"step":"native drag started","dragging":"true","shot":"…\\evidence-dragging.png"},
   {"at":1793329744974,"step":"source file deleted behind the app"},
   {"at":1793329753281,"step":"after drag-ended prune","revision":2,"items":0,"cards":0}],
 "final":{"revision":2,"items":0,"cards":0}}
```

即：卡片出现 → 1.1 s 内启动真实 OLE 会话 → 同刻外部删除 → 约 8.3 s 后（8 s 看门狗 + 0.6 s prune 窗口）状态已 revision 2、0 条目。**脚本从未调用 `after_drag`**，该调用来自前端 `drag-ended` 监听。

### 5.2 日志节选（受控轮次，UTC）

```text
[2026-10-30][03:09:03][notify::windows][TRACE] Event: path = `…\Task13 own image.png`, action = 1
[2026-10-30][03:09:04][hangline_lib::watchers::dirs][INFO] watchers: hung …\Task13 own image.png (Capture) 80x60 hash f61282136532ebae
[2026-10-30][03:09:04][hangline_lib::reveal][INFO] reveal: shown (reason NewItem, peek=true, items=1) at +43921 ms
[2026-10-30][03:09:04][hangline_lib::dragout][INFO] dragout: OLE drag start requested window=panel path=…\Task13 own image.png session=1 (ipc thread ThreadId(1))
[2026-10-30][03:09:04][hangline_lib::dragout][DEBUG] dragout: using the shell IDataObject (CF_HDROP via BHID_DataObject)
[2026-10-30][03:09:05][hangline_lib::watchers::dirs][DEBUG] watchers: cannot stat …\Task13 own image.png: 系统找不到指定的文件。 (os error 2)
[2026-10-30][03:09:12][hangline_lib::dragout][WARN] dragout: OLE drag loop received no input within 8000 ms; posting WM_CANCELMODE to drag thread 24632
[2026-10-30][03:09:12][hangline_lib::dragout][INFO] dragout: OLE drag ended after 8024 ms: session=1 ok=false dropped=false effect=none error=Some("the OLE drag loop received no input (locked or secure desktop?); the drag watchdog canceled the session") (drag thread ThreadId(33))
[2026-10-30][03:09:13][hangline_lib][INFO] after_drag: dropped 1 card(s) whose files are gone: ["4d8e55cfdc32aec5341aa46fa575bce9"]
[2026-10-30][03:09:24][hangline_lib::reveal][INFO] reveal: shown (reason NewItem, peek=true, items=1) at +… ms
[2026-10-30][03:09:28][hangline_lib][DEBUG] after_drag: no card dropped; item 46c66b2d89d1661e956c66e3769cd7df still has its file
[2026-10-30][03:09:28][hangline_lib][INFO] after_drag: item not-on-the-line is no longer on the line
[2026-10-30][03:09:42][hangline_lib][INFO] after_drag: dropped 1 card(s) whose files are gone: ["46c66b2d89d1661e956c66e3769cd7df"]
```

`03:09:04` 的第二条（`cannot stat`）是外部删除事件到达 watcher；watcher 不删除 item，session 仍保持到 `drag-ended`，随后由 `after_drag` 的 prune 移除。

### 5.3 运行时状态（CDP `get_app_state` 与 `state.json`）

| 节点 | revision | items | DOM cards | `state.json` |
| --- | --- | --- | --- | --- |
| 自有条目上栏后 | 1 | 1 | 1 | 含 `4d8e55…` |
| 拖拽中删除文件 | 1 | 1 | 1 | 含 `4d8e55…` |
| 看门狗 drag-ended → after_drag | **2** | **0** | **0** | `[]` |
| watched 条目上栏 | 3 | 1 | 1 | 含 `46c66b…` |
| 对仍存在文件调用 after_drag（含未知 id） | 3 | 1 | 1 | 不变 |
| 外部删除后调用 after_drag | **4** | **0** | **0** | `[]` |

`after_drag` 的返回耗时：存在文件 **5 ms**、缺失文件 **4 ms**（等待在 `hangline-after-drag` 线程，IPC 线程不阻塞）。

### 5.4 拖拽视觉证据

`docs/experiments/P1-drag-prune-dragging.png`（CDP `Page.captureScreenshot`，1920×210 透明面板）：单张 80×60 四象限卡片，拖拽会话进行中（同刻 DOM `data-dragging="true"`、`revealed=true`）。截图只证明 pressed 状态与卡片存在，不代表真实目标已接收文件。

## 6. 手动待验证（MANUAL-PENDING，Owner: Task 17）

以下三项必须在有真实鼠标/目标应用的桌面上人工完成；本轮**没有**跑通，不能标记 PASS，也**没有**以程序化 effect 冒充通过：

1. **M9 拖到微信（COPY）**：源文件保留、卡片保留。预期日志 `effect=copy`；`after_drag` 只做一次存在性检查（`Copy+exists → Done`），卡片保留、revision 不变。
2. **M10 拖到文件夹/桌面（Explorer MOVE）**：文件出现在目标目录、原路径消失、卡片掉落。预期 `effect=move`；`after_drag` 在 600 ms 初检后按 400 ms 重试（最多 5 次 ≈2 s），文件消失的那次重试触发 prune。若移除在 ~2.6 s 后仍未完成，卡片保留并记 info（watcher 后续再 prune）。
3. **M11 拖到回收站（shell 删除）**：卡片掉落，文件可在回收站还原。预期 `effect=move` 或 shell 直接删除；原路径消失，走与 M10 相同的重试/prune 路径。

**裁决**：controller 明确将 M9–M11 的真实目标行为留给 **Task 17（P1 验收）** 执行并更新证据；Task 13 的状态是「程序化判据通过 + M9–M11 MANUAL-PENDING」，不是 pass，也不是伪造 waived-pass。**未执行的原因**：本轮会话中的 WebView2 输入经 CDP 注入，OLE 的模态循环收不到该输入（日志明确为 “received no input … locked or secure desktop?”），`DoDragDrop` 只能由 8 s 看门狗取消；没有真实目标接收 `CF_HDROP`。为避免在共享桌面上注入 OS 级鼠标事件、干扰其他使用者，本轮没有做物理输入注入。

**程序化门禁证据**：`allowed_effects` 覆盖 COPY/MOVE；`after_drag_action` 决策表 8 组断言（Move/Copy/Link/None × exists）；`wait_for_after_drag` 的重试预算、单次检查、删除后 prune、item 中途离场等 7 个单测；§10 的 CDP 运行观察（move 重试 5 次后保留、重试窗口内删除后 prune、copy no-op、none+missing 安全网）。这些**不**证明 Explorer/微信/回收站的实际投递行为，真实目标由 Task 17 负责。

## 7. 单元测试与门禁

| 门禁 | 结果 |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml` | **143 passed; 0 failed; 5 ignored** |
| `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | 0 warnings |
| `pnpm check` | eslint、tsc、vitest（7 files / 37 tests）、i18n、version 全通过 |

复核修复新增/更新的测试：

- `dragout::tests::after_drag_action_follows_effect_and_existence`（8 行决策表，Move/Copy/Link/None × exists）
- `dragout::tests::drag_effect_names_round_trip_and_unknown_names_are_rejected`（`copy/move/link/none` 解析；未知名返回 None）
- `tests::after_drag_parses_known_effects_and_falls_back_to_none`（未知 effect → `None`）
- `tests::after_drag_retries_a_slow_move_until_the_file_disappears`（前两次 exists、第三次消失 → Prune）
- `tests::after_drag_keeps_the_card_when_the_move_is_still_pending`（1 次初检 + 3 次重试后 StillPending）
- `tests::after_drag_copy_is_decided_by_the_first_probe`（Copy+exists 只探测 1 次）
- `tests::after_drag_prunes_a_move_whose_file_is_already_gone`（Move+missing 不浪费重试）
- `tests::after_drag_none_missing_prunes_as_a_safety_net`
- `tests::after_drag_stops_when_the_item_leaves_the_line`（等待中 item 被其他路径移除 → ItemGone）
- `tests::after_drag_target_ignores_an_unknown_id`、`tests::after_drag_target_finds_the_dragged_item_by_id`（原名 `after_drag_*` 只测 `after_drag_target`，已按实际语义重命名）

原有测试保持通过：`store::tests::prune_drops_items_whose_files_are_gone`、`dragout::tests::drag_effects_offer_copy_and_move_but_never_link`、`app_state::tests::pruning_a_deleted_file_drops_it_bumps_the_revision_and_persists`。

## 8. 偏差与关注点

1. **前端拖出接线是 Task 13 补上的**：Task 10 移除了 P0 的拖出 demo（`P1-panel-ui.md` §8.4 记录真实条目拖出在 Task 13 接线），因此 `Panel.tsx` 原本既无 `start_drag` 也无 `drag-ended` 监听。本次在 `Panel.tsx` 以事件委托实现：卡面 pointerdown（排除叉按钮）→ 4 dp 阈值 → `start_drag(path)`；`data-dragging` 状态经 `GlassCard` prop 渲染；`drag-ended` 到达后清除 watchdog/busy 并把 `{id, effect}` 交给 `after_drag`。复核修复把“待结算的 id/effect”从 pressed 视觉里拆到独立 `pendingDrag` ref：pointerdown 只清 pressed 状态，不会丢掉延迟到达的 `drag-ended` 的 prune 目标；30 s 看门狗与开始下一次拖拽也会先结算它。
2. **effect 驱动（2026-10-30 复核修复）**：命令改为 `after_drag(id, effect)`；effect 来自 `drag-ended` payload，未知值记警告并按 `none` 处理。纯表 `after_drag_action` 驱动 `Prune/KeepWaiting/Done`；`Move+exists` 在 600 ms 后最多重试 5×400 ms（约 2 s），耗尽后保留可读文件并记 info（说明 shell move 仍 pending，watcher 后续会 prune）。`Prune` 才调用 `prune()`，且只有移除时才 `emit_state`。`should_prune` 与 `#[allow(dead_code)]` 已删除。
3. **`prune` 影响全表**：`after_drag` 的 `Prune` 分支调用 `AppState::prune()`（brief 要求），所以若同一初检/重试窗口内其他文件也消失，会一并掉落；空 prune 不写盘、不 bump revision。命令用 `AppState::item(id)` 判断未知 id；未知 id 不启动线程。
4. **`AppState::item(id)` 新增**：`lib.rs::item_by_id` 改为复用它，`copy_item`/`open_item` 语义不变（仍从当前 state 解析，不用前端快照）。
5. **`DragEffect` 含 `Link`**：brief 的接口行只列 `{Copy, Move, None}`，复核修复的决策表要求 `Link+exists → Done`、`Link+missing → Prune`（安全兜底）；实际枚举包含 Link（映射既有 `effect_name`），`allowed_effects` 永不提供 LINK，`drag-ended.effect` 的线格式不变。
6. **拖拽会话 token**：`drag-ended` 仍按 `sessionId` 去重；`after_drag` 的 id/effect 来自拖拽开始时捕获的 pending 记录，不读取“当前最新拖拽”，避免错过/串会话。
7. **日志轮转（复核轮次新增观察）**：`tauri-plugin-log` 默认 `max_file_size = 40_000` 且 `RotationStrategy::KeepOne`；11:40:35 的日志在 40 KB 处被删档重开，首轮 A（move+exists）的 retry 行因此丢失。随后用独立脚本重跑 A（`T13fix move keep2.png`）并在同一文件内保留 5 条 retry 行（§10.3）。历史轮次的 03:04:33 重开是 second-instance/重启造成；两处均不影响 CDP 状态判据。
8. **共享桌面干扰**：修复前受控轮次中仍出现卡片被浏览器输入管线复制的日志（`clipboard: copying … Task13 drag image.png`，按 CDP 按下/释放与桌面真实点击混合造成，来源不可区分），但未改变 item/revision，也不用于判据；收尾时剪贴板已重置为文本 baseline。复核轮次只做 IPC 调用与文件增删，未触碰剪贴板。
9. **未安装 rustfmt**：沿用现状，未运行 `cargo fmt --check`；代码按现有风格手写。
10. **hit rect 依赖 `draggingId`**：`dragging` 使卡片缩放到 0.97；复核修复把 `draggingId` 加入 hit-rect 上报的 layout 依赖，拖拽开始/结束都会重报矩形，不会留下按 1.0 尺寸计算的过期点击区域。

## 9. 环境恢复

**修复前受控轮次**：

- App 与 Vite 已停止；`state.json` 已删除（测试前不存在）。
- 测试图片 `Task13 own image.png`、`Task13 watched image.png`、`Task13 drag image.png` 已删除；`Screenshots` 恢复为测试前的两张 E5 PNG + `probe.txt`；watched 目录为空。
- `settings.json` 从备份恢复，SHA256 `8E315C7BA3B111186B9BE3C573FBC907FBB5C14C2453E363BD533CE4FCA79089`（与测试前一致，`extraFolders=[]`）。
- 剪贴板重置为文本 `Hangline Task13 clipboard baseline`；本轮未使用回收站，无新增回收站条目。

**复核修复轮次（2026-10-30 11:40–11:42）**：

- App 与 Vite 已停止；`state.json` 已删除（测试前不存在，收尾后为 `[]` 再删除）；`revision` 在收尾时无残留 item。
- 测试图片 `T13fix move keep.png`、`T13fix move gone.png`、`T13fix copy.png`、`T13fix none missing.png`、`T13fix move keep2.png` 已删除；`Screenshots` 恢复为两张 E5 PNG + `probe.txt`。
- 未改 `settings.json`（SHA256 仍为 `8E315C7BA3B111186B9BE3C573FBC907FBB5C14C2453E363BD533CE4FCA79089`）；未使用剪贴板/回收站。
- `%TEMP%\hangline-task13`（exe、测试图、CDP/证据脚本、`after-drag-*-result.json`）为临时证据，不提交；提交的运行时产物仍只有 `docs/experiments/P1-drag-prune-dragging.png`。

## 10. 复核修复轮次：effect 驱动 + 慢移动重试（2026-10-30 11:40–11:42）

### 10.1 目的与方式

复核发现：原实现把 `after_drag` 做成只有 id、后台 600 ms 后 existence-based `prune()`，`should_prune` 因此带 `#[allow(dead_code)]` 且命令不接收 effect；`Move + exists` 没有重试，慢速 shell 移动会留下指向已移走文件的卡片；pointerdown 还会清掉延迟 `drag-ended` 所需的 id。本轮按 spec §4.6 E / plan Task 13 Step 3 的 2026-10-30 修订修复后，按下列方式取证：

- `cargo build` 的 debug exe 复制到 `%TEMP%\hangline-task13\hangline.exe`，`pnpm dev`（Vite 5173），单实例，`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*`。
- 每条 item 来自自有 `Screenshots` 目录的新建 PNG（不同尺寸/颜色，避免内容去重）；用 CDP `get_app_state` 取 item id，再以 `window.__TAURI_INTERNALS__.invoke('after_drag', { id, effect })` 直接调用命令。
- 不注入鼠标、不做真实目标投递；状态判据 = `get_app_state` 的 revision/items + 文件存在性 + 应用日志；时间用 Node `Date.now()` 与日志 UTC 时间（同机）核对。
- 条目清理用 `take_down(id, deleteFile=false)`，不触发回收站；`state.json` 收尾后为 `[]` 并删除。

### 10.2 预期 vs 观察

| # | 场景 | 调用 | 预期 | 观察（本地 UTC+8 / 日志 UTC） | 结论 |
| --- | --- | --- | --- | --- | --- |
| A | move + 文件仍存在（慢移动） | `after_drag(id,"move")` | 600 ms 初检后 5 次重试（约 2 s），仍存在则保留；revision 不变；命令立即返回 | 命令返回 **5 ms**；`revision 11→11`、items 1、`onLine=true`、`fileExists=true`（调用后 +3.2 s 复核）；日志 03:42:15–17 连续 `retry 1/5…5/5 … exists=Some(true)`，03:42:17 `still has its file after the retry window … shell move is still pending` | PASS（程序化） |
| B | move + 重试窗口内删除文件 | `after_drag(id,"move")`，t+1213 ms 删除源文件 | 下一次重试发现缺失 → `prune`，revision +1、卡片消失 | 命令返回 **8 ms**；删除后 **220 ms**（调用后约 1.43 s，即 retry 2/5）观察到 `revision 5→6`、items 0；日志 03:40:37 `retry 1/5 … Some(true)`、`retry 2/5 … Some(false)`，03:40:38 `dropped 1 card(s)` | PASS（程序化） |
| C | copy + 文件仍存在 | `after_drag(id,"copy")` | 单次存在性检查后 no-op；revision 不变、卡片保留 | 命令返回 **17 ms**；1.5 s 后 `revision 7→7`、items 1、`onLine=true`、`fileExists=true`；日志 `no card dropped … (effect=copy); its file still exists` | PASS（程序化） |
| D | none + 文件缺失（安全网） | 先删除文件，再 `after_drag(id,"none")` | 600 ms 初检即 `prune`（安全网），revision +1 | 命令返回 **9 ms**；调用后 **679 ms** 观察到 `revision 9→10`、items 0；日志 `dropped 1 card(s)` | PASS（程序化） |

补充：主脚本 A 的另一条 item（`f81cca44…`）在调用后 +307/+1008/+1612/+2104/+3212 ms 采样均为 `revision 3`、items 1、文件存在，与 §5 的 600 ms 窗口一致；单独的 A 重跑（`a3c3f962…`）用于保留被 40 KB 日志轮转截掉的 retry 行。

### 10.3 日志节选（UTC）

```text
[2026-10-30][03:42:15][hangline_lib][DEBUG] after_drag: retry 1/5 for a3c3f9621eeeb5262d1186fc68f06abb (effect=move): file exists=Some(true)
[2026-10-30][03:42:16][hangline_lib][DEBUG] after_drag: retry 2/5 for a3c3f9621eeeb5262d1186fc68f06abb (effect=move): file exists=Some(true)
[2026-10-30][03:42:16][hangline_lib][DEBUG] after_drag: retry 3/5 for a3c3f9621eeeb5262d1186fc68f06abb (effect=move): file exists=Some(true)
[2026-10-30][03:42:17][hangline_lib][DEBUG] after_drag: retry 4/5 for a3c3f9621eeeb5262d1186fc68f06abb (effect=move): file exists=Some(true)
[2026-10-30][03:42:17][hangline_lib][DEBUG] after_drag: retry 5/5 for a3c3f9621eeeb5262d1186fc68f06abb (effect=move): file exists=Some(true)
[2026-10-30][03:42:17][hangline_lib][INFO] after_drag: item a3c3f9621eeeb5262d1186fc68f06abb still has its file after the retry window (effect=move); the shell move is still pending, keeping the card (watcher events will prune later)
[2026-10-30][03:40:37][hangline_lib][DEBUG] after_drag: retry 1/5 for d3450fbaa8f4a37b5fc654184bce925a (effect=move): file exists=Some(true)
[2026-10-30][03:40:37][hangline_lib][DEBUG] after_drag: retry 2/5 for d3450fbaa8f4a37b5fc654184bce925a (effect=move): file exists=Some(false)
[2026-10-30][03:40:38][hangline_lib][INFO] after_drag: dropped 1 card(s) whose files are gone: ["d3450fbaa8f4a37b5fc654184bce925a"]
[2026-10-30][03:40:38][hangline_lib][DEBUG] after_drag: no card dropped for 5de1e83b59b095f985e9ee1b267f1198 (effect=copy); its file still exists
[2026-10-30][03:40:40][hangline_lib][INFO] after_drag: dropped 1 card(s) whose files are gone: ["5800da2ce9bdc94c0291b258f31aad51"]
```

日志轮转说明：`tauri-plugin-log` 默认 40,000 字节且 `RotationStrategy::KeepOne`，2026-10-30 03:40:35 主脚本 A 的 retry 行被删档；A 随后用独立脚本重跑并以 `T13fix move keep2.png` 取证，retry 行完整保留（上段）。B/C/D 与 A 重跑在同一次 app 会话内完成。

### 10.4 结论与剩余项

- **effect 驱动生效**：命令签名 `after_drag(id, effect)`，未知 effect 记警告按 `none` 处理，未知 id 在启动线程前 no-op；`should_prune`/`#[allow(dead_code)]` 已删除。
- **慢移动重试生效**：`Move+exists` 在 600 ms 后 5×400 ms 重试；A 保留（revision 不变），B 在下一次重试 prune（revision +1）；重试耗尽只记 info、不谎称已 prune。
- **安全网生效**：`Copy+exists` 单次检查 no-op；`None+missing` 在 600 ms 初检即 prune。
- **IPC 不阻塞**：四条命令返回 5–17 ms；prune/日志在后台线程完成。
- **仍待人工（不得写成 PASS）**：M9 微信 COPY、M10 文件夹/桌面 Explorer MOVE、M11 回收站删除的真实目标投递与物理鼠标行为；这些**保持 MANUAL-PENDING，由 Task 17（P1 验收）负责**。本轮的 `after_drag(id,"move"/"copy"/"none")` 是程序化 effect + 真实文件系统状态，**不**证明 Explorer/微信/回收站会如何选择 effect 或删除文件。
