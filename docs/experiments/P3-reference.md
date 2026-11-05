# P3 参考记录：长按手势、标注写回与录屏裁剪

- **任务**：P3-1「Long-press gesture, pure editor helpers, i18n」
- **分支 / 基线**：`feat/core`，P3 计划提交 `6d2a5ec`（父提交 `5d89ac6`）
- **日期**：2026-11-03
- **范围**：设计规格 §3.1（交互总表）、§3.7（标记编辑器）、§3.8（录屏与裁剪）、§7（M12/M13）
- **结论**：P3 的长按阈值取 **0.45 s / 4 dp（超过才取消）**；标注窗口按图片等比缩放到工作区 **80%**，上限 **1400×900**、下限 480×360；标注与裁剪都走「同目录临时文件 → 原子替换原名 → 刷新缩略图并广播状态」的写回管线。本文件是参考记录，不是运行时验收证据；M12/M13 的实测结果将记录在 `docs/experiments/P3-markup-record.md` 与 `docs/acceptance/P3-core.md`，在用户手工验证前保持 **MANUAL-PENDING**。

## 1. 目的与边界

P3 需要在不引入新依赖的前提下，用 Windows 原生能力替代 macOS 的 Markup 与 AVKit 裁剪条。本记录做三件事：

1. 把规格与计划里的数值（0.45 s、4 dp、80%、1400×900、2 GB/60 min）固化成可直接引用的表，供 P3-1 的纯函数与后续任务使用；
2. 记录参考实现（Tendedero `GrabArea.swift` / `Markup.swift` / `Trim.swift`）的真实行为与行号，明确哪些只是行为参考、哪些 API 不能照搬；
3. 提前列出写回/裁剪管线、V2 排除项与已知风险，避免后续任务自行发明阈值或降级策略。

边界：本任务只新增 `longPress.ts`、`editorKeys.ts`、i18n 键与本文档；不实现编辑器 UI、Rust 命令、窗口管理，也不在本机运行任何 macOS 参考代码。参考 Swift 文件是工作区内的只读副本，不属于本仓库，也不随应用分发。

## 2. 参考来源

| 来源         | 位置                                                                                                     | 用途                                             |
| ------------ | -------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| 设计规格     | `docs/superpowers/specs/2026-10-28-hangline-windows-design.md` §3.1/§3.7/§3.8/§7                         | 产品行为与阈值的最终依据                         |
| P3 实现计划  | `docs/superpowers/plans/2026-11-03-hangline-windows-markup-record.md`（Reference data、Task P3-1..P3-8） | 文件划分、IPC、写回/裁剪细节                     |
| 长按参考     | `research/ui/src/GrabArea.swift`                                                                         | 0.45 s 定时器、4 px 拖拽取消、点击/双击/拖拽仲裁 |
| 标注写回参考 | `research/ui/src/Markup.swift`                                                                           | 系统 Markup 的返回值处理与原子写回               |
| 裁剪参考     | `research/ui/src/Trim.swift`                                                                             | 裁剪窗口比例、AVKit 裁剪条、临时文件与替换       |
| 既有纯函数   | `src/windows/panel/dragThreshold.ts`、`src/lib/cardGesture.ts`                                           | 4 dp 径向阈值、延迟单击的实现风格                |

> 证据口径：本文的 Swift 行号均指工作区根目录 `research/ui/src/` 的本地副本；产品数值以规格/计划为准，Swift 只提供行为参照。未在 Windows 上实测的项不写成已测。

## 3. 参考数据表（计划 Reference data）

