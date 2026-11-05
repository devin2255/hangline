# P1 展开/收起状态机与 spring 动画实验报告

- **任务**：Task 11「展开/收起状态机与自动展开（含 spring 动画）」
- **分支 / 基线**：`feat/core`，父提交 `b97f801`
- **日期**：2026-10-29
- **结论**：状态机、30 Hz 控制器、`reveal`/`tuck` 事件、前端 spring 动画与 busy 门控全部实现。新条目 `reveal(peek=true)` 与指针在带内保持、指针离开 500 ms 防抖、空线 700 ms 自动隐藏、点击线外立即收起并在指针离开触发带前抑制回弹、按压 busy 时不收起、顶边 0.5 s 停留可选展开、二次启动 pinned 展开均在运行时观察到；**2.5 s peek 到期本身没有被运行期触发**（§3 #3，由单元测试 `new_item_peeks_then_tucks_after_delay` 覆盖），menu/drag busy 仅接线、运行期待 Task 12/13（§3 #8）；展开 spring 实测 500–523 ms 收敛（规格上限 700 ms），收起 CSS ease-in 实测 216–217 ms（规格 220 ms），tuck 后 315–327 ms 窗口隐藏。运行时还发现并修复了一个 Task 11 引入的真实缺陷：命中矩形未扣除面板 translateY，导致面板滑下后 30 Hz hit-test 仍处于 click-through。所有程序化判据通过；真实物理键盘/托盘菜单点击、真实指针手感、`prefers-reduced-motion` 的视觉表现留待人工复核（§7）。

> **证据口径**：本文件区分【运行观察】与【单元测试/接线】；未跑过的项不写成已观察。

## 1. 目的与范围

验证 Task 11 的实现（设计文档 §3.3 / §3.4 / §4.4）：

1. 纯状态机：`wanted / revealed / pinned / peekUntil / dismissedUntilLeave`，peek 结束、pinned、500 ms 离开防抖、空线 700 ms、点击线外、busy 门控；
2. 控制器：单线程持有状态；面板可见时 ~30 Hz 轮询指针（面板 + 向下 40 dp 物理带）、`GetAsyncKeyState(VK_LBUTTON)` 上升沿；窗口可见性只经 `panel::show`/`panel::hide` 两个函数，但 **`panel::show` 不是控制器独占的入口**——`lib.rs` 的 ready 握手（`report_panel_ready`）直接调用 `panel::show`，控制器只是通过 `on_ready_request` 得知这次启动显示；`reveal`/`tuck` 事件只发给面板 webview；
3. 展开路径：新条目 `reveal(peekFor: revealPeekMs)`、热键/托盘/二次启动 `reveal(pinned)`、可选顶边停留 0.5 s；空线不允许 hover/顶边展开，但显式 toggle 可以展示空线；
4. 收起路径：指针离开 500 ms、空线 700 ms、点击线外立即收起并抑制回弹、再次 toggle、busy（按压/菜单/拖动）不收起；
5. 前端：`spring.ts` 半隐式欧拉积分器，`stiffness 224 / damping 25`；rAF 驱动 `translateY` 从 `-(panelHeight+12)` 到 0；收起 `easeIn 220 ms`；窗口隐藏时暂停 rAF；`prefers-reduced-motion` 立即定位；
6. 事件契约：`reveal { reason: "newItem"|"toggle"|"topEdge"|"secondInstance", peek }`、`tuck { reason: "pointerLeave"|"toggle"|"empty"|"dismiss" }`，前端类型与处理器；
7. 命中矩形仍按物理像素上报，且与展开/收起后的实际卡片位置一致。

不在本任务：点击复制/打开/删除（Task 12）、原生拖出（Task 13）、容量/连续多屏跟随（Task 14）、完整托盘/设置 UI（Task 15）、全屏抑制（spec §3.4 的独立路径）。

## 2. 环境与运行方式

| 项 | 值 |
| --- | --- |
| OS | Microsoft Windows 11 家庭版中文版，10.0.26300 |
| Rust | rustc 1.90.0 / cargo 1.90.0 |
| App | Hangline 0.1.0（debug，`cargo build`） |
| 前端 | Vite 7.3.7 dev server（`http://127.0.0.1:5173`） |
| 显示器 | `\\.\DISPLAY1` 1920×1080 @100%，工作区 1920×1032，主屏；`\\.\DISPLAY2` 1920×1080 @150%（`x=-1920`） |
| 观测 | `%LOCALAPPDATA%\app.hangline.Hangline\logs\Hangline.log`、WebView2 CDP（`--remote-debugging-port=9222`）、Win32 `GetWindowRect` / `GetWindowLongPtr(GWL_EXSTYLE)` / `GetAsyncKeyState` |

