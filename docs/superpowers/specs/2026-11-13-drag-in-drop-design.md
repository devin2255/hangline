# 拖入投放（Drag-in Drop）设计：把外部图片直接挂到晾衣绳上

- **日期**：2026-11-13
- **目标版本**：v1.5.0
- **状态**：设计已确认（用户 2026-11-13 批准：方案 1、复制入库、顶部自动唤出），待实现
- **关联**：[主设计 spec §3.3/§3.4/§4.4](../specs/2026-10-28-hangline-windows-design.md)（面板几何、reveal/tuck 状态机、事件表）

## 0. 一句话

支持把**文件夹/Explorer 的图片、微信/QQ 聊天窗里的图片、浏览器里的图片、系统截图工具结果**直接拖到屏幕上沿；晾衣绳自动唤出并高亮，松手后图片复制进 Hangline 自有目录并挂成卡片（哈希去重、缩略图、落绳动画），不新增窗口、不抢焦点、不联网。

## 1. 背景与目标

当前 Hangline 只有三种入库路径：截图、剪贴板监听、目录监听。用户最常见的"我手里已经有一张图"场景（微信聊天图片、下载文件夹里的图、网页图片）没有直接入口，只能先另存再等目录监听或截图。

**目标**

1. 外部拖拽进入面板条带（或拖到屏幕顶部唤出条带）→ 松手挂上卡片；
2. 兼容"只给文件路径"和"只给位图（CF_DIB）"两类来源；
3. 复制入库（用户选择 A）：源文件/微信临时文件删除后卡片仍可用；
4. 复用现有入库纪律：BLAKE3-64 去重、缩略图、原子写、`state` 广播、reveal 动画、点击穿透语义；
5. 不改变拖**出**行为，不新增窗口，不抢焦点，不新增任何网络行为。

**非目标（本次不做）**

- 拖入视频/录屏（图片 only；视频已有录制目录监听）；
- 从 URL/网页直接下载图片（只接受拖拽携带的字节）；
- 拖入时替换/排序已有卡片（新卡片始终挂最右端，与现有新增语义一致）；
- 新增设置开关（默认开启；后续若需要可加托盘项）；
- 拖到屏幕顶部以外区域自动唤出（只在顶部唤出带生效）。

## 2. 用户流程

### 2.1 绳子已显示（截图后的 peek / 热键 pinned）
拖拽图片进入面板条带 → 绳子高亮 + 提示"松手挂上绳子" → 松手 → 卡片挂在右端，伴随现有 220ms 弧度动画。

### 2.2 绳子收起（默认状态）
按住图片拖动 → 指针进入目标显示器顶部唤出带（工作区顶 14dp）→ 绳子自动露出（不抢焦点）→ 拖到绳子上 → 松手挂上。拖拽取消或松手在面板外 → 绳子按现有 tuck 规则收起。

### 2.3 重复/不支持
同一张图重复拖入 → 不新增卡片，显示 2s 轻提示"已在绳上"；不支持的文件/视频 → 同样轻提示"仅支持图片"，日志记录格式名。

## 3. 现状约束（调研结论）

1. **面板是透明、无边框、`focusable(false)` 的置顶窗口**；点击穿透由 `hittest.rs` 30Hz 轮询 + `set_ignore_cursor_events` 实现（只有卡片 rect 可点）。OLE 投放需要窗口在命中测试中可见，因此必须在拖拽期间临时关闭点击穿透。
2. **Tauri/wry 的拖放事件只提供文件路径**（`DragDropEvent::Enter/Drop { paths }`），拿不到 `CF_DIB`；微信/QQ 等来源可能只给位图。因此不能用现成事件满足目标 2。
3. **wry 的拖放接管方式已被源码佐证**：`wry-0.57.1/src/webview2/drag_drop.rs` 在 WebView2 的每个子 HWND 上 `RevokeDragDrop` + `RegisterDragDrop(拖动目标)`；这说明"在 WebView2 子窗口上替换拖放目标"是可行路径。我们采用同样的注册策略，但换成自己的 `IDropTarget` 与完整格式读取。
4. Tauri 2.12.5 的 `WebviewWindowBuilder::disable_drag_drop_handler()` 可关闭 wry 自带处理器，避免它先注册再被我们覆盖的竞态。
5. 剪贴板监听已有完整的 `CF_DIB/CF_DIBV5/PNG` 解码、PNG 编码、原子写、唯一命名代码（`watchers/clipboard.rs`），可抽成共享模块复用。
6. reveal 控制器已有 `Command` 通道、`TuckReason`、`BUSY` 标志、`topEdge` 唤出路径；新增一个 drop 原因即可，不需要新的窗口所有权模型。
7. 点击穿透的普通点击语义必须保持：仅当"左键按下 + 有位移"才进入拖拽门控，普通点击/悬停不受影响。