| 项目       | 数值 / 行为                                                                                                  | 来源                                                      |
| ---------- | ------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------- |
| 长按触发   | 0.45 s 持续按住（P2 已有按下缩放 0.95 反馈）                                                                 | spec §3.1/§3.7；`GrabArea.swift:108`                      |
| 长按取消   | 截止前抬指/离开，或移动 **超过** 4 dp（与拖出同阈值）                                                        | spec §3.1；`GrabArea.swift:169-172`；`dragThreshold.ts:2` |
| 编辑器尺寸 | 图片等比缩放到工作区 80%，最大 1400×900，无边框深色窗口 + 自定义标题栏                                       | spec §3.7                                                 |
| 工具       | 画笔（3 档粗细、6 色）、矩形、箭头、文字（多行）、裁剪、撤销/重做 ≥ 20、保存/取消                            | spec §3.7                                                 |
| 快捷键     | Ctrl+Z / Ctrl+Y 撤销重做，Ctrl+S 保存，Esc 取消（有改动时二次确认）                                          | spec §3.7                                                 |
| 标注写回   | 合成全分辨率 PNG → 同目录临时文件 + 原子替换 → 刷新缩略图 → `onSaved`                                        | spec §3.7                                                 |
| 录屏卡片   | 首帧 + 播放角标；单击复制文件（`CF_HDROP`，不写 PNG）；双击默认播放器；长按裁剪                              | spec §3.8                                                 |
| 裁剪窗口   | HTML5 `<video>` 预览 + 双滑块（起止）+ 时长；≤ 2 GB / ≤ 60 min，超出提示不支持                               | spec §3.8                                                 |
| 裁剪导出   | WinRT `Windows.Media.Editing.MediaComposition` + `MediaComposition.RenderToFileAsync`（默认 `MediaEncodingProfile`），临时文件，`ReplaceFile`/rename，进度事件 | spec §3.8                                                 |
| M12        | 标注保存替换原图，缩略图立即刷新，编辑器内 Ctrl+Z 可用                                                       | spec §7.2                                                 |
| M13        | Game Bar 录屏被收上栏，裁剪后文件可播放且时长符合预期                                                        | spec §7.2                                                 |
| P3 DoD     | M12/M13 通过；标注保存失败率 0（100 次压力）                                                                 | spec §8                                                   |

## 4. 长按模型与点击/拖拽仲裁

### 4.1 纯控制器语义（P3-1）

`createLongPress({ holdMs?, moveThresholdPx?, onLongPress })` 返回 `pointerDown / pointerMove / pointerUp / cancel / held`，全部状态在闭包内，不依赖 DOM 或真实时钟（通过 `globalThis.setTimeout/clearTimeout`，测试用 fake timers）。

| 事件                          | 行为                                                                               |
| ----------------------------- | ---------------------------------------------------------------------------------- |
| `pointerDown(x, y)`           | 清除旧计时器、记录起点、`held=false`、启动 450 ms 计时（传入 `holdMs` 时用传入值） |
| 到期且未取消                  | `held=true`，调用一次 `onLongPress()`；该次按压的后续 move/up 全部忽略             |
| `pointerMove` 径向距离 > 4 px | 取消本次长按，不再触发                                                             |
| `pointerMove` 径向距离 ≤ 4 px | 保持等待（**恰好 4 px 不取消**，与 `dragThreshold.ts` 的严格大于一致）             |
| `pointerUp`（截止前）         | 取消，不触发                                                                       |
| `pointerUp`（触发后）         | 无操作；`held` 仍为 true，供调用方吞掉随后那次 click                               |
| `cancel()`                    | 取消待触发计时；触发后调用无副作用                                                 |
| 下一次 `pointerDown`          | 重新武装：`held=false`，重新计时；因此第二次长按可以再次触发                       |

`held` 的定义是「本次长按已触发且尚未被下一次按下重置」，不是「指针当前按着」。这样卡片可以在 `click` 处理器里检查 `held`，避免长按松手后误触复制。调用方需在 `pointercancel`、`pointerleave`、卸载、取下线、窗口失焦时调用 `cancel()`。

### 4.2 点击 / 双击 / 拖拽仲裁（参考 `GrabArea.swift`）

参考实现把四种手势放在同一个 `GrabView` 上，顺序很关键：