**运行方式（沿用 Task 2/5/7/8/10 的沙箱 workaround）**：

1. `pnpm dev` 启动 Vite；
2. `cargo build --manifest-path src-tauri/Cargo.toml`，把 `src-tauri/target/debug/hangline.exe` 复制到 `%TEMP%\hangline-task11\hangline.exe`；
3. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-allow-origins=*` 启动副本；
4. CDP `Runtime.evaluate` 读取 `getComputedStyle(.panel).transform`、`data-revealed`、卡片矩形；页面内 rAF 采样器记录每帧 `translateY`；`Runtime.consoleAPICalled` 记录前端 `reveal/tuck/spring settled` 时间；
5. 输入：`SetCursorPos` 控制指针；`mouse_event` 产生真实左键按下/释放/点击；`keybd_event` 注入 `Ctrl+Alt+T`；第二次启动 exe 触发单实例 toggle；
6. 条目来源：运行时向 `%LOCALAPPDATA%\Hangline\Screenshots` 复制自生成的 640×360 PNG，目录 watcher 挂上条目并发出 `reveal-request`；空线场景删除 `state.json` 后重启。

> 与 Task 10 相同，证据图只包含面板 webview 内容（透明像素压到 `#14171e`），不含桌面壁纸、远程会话 ID 或任务栏。

## 3. 预期 vs 观察

| # | 项 | 预期 | 观察 | 结论 |
| --- | --- | --- | --- | --- |
| 1 | 空线启动 | 首帧后显示；空线无 peek 时 700 ms 后收起并隐藏 | 最终二进制：`panel ready at +1181 ms` → `tuck (Empty) at +1885 ms`（**704 ms**）→ `hidden at +2211 ms`（tuck 后 **326 ms**）；DOM `translateY=-222`、`data-revealed=false` | PASS |
| 2 | 空线显式 toggle | 空线允许 pinned 展开（hover/顶边不允许） | 注入 `Ctrl+Alt+T` → `reveal: shown (reason Toggle, peek=false, items=0) at +86936 ms`；DOM `y=0/revealed=true`；再次注入 → `tuck (Toggle)`，隐藏 | PASS |
| 3 | 新条目自动 peek | 新条目 → `reveal(newItem, peek=true)`；peek 2.5 s 内不因指针离开收起 | `reveal: shown (reason NewItem, peek=true, items=1) at +90434 ms`；指针保持在带内 3 s，DOM 仍 `y=0/revealed=true`——只证明 peek 展开与悬停保持，**2.5 s peek 到期未被运行期触发**；#4 的 514 ms tuck 是 500 ms 指针离开防抖，不是 peek 到期 | PASS（运行观察 `reveal(peek=true)` 与指针悬停行为；**2.5 s peek 到期由单元测试覆盖**，非运行期） |
| 4 | peek 结束后指针离开 | 离开面板+40 dp 带 500 ms 后收起 | 指针移到 (960,700) 后：`tuck (PointerLeave) at +93844 ms (pointer outside Some(514) ms)`；日志 `hidden at +94169 ms` | PASS |
| 5 | 指针离开 500 ms 防抖 | 暂不收起，计时到点才收起 | 单元测试 `pointer_leave_is_debounced_by_500ms`（2500/2999 否，3000 是）；运行时所有 `PointerLeave` 的 outside 值 506–651 ms（忙时 532 ms 见 #8） | PASS |
| 6 | 点击线外 | 立即收起 + `dismissedUntilLeave`，直到指针离开触发带 | pinned 展开后真实点击 (960,700)：`tuck (Dismiss) at +397070 ms` → `hidden +397397 ms`；带内点击 (960,230) 后 `new-item peek suppressed until the pointer leaves the band`，离开后下一条目正常 `reveal (NewItem) at +99348 ms` | PASS |
| 7 | 收起动画/隐藏时序 | `easeIn 220 ms`，动画结束后窗口 hide | rAF 采样：tuck 起于 2224 ms 的 `-2.22px`，2441 ms 到 `-222px`，**217 ms**；Rust `hidden` 在 tuck 后 **315–327 ms**（`TUCK_ANIMATION_MS = 300` 的隐藏延时覆盖 220 ms CSS 动画，不是 220 + 300 相加） | PASS |
| 8 | busy（按压/菜单/拖动） | busy 时指针离开也不收起；结束后重新计 500 ms | **仅按压 busy 有运行证据**：真实按下卡片 → 指针移到 (960,700) → 越过 peek 2.7 s 后 DOM 仍 `y=0/revealed=true`；释放后 `tuck (PointerLeave) at +381704 ms (pointer outside Some(532) ms)`。menu/drag busy 的输入源与 OR 已接线（`menu_open` 预留、`dragout::is_dragging` 就位），真实来源分别在 Task 12/15 与 Task 13 才产生 | PASS（press 运行观察；menu/drag **接线，运行期待 T12/13**） |
| 9 | 顶边停留（可选） | `topEdgeReveal=true` 且线非空、顶边停留 0.5 s → 展开 | `set_setting topEdgeReveal=true` 后 (960,0) 停留：`reveal: shown (reason TopEdge, peek=false, items=3) at +104581 ms`；指针离开：`tuck (PointerLeave) ... outside Some(512) ms`；随后恢复 false | PASS |
| 10 | 二次启动 | 第二次启动唤起已有实例并 toggle；reason=`secondInstance` | 第二次启动进程 exit 0，已有实例 `reveal: shown (reason SecondInstance, peek=false, items=8) at +448596 ms` | PASS |
| 11 | 展开 spring | `stiffness 224 / damping 25`，700 ms 内收敛到目标 ±0.5 | 页面 rAF 采样：firstMoved 290 ms → firstZero 790 ms（**500 ms**）；前端 console `reveal spring settled in 508–528 ms`；10 次 reveal 全部 ≤528 ms（settled 判据为位移 ≤0.1 px 且速度 ≤1 px/s，与单测的 ±0.5 不同，见 §6.7） | PASS |
| 12 | 命中矩形 vs translateY | 面板滑下/收起后，卡片命中矩形仍是实际可见位置 | **修复前**：6 张卡已上报、指针 (873,100) 在卡片上时 `WS_EX_TRANSPARENT=True`（点击穿透）；**修复后**（扣除 `translateY`）：同一探针 `WS_EX_TRANSPARENT=False` | PASS（运行时修复） |
| 13 | 事件去重 | 每次新条目只触发一次前端 reveal | 修复前一次新条目 console 出现 `reveal(newItem, peek=false)` + `reveal(newItem, peek=true)` 两条（watcher 的裸事件被 JS 的 `Any` 监听收到）；改用 Rust-only `reveal-request` 后每次只有一条 | PASS（运行时修复） |