## 4. 架构

```
外部拖拽源（Explorer / 微信 / 浏览器 / 截图工具）
        │  OLE DoDragDrop（CF_HDROP / CF_DIB / CF_DIBV5 / "PNG"）
        ▼
┌────────────────────────────────────────────────────────────┐
│ dropin.rs：Rust OLE IDropTarget（注册在 panel 及全部子 HWND）│
│  DragEnter/Over → 返回 DROPEFFECT_COPY，emit dropin-hover    │
│  Drop → 同步提取原始数据（路径/字节），spawn worker 入库      │
│  DragLeave → emit dropin-leave                               │
└───────────────┬────────────────────────────────────────────┘
                │ worker（不阻塞 OLE/UI 线程）
                ▼
┌────────────────────────────────────────────────────────────┐
│ image_io.rs（从剪贴板抽取的共享纯函数）                       │
│  decode_dib / decode_png / encode_png / write_png_atomic    │
│  unique_path / pixel caps                                    │
└───────────────┬────────────────────────────────────────────┘
                ▼
┌────────────────────────────────────────────────────────────┐
│ 入库：BLAKE3-64 dedupe → Screenshots\Dropped yyyy-MM-dd      │
│ HH.mm.ss.png → thumbnails::attach → AppState::add_item       │
│ (kind=Dropped) → emit_state → 前端 state 广播 + 弧度动画      │
└────────────────────────────────────────────────────────────┘

拖拽门控（dropin 内 40ms 线程）+ reveal 集成：
  左键按下 + 位移≥4px + !dragout::is_dragging()
    ├─ 绳子收起 + 指针进入顶部唤出带 → reveal::request_drop_reveal()
    ├─ 指针在面板 rect 内 → hittest::DragHold(true)（hittest 单写者把
    │    ignore_cursor_events 置 false，OLE 才能命中面板）
    └─ 松手/离开 → DragHold(false) + reveal::end_drop_reveal()
```

### 4.1 组件与职责

| 组件 | 职责 | 依赖 |
|---|---|---|
| `src-tauri/src/dropin.rs`（新） | `IDropTarget` 实现与注册/注销；格式提取；拖拽门控状态机；emit 前端事件 | Win32 OLE、`image_io`、`app_state`、`thumbnails`、`dedupe`、`reveal`、`hittest` |
| `src-tauri/src/image_io.rs`（新，抽自剪贴板） | 纯图片编解码/写盘/命名/上限检查 | `image`、`windows` DIB 解析 |
| `src-tauri/src/hittest.rs`（改） | 新增 `DragHold` 共享标志；`should_ignore = !hit && !drag_hold`，仍是 `set_ignore_cursor_events` 唯一写者 | 无 |
| `src-tauri/src/reveal.rs`（改） | 新增 `Command::DropReveal` / `RevealReason::Drop` / `State.drop_hold`；drop 期间不因空绳/指针离开而收起；拖拽结束恢复 | `panel` |
| `src-tauri/src/panel.rs`（改） | builder 加 `.disable_drag_drop_handler()`；创建完成后调用 `dropin::install(&win)` | `dropin` |
| `src-tauri/src/item.rs`（改） | `ItemKind::Dropped`（wire `"dropped"`） | 无 |
| `src-tauri/src/watchers/clipboard.rs`（改） | 改用 `image_io`（行为不变，测试同步） | `image_io` |
| 前端 `Panel.tsx` / `Panel.css` / `i18n`（改） | 监听 `dropin-hover`/`dropin-leave`/`dropin-result`；绳子高亮 + 提示 pill；重复/拒绝轻提示 | 无 |
| `src/lib/types.ts`（改） | `ItemKind` 增加 `"dropped"`；drop 事件类型 | 无 |