1. `mouseDown` 先处理双击与左上角叉，再记录起点、启动 0.45 s 的 `holdTimer`；计时器回调只有在 `downPoint != nil && !startedDrag` 时才触发长按（`GrabArea.swift:123-151`）。
2. `mouseDragged` 在径向距离 `> 4` 时置 `startedDrag`、结束按压并开始原生拖拽；一旦拖拽开始，长按计时器已被取消（`GrabArea.swift:168-182`）。
3. `mouseUp` 只有在「按下过、没拖拽、没长按、单击」时才启动延迟复制；延迟时长是系统双击间隔 `NSEvent.doubleClickInterval`，避免双击首击污染剪贴板（`GrabArea.swift:184-203`）。
4. 长按触发时先清掉 0.95 按压姿态（`onPressChange(false)`），再打开编辑器；规格还要求「长按中」面板不收起（spec §3.1 第 86 行）。

Windows 侧对应关系：延迟单击复用 `cardGesture.ts` 的 `createCardGesture`；4 px 阈值复用 `dragThreshold.ts` 的 `exceedsDragThreshold`；P3-1 的长按控制器只负责 0.45 s 与移动取消，P3-4 负责把它接到 `GlassCard` 的 pointer 事件上，并在长按触发后取消待执行的单击复制。macOS 的 force click / Quick Look 不移植（Windows 用双击默认播放器/打开）。

## 5. 标注编辑器窗口与尺寸

规格只给出比例与上限（spec §3.7）：图片等比缩放到工作区的 80%，最大 1400×900；计划进一步给出最小 480×360，并要求无边框、深色、自定义标题栏、居中到条目/面板所在显示器（plan Task P3-3）。

计算顺序建议：

1. 取目标显示器 `work_area`（物理像素），换算成前端 CSS px（除以 `scaleFactor`）；图像尺寸 `imgW/imgH` 用**图像自然像素**，不乘 DPI。150% 缩放下同一个工作区换算出的 CSS px 更小（例如 2560 物理宽 → 1706.7 CSS px），窗口物理像素 = 内容 CSS px × 1.5，因此屏幕上仍占工作区的 80%，但窗口的 CSS 尺寸更小；
2. `scale = min(workW * 0.8 / imgW, workH * 0.8 / imgH, 1400 / imgW, 900 / imgH)`；
3. 窗口内容尺寸 = `clamp(round(imgW * scale), 480, 1400) × clamp(round(imgH * scale), 360, 900)`：**先按比例缩放，再做最小钳制**。普通截图保持图片宽高比；极端比例（如 100×20000 的长条）会触发 480×360 的最小值，单轴钳制会破坏窗口比例，此时图片自身比例由 `object-fit: contain` 保持，窗口比例不再等于图片比例；最后再把宽高钳到工作区（工作区小于 480×360 时以工作区为准）；
4. 图片以 `object-fit: contain` 铺满内容区，画布绝对定位到同一显示矩形；保存时再按自然像素重绘，显示坐标与图像坐标分离（P3-2 的 `toImagePoint`）。

macOS 参考实现没有可复用的窗口几何：`Markup.swift` 直接调用系统 Markup 扩展（`Markup.swift:15-28`），窗口由系统决定。因此 80%/1400×900 是 Hangline 自己的设计值，不是从 Swift 抄来的数值。150% DPI 与亮/暗主题下的布局属于手工验收项。

## 6. 标注写回管线

1. 长按/打开 → Rust 校验条目存在，返回 `{ item, owned, theme }`；`owned` 仅当路径在 `%LOCALAPPDATA%\Hangline\Screenshots` 内为 true。
2. 前端加载原图，按显示尺寸绘制形状；撤销栈上限 ≥ 20（计划取 `MAX_HISTORY = 24`）；文字支持多行；裁剪保存偏移量。
3. 保存 → 离屏画布按图片自然尺寸重绘（形状按 `scale` 放大、应用 crop offset）→ `toDataURL("image/png")` → base64 → `save_annotation(id, pngBase64)`。
4. Rust：再次校验 `owned`（前端禁用不是安全边界）→ base64 解码 → `image::load_from_memory` 得 RGBA8 → 限制 12000×12000 与 128 MB 解码后大小 → 重新编码 PNG。
5. 在目标同目录写 `.hangline-markup-<uuid>.png.tmp`，`sync_all` 后 rename 覆盖原文件；任何一步失败都删除临时文件并返回错误，原文件保持不变。
6. 成功后计算 BLAKE3、强制重建 432 px 缩略图、更新 width/height/hash/`updated_at = now`、`replace_item` 持久化并广播 state。
7. 前端关闭编辑器；面板用 `assetUrl(path/thumbPath, updatedAt)`（`?v=<updatedAt>`）重新取图，绕开 WebView2 的旧字节缓存，无需重启。
8. 失败时编辑器保持打开并显示 `Save failed`；只读条目保存按钮禁用并显示 `Read-only (watched folder)`。