## 4. 证据

### 4.1 状态机事件日志（节选）

最终二进制的空线启动（`Hangline.final-empty.log`）：

```text
[11:09:57][hangline_lib::reveal][INFO] reveal: panel ready at +1181 ms; startup show is a non-pinned reveal (empty auto-hide 700 ms)
[11:09:58][hangline_lib::reveal][INFO] reveal: tuck (Empty) at +1885 ms (busy=false, pointer outside None ms); hiding in 300 ms
[11:09:58][hangline_lib::panel][DEBUG] panel: hidden
[11:09:59][hangline_lib::reveal][INFO] reveal: hidden at +2211 ms (tuck animation finished)
```

空线 toggle、新条目 peek、指针离开（`Hangline.task11.log`）：

```text
[11:00:27][hangline_lib::reveal][INFO] reveal: shown (reason Toggle, peek=false, items=0) at +86936 ms
[11:00:28][hangline_lib::reveal][INFO] reveal: tuck (Toggle) at +88270 ms (busy=false, pointer outside None ms); hiding in 300 ms
[11:00:29][hangline_lib::reveal][INFO] reveal: hidden at +88593 ms (tuck animation finished)
[11:00:30][hangline_lib::reveal][INFO] reveal: shown (reason NewItem, peek=true, items=1) at +90434 ms
[11:00:34][hangline_lib::reveal][INFO] reveal: tuck (PointerLeave) at +93844 ms (busy=false, pointer outside Some(514) ms); hiding in 300 ms
[11:00:34][hangline_lib::reveal][INFO] reveal: hidden at +94169 ms (tuck animation finished)
```

点击线外 + 回弹抑制：