### 4.2 关键决策

1. **自建 `IDropTarget`，不用 Tauri 拖放事件**：后者只有路径，微信位图场景不成立。注册方式复制 wry 的既有策略（`EnumChildWindows` → `RevokeDragDrop` → `RegisterDragDrop`），并把目标对象保存在 `DropTargets` 全局中随 App 生命周期存活。
2. **OLE 回调只做提取，不做解码**：`Drop` 里只把 `CF_HDROP` 路径 / `PNG` / `DIB` 字节复制成 owned 数据（受 64MB 上限保护），立即返回；解码/写盘/入库放到 worker 线程，避免卡住 OLE 和 UI（与拖出实现"工作线程 + 主线程"的教训一致）。
3. **复制入库 + 新 `ItemKind::Dropped`**：写入自有 `Screenshots` 目录；卡片可标注、可删除（回收站）、可"仅从线上移除"。来源类型进 `state` 广播，前端类型与右键菜单规则同步。
4. **点击穿透只由 hittest 线程写**：dropin 通过 `hittest::DragHold` 标志表达"拖拽期间需要命中"，避免两个线程同时调 `set_ignore_cursor_events` 互相覆盖。
5. **唤出走 reveal 控制器**：新增 `Command::DropReveal` + `State.drop_hold`，不直接 `panel::show`；全屏抑制/捕获抑制沿用现有门控（抓屏中投放下沉，不强行弹线）。
6. **不复制剪贴板逻辑**：把解码/编码/原子写抽到 `image_io.rs`，剪贴板与 dropin 共用同一套上限与命名纪律。

## 5. 数据流与格式

### 5.1 支持格式与优先级

对每次投放，从 `IDataObject` 按顺序判断，**只取第一条可用表示**（避免同一张图被多种格式重复入库）：

| 优先级 | 格式 | 处理 |
|---|---|---|
| 1 | `CF_HDROP` | 取全部路径；只接受图片扩展名（png/jpg/jpeg/bmp/webp/gif）；逐个解码 |
| 2 | `"PNG"` 注册格式 | 字节直接进解码（trim 到 IEND） |
| 3 | `CF_DIBV5` / `CF_DIB` | 用共享 `decode_dib` 还原 RGBA |

- 同一来源同时提供 `CF_HDROP` 与位图时用 `CF_HDROP`（原始文件质量更好）；
- 文件路径来源也统一"解码 → RGBA → PNG"，与剪贴板/截图的存储形态一致；
- 拒绝：视频、非图片、解码失败、空数据、超限；每类只记一条日志（不记录用户完整路径，记录扩展名/尺寸/原因）。

### 5.2 上限（防拖爆内存）

| 限制 | 值 | 理由 |
|---|---|---|
| 单图解码后像素 | ≤ 50 MP | 与剪贴板/截图的上限口径一致（复用 `check_pixel_cap`） |
| 单次原始数据总量 | ≤ 64 MB | OLE 数据复制上限，超出拒绝 |
| 单次图片数量 | ≤ 20 张 | 与卡片容量/处理时延平衡 |

### 5.3 命名与去重

- 存入 `%LOCALAPPDATA%\Hangline\Screenshots\Dropped yyyy-MM-dd HH.mm.ss.png`；同秒冲突用现有 `unique_path` 追加序号；
- 写盘用 `.png.tmp` + rename 原子写；
- 去重：对**最终 PNG 字节**做 BLAKE3-64；命中 LRU 或同内容已在线上 → 跳过并计为 duplicate；
- `dimensions` 用解码后的像素尺寸；`thumbnails::attach` 在首次 broadcast 前生成预览；
- 入库顺序 = 拖入顺序，全部挂最右端（新卡语义）。

## 6. 拖拽门控状态机（纯逻辑，可单测）