参考 `Markup.swift` 的返回值处理：系统扩展可能交回 `URL`、`NSImage` 或 `NSItemProvider`，宿主逐一处理并优先原格式（`Markup.swift:45-80`），最终 `data.write(to: target, options: .atomic)`（`Markup.swift:90-98`）。Windows 没有系统扩展，改为自绘画布并回传 PNG，但「原子写、失败不碰原文件、成功后回调刷新」的语义一致。P3-3 的 100 次保存压力测试要求 0 失败且无 `.tmp` 残留。

## 7. 录屏裁剪管线

1. 监听 `%USERPROFILE%\Videos\Captures`，新 mp4/mov 上栏；首帧通过 WinRT `MediaComposition` + `MediaClip::CreateFromFileAsync` + `GetThumbnailAsync` 生成 432 px 预览，同时修正宽高与时长；失败则保留文件名占位卡。
2. 卡片显示首帧 + 播放角标；单击复制文件路径（`CF_HDROP`，不生成 PNG），双击默认播放器，长按打开裁剪窗口。
3. `get_trim_target` 返回 `{ item, sizeBytes, durationMs, maxBytes, maxDurationMs, theme }`；前端 `<video>` 预览，`loadedmetadata` 后显示时长，双滑块以 10 ms 为步长，结束时间必须严格大于开始时间 + 100 ms（`end > start + 100 ms`；`end - start = 100 ms` 本身不合法，P3-7 的纯校验按此边界实现）。
4. 超过 2 GB 或 60 min 时禁用保存并显示对应文案；解码/播放失败时显示错误和「用默认播放器打开」回退按钮。
5. `trim_video(id, startMs, endMs)` 先做纯校验：`TooLarge`、`TooLong`、`EmptyRange`、`RangeOutOfBounds`，边界为 `end > start + 100 ms` 且 `end ≤ duration`；再在专用线程初始化 MTA，调用 `MediaComposition` 追加 clip、设置 `TrimTimeFromStart`/`TrimTimeFromEnd`，`RenderToFileAsync` 到同目录临时文件，期间广播 `trim-progress`（粗粒度 0.05 / 0.6 / 1.0）。
6. 渲染成功后原子替换原文件，重建首帧预览、更新时长/尺寸/`updated_at`、广播 state；任何失败删除临时文件，原文件不变，前端保留窗口并显示 `Trim failed` + 回退按钮。

参考 `Trim.swift`：窗口按视频比例缩放到 720×520 以内（`Trim.swift:55-60`），等 `readyToPlay` 后才弹 AVKit 裁剪条（`Trim.swift:37-52`），用 passthrough 预设导出到隐藏临时文件 `.trimming-<uuid>.<ext>`（`Trim.swift:73-85`），成功后 `replaceItemAt`（`Trim.swift:88-90`），失败删除临时文件（`Trim.swift:92-95`）。Windows 侧的关键差异是 **MediaComposition 导出会重新编码**（不是 passthrough），所以必须考虑编解码器可用性与时长取整误差；窗口也不是 AVKit 裁剪条，而是自绘双滑块。

## 8. V2 排除项

- **不做马赛克、序号、高亮笔**（spec §3.7 末尾）；P3 工具固定为画笔/矩形/箭头/文字/裁剪。
- **不做自录屏**：V1 不内置录屏，托盘只提供「用 Xbox Game Bar 录制（Win+Alt+R）」提示（spec §3.8）。
- **不移植 macOS 专属交互**：系统 Markup 扩展、AVKit 裁剪条、force click / Quick Look、原生分享服务均不适用。
- **右键「标记/裁剪」入口不在 P3 范围**：P3 以长按为主入口；上下文菜单入口列入 V1.1（plan Task P3-8 已知限制）。
- **不做无损 passthrough 裁剪**：WinRT 路线接受重新编码带来的画质/时长误差，换取系统自带编解码支持。