```text
[11:00:35][hangline_lib::reveal][INFO] reveal: shown (reason Toggle, peek=false, items=1) at +95146 ms
[11:00:36][hangline_lib::reveal][INFO] reveal: tuck (Dismiss) at +96543 ms (busy=false, pointer outside None ms); hiding in 300 ms
[11:00:36][hangline_lib::reveal][INFO] reveal: click outside the panel -> dismissed until the pointer leaves the band (at +96543 ms)
[11:00:37][hangline_lib::reveal][INFO] reveal: hidden at +96869 ms (tuck animation finished)
[11:00:38][hangline_lib::reveal][INFO] reveal: new-item peek suppressed until the pointer leaves the band
[11:00:38][hangline_lib::reveal][DEBUG] reveal: pointer left the band; dismiss suppression cleared
[11:00:39][hangline_lib::reveal][INFO] reveal: shown (reason NewItem, peek=true, items=3) at +99348 ms
```

顶边停留、busy、二次启动：

```text
[11:00:45][hangline_lib::reveal][INFO] reveal: shown (reason TopEdge, peek=false, items=3) at +104581 ms
[11:00:46][hangline_lib::reveal][INFO] reveal: tuck (PointerLeave) at +106068 ms (busy=false, pointer outside Some(512) ms); hiding in 300 ms
[11:00:46][hangline_lib::reveal][INFO] reveal: hidden at +106393 ms (tuck animation finished)
[11:05:16][hangline_lib::reveal][INFO] reveal: shown (reason NewItem, peek=true, items=8) at +375870 ms
[11:05:22][hangline_lib::reveal][INFO] reveal: tuck (PointerLeave) at +381704 ms (busy=false, pointer outside Some(532) ms); hiding in 300 ms
[11:05:22][hangline_lib::reveal][INFO] reveal: hidden at +382026 ms (tuck animation finished)
[11:06:29][hangline_lib::reveal][INFO] reveal: shown (reason SecondInstance, peek=false, items=8) at +448596 ms
```

### 4.2 动画时序

**展开 spring**（页面 rAF 采样，6 张卡，`translateY` 从 −222 到 0）：

| 采样 | 时间 ms | translateY px | data-revealed |
| --- | --- | --- | --- |
| firstMoved | 290 | −219.83 | true |
| 1/2 | 约 450 | −110 级 | true |
| firstZero | 790 | 0 | true |
| last | 1673 | 0 | true |

→ **500 ms** 采样窗口到达 0；console 记录 `reveal spring settled in 523 ms`（同一次），全部 10 次 508–528 ms，满足规格 ≤700 ms 与单测 ±0.5 收敛判据。

**收起 CSS ease-in**（同一采样器，从 0 → −222）：

| 采样 | 时间 ms | translateY px |
| --- | --- | --- |
| tuckStart | 2224 | −2.22 |
| 中段 | 2320 | −100 级 |
| fullyTucked | 2441 | −222 |

→ **217 ms**，与 `transition: transform 220ms ease-in` 一致；Rust 在 tuck 后 315–327 ms 才 `hide()`：`TUCK_ANIMATION_MS = 300` 的延时覆盖了 220 ms 的 CSS 收起动画（不是 220 ms + 300 ms 相加），不留下隐形窗口。

**busy 按压**：指针按在卡片上并移到带外，越过 peek 后 DOM 仍 `y=0`；释放后 `PointerLeave outside 532 ms` 才收起，证明 busy 期间既不收起也不累计离开计时。

### 4.3 命中矩形与 translateY（运行时发现的修复）

Task 11 之前面板窗口从不位移，`getBoundingClientRect()` 就是窗口 client 坐标。加入 tuck 位移后，前端在面板收起时上报的矩形仍包含 `translateY(-222)`，Rust 30 Hz hit-test 按这些「屏幕外」矩形判断，结果面板滑下后 `WS_EX_TRANSPARENT` 一直为 true，真实鼠标无法点到卡片。

探针（6 张卡，指针在 (873,100)，卡面 798..948 × 58..146）：

```text
修复前: hit rects = 6, WS_EX_TRANSPARENT=True  （点击穿透，pointerdown 收不到）
修复后: hit rects = 6, WS_EX_TRANSPARENT=False （可点击）
```

修复：`collectHitRects` 先读取根元素的当前 `translateY`，再把卡片 rect 的 y 减去该值后换算物理像素；视口裁剪仍在同一变换坐标系内完成。这样即使矩形是在收起状态上报的，spring 落定后的命中位置也正确。此项已加入 `Panel.tsx` 注释，并成为后续 Task 12/13 点击/拖出的前提。

### 4.4 事件去重（运行时发现的修复）

Task 8/9 的 watcher 以 `emit("reveal", {reason:"newItem"})` 广播新条目；Task 11 的控制器也监听 `reveal`。运行时一次新条目出现两条前端日志：