输入（每 40ms 采样）：`left_button_down`、`cursor_local`（面板内坐标/屏幕坐标）、`panel_visible`、`outgoing_drag`（`dragout::is_dragging()`）、`monitor_top_band`、`panel_rect`。

状态：

| 状态 | 进入条件 | 行为 | 退出 |
|---|---|---|---|
| `Idle` | 初始/松手/取消 | 不做任何事 | 左键按下 + 位移≥4px → `Suspect` |
| `Suspect` | 有按下且位移 | 记录起点；若绳子收起且进入顶部唤出带 → 请求 DropReveal；若指针进入面板 rect → `OverPanel` | 松手 → `Idle`；位移结束但仍在带内保持 |
| `OverPanel` | 指针在面板 rect（含 8dp 容差） | `DragHold(true)`；emit `dropin-hover` | 指针离开 rect+容差 → `Suspect`；松手 → `Idle` |
| 任意 | `outgoing_drag` 为真 | 忽略门控（拖出由现有 OLE 会话负责） | 会话结束 → `Idle` |

- 位移阈值 `DRAG_START_PX = 4`（隐藏常量，与 hit 逻辑解耦）；
- **普通点击保护**：`Suspect` 只在"按下 + 位移"成立后进入，且仅在面板 rect 内才关闭穿透；因此普通点击（无位移）永远不关闭穿透；
- 顶部唤出带 `DROP_REVEAL_BAND_DP = 14`（工作区顶部向下 14dp；覆盖"屏幕最上沿"且不误伤标题栏点击）；
- 松手后延迟一个采样周期（40ms）再恢复穿透，确保 OLE 完成 Drop 回调。

## 7. reveal 集成

- 新增 `RevealReason::Drop`，payload 与 `topEdge` 类似：`{ reason: "drop", peek: false }`；
- `Command::DropReveal`：`state.wanted = true`，`drop_hold = true`；若当前被全屏/捕获抑制则**忽略**（不排队）；
- 控制器 tick：`drop_hold` 时跳过 `pointerLeave`、`emptyHide` 的 tuck 判定；`Command::DropEnd` 清除标志并立即按当前指针位置重算一次 tuck（松手在面板外 → 正常收起）；
- 拖拽取消/松手后无论是否投放成功，都发送 `DropEnd`；
- 点击外部 dismiss 逻辑不变（拖拽期间不会触发点击）。

## 8. 前端交互与事件

新增 Rust → 前端事件（仅 panel webview）：

| 事件 | payload | 前端行为 |
|---|---|---|
| `dropin-hover` | `{ images: number }` | 面板加 `data-drop-active`；绳子高亮（材质高光增强，180ms 过渡，不改变线宽）；显示提示 pill |
| `dropin-leave` | `{}` | 取消高亮/提示 |
| `dropin-result` | `{ added, duplicates, rejected }` | `added>0` 时无需额外 UI（state 广播会挂卡）；否则显示 2s 轻提示（重复/仅支持图片）；随后面板恢复 |

- 提示文案新增 i18n：`Drop to hang` / `松手挂上绳子`、`Already on the line` / `已在绳上`、`Images only` / `仅支持图片`；
- 不抢焦点、不新增窗口；提示 pill 复用现有 `.panel__hint` 视觉语言；
- 空绳时 `panel__hint` 与投放提示互斥（投放提示优先）。

## 9. 错误处理

| 场景 | 行为 |
|---|---|
| `RegisterDragDrop`/`RevokeDragDrop` 失败 | 记 warn，投放降级为不可用（不影响其他功能）；日志带 HWND 与错误码 |
| 不支持格式/视频 | 拒绝并计入 `rejected`，emit result 轻提示；日志只记格式名 |
| 解码失败/超限 | 同上，附原因（尺寸/解码错误） |
| 写盘失败 | 释放 dedupe 认领，记 error，emit result 提示；绝不让 hash 被永久吞掉 |
| 入库失败 | 删除已写文件、释放 dedupe、广播回滚后的 state（沿用 watcher 的失败纪律） |
| 投放时面板被全屏/捕获抑制 | 不唤出、不投放；日志 debug |
| OLE 回调内异常 | 全部捕获并返回 `DROPEFFECT_COPY`/`NONE`，绝不 panic 到 COM |