## 9. 已知风险与待验证项

| 风险                               | 影响                                      | 处理 / 验证                                                                                  |
| ---------------------------------- | ----------------------------------------- | -------------------------------------------------------------------------------------------- |
| WinRT 编解码器支持不全             | HEVC/AV1 等源可能裁剪失败，H.264/AAC 最稳 | 失败走错误 + 默认播放器回退；运行时用 ffmpeg 生成的 H.264 fixture 验证                       |
| DRM / 受保护内容                   | `MediaComposition` 可能拒绝打开或渲染     | 明确报错、绝不改原文件；不做绕过                                                             |
| 监听文件夹只读                     | 外部目录条目不允许写回                    | `owned=false` → 保存禁用 + `Read-only (watched folder)`；Rust 再次校验；手工验收第 5 项      |
| 超过 2 GB / 60 min                 | 裁剪窗口无法处理                          | 打开即禁用保存并提示；纯校验单测覆盖边界，运行时不生成超大 fixture                           |
| 首帧提取失败                       | 录屏卡片没有预览                          | 保留文件名占位卡；裁剪仍可按路径打开                                                         |
| HEIC / 画布不支持的图片            | 编辑器无法绘制像素                        | 显示占位与文件名、禁用保存（P3-4 明确 disabled 状态）；手工验证                              |
| 原子替换失败（播放器占用、磁盘满） | 保存/裁剪失败                             | 同目录临时文件 + rename；失败删除临时文件、原文件不变；提示错误                              |
| WebView2 字节缓存                  | 保存后卡片仍显示旧图                      | `?v=<updated_at>` 版本化 URL；P3-3 检查 DOM src 与图片尺寸变化                               |
| 150% DPI / 多显示器                | 窗口超出工作区或坐标偏移                  | 按目标显示器工作区计算、物理像素 ↔ CSS px 换算；手工验收第 9 项                              |
| 长按与拖拽/单击抖动                | 误开编辑器或误复制                        | 严格 `> 4 px` 径向取消、触发后吞掉 click、`pointercancel/leave` 取消；单测 + CDP 500 ms 长按 |
| MTA/COM 线程模型                   | 裁剪阻塞 UI 或 COM 初始化失败             | 专用线程 `RoInitialize(MTA)`、命令异步；进度事件粗粒度                                       |
| 保存失败率                         | P3 DoD 要求 0/100                         | P3-3 100 次循环保存，断言全部成功且只有一个文件、无 `.tmp`                                   |

尚未验证（本任务不做）：Windows 上 MediaComposition/WinRT 导出对具体 Game Bar 文件的兼容性、DRM 行为、首帧 API 的实际返回、150% DPI 下的窗口尺寸；这些都需要 P3-5/P3-7 的运行时证据，当前只能标注为待测，不能写成通过。

## 10. 与后续任务的衔接

- **P3-1（本任务）**：`createLongPress` 默认 450 ms / 4 px；`editorKeys` 覆盖 Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z / Ctrl+S / Esc；i18n 提供编辑器与裁剪窗口全部文案（en / zh-Hans 键集合一致）。
- **P3-2/P3-3**：标注编辑器 UI、`annotation.ts` 纯模型、Rust 原子写回与版本化资源；窗口尺寸按 §5，写回按 §6。
- **P3-4**：把长按控制器接到 `GlassCard`，左键、非叉号、非拖拽才启动；触发后取消待复制并抑制 click；只读条目按 §9 处理。
- **P3-5/P3-6/P3-7**：首帧、裁剪窗口、WinRT 裁剪分别按 §7 实现；M12/M13 的运行时证据在 `P3-markup-record.md`，手工项在 `P3-core.md` 保持 MANUAL-PENDING。
- **P3-8**：README/验收文档引用本表数值；除用户手工确认外，不把风险项改写成 PASS。