```text
Hangline panel: reveal (newItem, peek=false) at 149364 ms   ← watcher 裸事件，无 peek
Hangline panel: reveal (newItem, peek=true)  at 149375 ms   ← 控制器事件
```

原因：Tauri JS `listen` 注册的目标是 `EventTarget::Any`，`emit_to(EventTarget::App, ...)` 的过滤对 `Any` 监听不做限制，裸请求仍到达 webview。修复：watcher 改用独立的 Rust-only 事件名 `reveal-request`，控制器监听它并只发送带 `reason`/`peek` 的 `reveal`；修复后每次新条目只有一条前端事件。

### 4.5 证据图

`P1-reveal-panel.png`（1920×210，110,910 bytes，SHA256 前 16：`73E13839907FDDC4`）：展开状态、8 张卡片的面板 webview 截图，透明像素压到 `#14171e`。

## 5. 测试与 RED/GREEN

### 5.1 Rust 状态机（TDD）

- RED：`cargo test reveal::tests` → `0 passed; 8 failed`，均为 `assertion failed`（stub 返回 false / None），输出保存于 `%TEMP%\hangline-task11-red-reveal.txt`；
- 实现后 GREEN：`9 passed; 0 failed`（新增 `pointer_toggle_right_after_a_click_dismiss_stays_hidden`）；
- 覆盖：peek 结束、peek 期间不收起、pinned 忽略离开/空线、空线 700 ms、点击线外 + 回弹抑制、busy、500 ms 防抖、顶边 500 ms、托盘指针 toggle 防重复处理；
- 全量 `cargo test`：**110 passed, 0 failed, 5 ignored**（115 tests）。

### 5.2 前端 spring（TDD）

- RED：`pnpm exec vitest run src/lib/spring.test.ts` → `Cannot find module './spring'`；
- GREEN：**5 passed**（700 ms 内 ±0.5 收敛、120 帧有界、`dt=1s` 稳定、`MAX_DT` 钳制、非正 dt 不变）；
- 全量 `pnpm check`：lint / typecheck / **28 passed** / i18n / version 全绿。

### 5.3 门禁

```text
cargo test   → 110 passed; 0 failed; 5 ignored
cargo check  → Finished, 0 warnings
pnpm check   → eslint + tsc + 28 tests + i18n + version, exit 0
```

## 6. 偏差与决策

1. **Rust 请求事件改名 `reveal-request`**：见 §4.4。前端契约仍是 `reveal { reason, peek }`；watcher 只负责「有新条目」信号，状态机决策留在控制器。这是对 Task 8/9 文档中 `reveal {reason:"newItem"}` 的直接增量修改，属 Task 11 的接线职责。
2. **ready 启动显示视为一次非 pinned reveal**：保留 Task 10「首帧后 show」的握手，但状态机从 ready 时刻开始计时，因此空线会在 700 ms 后 `tuck(Empty)`。这是 spec §3.4 收起路径 2 的直接结果；若后续产品希望空线启动常驻，应改 spec 而不是绕过状态机。
3. **隐藏轮询**：面板隐藏时控制器默认 200 ms 醒来一次（与 hittest 的 `HIDDEN_INTERVAL` 相同），只读内存快照，不调用窗口 API；仅当 `topEdgeReveal` 开启或有 `dismissedUntilLeave` 待解除时才 30 Hz 读 `GetCursorPos`（顶边路径与「直到指针离开触发带」语义所必需）。因此「隐藏时停止窗口轮询」仍成立，但「线程完全休眠」不成立，已在 `reveal.rs` 模块文档写明。
4. **托盘/菜单点击防重复**：点击托盘图标/菜单本身是「面板外左键」，会先触发 dismiss；随后托盘动作再 toggle 会把线重新拉下。控制器记录最近一次 dismiss，600 ms 内来自指针的 toggle 视为同一次点击的收尾，保持隐藏；热键与二次启动不受影响。该保护有单测，未在真实托盘菜单上做运行时点击（§7）。
5. **`panel::show` 幂等**：已可见时直接返回，避免控制器每次 reveal 都重复 `win.show()`/托盘标签同步；`hide` 仍在 tuck 动画结束后只调用一次。
6. **命中矩形补偿 translateY**：见 §4.3。它是 Task 11 引入的非纯状态机缺陷，修复属于本任务范围（否则展开后的面板不可点击，Task 12/13 无法继续）。
7. **spring 的 508–528 ms 与 417 ms 不是同一判据**：417 ms 是 60 fps 数值积分首次进入 ±0.5 px 的时刻（也就是单测「700 ms 内收敛到 ±0.5」的判据）；运行时 console 的 settled 用的是更紧的 `|x| ≤ 0.1 px && |v| ≤ 1 px/s`，同一 60 fps 数值模拟下约 **517 ms**，与运行时 508–528 ms 同量级。差值来自 rAF 偶发掉帧被 `MAX_DT=1/30` 钳制（模拟时间略短于真实时间）；两种判据都满足规格 ≤700 ms。
8. **busy 的输入源**：`set_panel_busy(bool)` 由前端 `pointerdown` 置位、`pointerup`/`pointercancel`/`blur`/`visibilitychange` 清除；原生拖出继续由 `dragout::is_dragging()` 提供，二者在控制器里 OR。菜单打开标志预留给 Task 12/15。
9. **共享桌面干扰**：测试期间桌面被真实用户/远程会话使用（一次启动在 ready 后 +233 ms 记录到一次非注入的 `tuck (Dismiss)`；空线自动隐藏的最终取证因此在独立重启中完成）。所有引用的时序均来自脚本注入动作明确的阶段，未把干扰行当作结论。