## 10. 测试计划

### 10.1 Rust 单测（新增 ≥25）
- 门控状态机：普通点击不触发；按下+位移触发；进入/离开面板；松手恢复；外部拖出被忽略；顶部唤出带边界；全屏/捕获抑制下不唤出。
- 格式判定：`CF_HDROP` 优先、PNG 次之、DIB 兜底；混合来源不重复入库；扩展名过滤；上限截断/拒绝。
- 复用 `image_io`：DIB/PNG 解码、编码、原子写、唯一命名、像素上限（迁移剪贴板测试后仍全绿）。
- 入库：去重命中、命名、`ItemKind::Dropped` 序列化、失败回滚（临时目录）。
- reveal 纯函数：`DropReveal`/`DropEnd` 对 `State` 的影响、drop_hold 抑制 tuck。

### 10.2 真实拖放验证
- 写一个一次性 C# 工具（不提交或放 `.acceptance/`）：创建 `DataObject`，分别以 `FileDrop`（多文件）与 `Bitmap`（CF_DIB）发起真实 `DoDragDrop` 到面板，验证卡片挂上、去重、多图顺序；
- 真机来源：Explorer 文件夹图片、微信聊天图片、QQ、浏览器图片、截图工具（用户配合微信/QQ 实测一次）；
- 回归：点击穿透（普通点击桌面不误触）、拖出卡片、截图/标注/搜索/更新、内存（投放 20 张后落绳后常驻不增长）。

### 10.3 发布门
`pnpm check`（含新增前端测试/i18n 92+ keys）、`cargo check/test`、本机发布构建体积门 ≤8.5MB、`--selftest`、真机截图/录屏证据存档；发 v1.5.0。

## 11. 实现阶段（写入计划）

1. **可行性冲刺（最小改动）**：面板 `disable_drag_drop_handler` + 注册一个只打日志的 `IDropTarget`，用 C# 工具验证子 HWND 能收到 `CF_HDROP` 与 `CF_DIB`。失败则停下来重新设计（回退方案：Tauri 路径事件 + 位图不支持，或独立投放窗口）。
2. **`image_io` 抽取**：从剪贴板模块抽出共享函数，剪贴板行为不变，测试全绿。
3. **dropin 入库**：格式提取 + 解码 + 去重 + 落盘 + `ItemKind::Dropped` + state 广播。
4. **拖拽门控 + reveal 集成**：状态机、顶部唤出、`DragHold`、`DropReveal/DropEnd`、全屏/捕获抑制。
5. **前端反馈**：事件监听、高亮、提示 pill、i18n、结果提示。
6. **测试与真机验证**：单测、C# 拖拽工具、微信/QQ/Explorer 实测、回归矩阵、修缺陷。
7. **发布 v1.5.0**：CHANGELOG/README/spec Amendment/release notes、体积门、selftest、CI、发布后核验。

## 12. 风险与回退

| 风险 | 概率 | 缓解 |
|---|---|---|
| WebView2 子 HWND 注册后仍无法收到投放 | 低（wry 同策略） | 阶段 1 先验证；失败回退 Tauri 事件（只支持文件路径）+ 文档说明微信位图不支持 |
| 拖拽门控误伤普通点击/拖选 | 中 | 严格"按下+位移"条件；只在面板 rect 内关闭穿透；真机回归覆盖 |
| 面板在拖拽期间抢焦点/闪烁 | 低 | 保持 `focusable(false)`；reveal 用现有动画；drop 期间不做 show/hide 抖动 |
| 大图/多图卡顿 | 中 | 提取与解码分离到 worker、上限限制、单次 ≤20 |
| 与拖出会话互相干扰 | 中 | `dragout::is_dragging()` 时门控完全让位；松手恢复延迟 |
| 微信只给位图时质量/尺寸差异 | 低 | 统一 RGBA→PNG，保留原始像素 |

**明确不做**：修改拖出行为、新增可见窗口、网络下载、拖入排序、视频投放。