## 7. 手动待验证

1. **真实物理键盘 / 真实托盘点击**：本轮热键用 `keybd_event` 注入、二次启动用真实 exe；P0-E3 已确认注入对 `RegisterHotKey` 有效，但真实键盘和真实任务栏托盘菜单（含托盘菜单的防重复处理）仍需人工各点一次。
2. **视觉手感**：spring 的过冲/回弹与 220 ms 收起的实际观感；`prefers-reduced-motion` 需要手动在 Windows「动画效果」中关闭后看一次（代码路径为立即定位）。
3. **菜单/长按/拖出 busy**：本轮只验证了「按压卡片」busy；`menu_open`/`drag_active` 的接线在 Task 12/13 才真正产生，届时需复测「菜单打开期间线不收起」和拖出期间不收起（§3 #8 同口径：接线，运行期待 T12/13）。
4. **多显示器 / 150%**：本轮动画在 100% 主屏取证；150% 下 rAF 与 CSS 变换应一致，但未单独截图。
5. **桌面壁纸真实合成**：证据图仍是 webview 内容合成到中性底；透明窗口在真实壁纸上的观感同 Task 10 §6。
6. **全屏抑制**：spec §3.4 的 `SHQueryUserNotificationState` 路径不在 Task 11，未实现/未验证。

## 8. 清理与恢复

| 项 | 动作 / 结果 |
| --- | --- |
| App / Vite | 已停止（无 `hangline.exe`、无 5173 监听） |
| `settings.json` | SHA256 恢复为 `8E315C7B...A79089`（与测试前一致，`topEdgeReveal=false`） |
| `state.json` | 测试前不存在；运行产生的 9 条测试状态已删除 |
| `Screenshots` | `hangline-t11-*.png` 测试图全部删除；原有 2 张 Hangline 截图保留 |
| `Hangline.log` | 恢复为测试前 4,417 bytes，SHA256 `CE69D5014236FC8B448CD9505F80C4873A27CF2CB83F19A6533AB75CF2D2FA92`（与 Task 10 结束时一致） |
| 临时文件 | 脚本/采样/原始日志保留在 `%TEMP%\hangline-task11`（不提交），任务结束后删除 |

## 9. 关键命令

```powershell
pnpm dev
cargo build --manifest-path src-tauri/Cargo.toml
Copy-Item src-tauri\target\debug\hangline.exe "$env:TEMP\hangline-task11\hangline.exe"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222 --remote-allow-origins=*'
Start-Process "$env:TEMP\hangline-task11\hangline.exe"

# 采样 translateY / data-revealed（页面内 rAF）
node "$env:TEMP\hangline-task11\cdp.mjs" eval-file "$env:TEMP\hangline-task11\sampler-start.js"
node "$env:TEMP\hangline-task11\cdp.mjs" eval-file "$env:TEMP\hangline-task11\sampler-summary.js"
# 注入输入
pwsh -NoProfile -File "$env:TEMP\hangline-task11\input.ps1" -Action toggle   # Ctrl+Alt+T
pwsh -NoProfile -File "$env:TEMP\hangline-task11\input.ps1" -Action click -X 960 -Y 700
# 命中/穿透探针
GetWindowLongPtr(panelHwnd, -20) -band 0x20   # WS_EX_TRANSPARENT
```
