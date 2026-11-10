# Hangline（晾图绳）Windows 版 — 设计文档

> 状态：**待评审**（review 通过后进入实现计划）
> 日期：2026-10-28
> 作者：Hangline 项目组（DeepSeek Harness session）
> 灵感来源：[Tendedero](https://github.com/alejandrobujan/tendedero)（macOS，MIT + 名称/图标保留条款）
> 关联调研：`%USERPROFILE%\Documents\deepseek-harness\default-workspace\tendedero-深度解析.md`

---

## 1. 背景与目标

### 1.1 背景

Tendedero 用一条「晾衣绳」把 macOS 截图从桌面搬到屏幕顶栏：截图自动飞上绳、鼠标靠到菜单栏线滑下来、单击复制/长按标注/拖走或丢弃。21 天 2,627 stars，证明了这个产品形态成立。

Windows 没有等价的「系统截图落盘钩子」，也没有菜单栏，但截图痛点相同（桌面堆满 `Screenshot 2026-10-28 103211.png`、截图工具用完即忘、临时截图需要二次寻找）。Hangline 是这一形态的 Windows 独立实现。

### 1.2 许可边界（硬约束）

Tendedero 的 LICENSE 为 MIT + 「Name and icon」附加条款：修改/再分发版本**必须换名换图标**，且不得暗示官方背书。因此：

- 产品名 **Hangline**，中文展示名「晾图绳」；图标必须为原创（代码/SVG 绘制，不复制 Tendedero 图标）；
- 代码为独立实现，V1 不复制 Tendedero 源码；**允许**移植其 11 语言翻译文本（MIT 覆盖，需在 `THIRD_PARTY_NOTICES.md` 注明版权与出处）；
- README 明确写「inspired by Tendedero」，不与官方产生混淆。

### 1.3 目标（V1 发布）

> **Amendment 2026-11-09 — 内存口径修订（双口径）**
>
> **性质：目标口径修订，不是实现回归。** 2026-10-28 的目标「常驻内存目标 ≤ 150 MB」在 v1.1.0 release 构建的真机实测中**只在「仅主进程」口径成立**：
>
> | 口径 | 实测值 | 说明 |
> |---|---|---|
> | 主进程（`hangline.exe`）RSS / Private | **39.4 MB / 12.7 MB** | 只算主进程本身 |
> | WebView2 子进程（7 个，均属 `app.hangline.Hangline`）RSS | **531.6 MB** | 单进程 **20–135 MB** 量级 |
> | 进程树总 RSS | **571.0 MB** | 任务管理器「应用」分组的真实观感 |
> | 进程树总 Private（私有内存） | **293.8 MB** | 较少受共享页重复计数影响，选作主指标 |
>
> **自 2026-11-09 起采用双口径：**
>
> 1. **主进程 ≤ 150 MB —— 必须**（硬门；只优化主进程数字而进程树失控，视为不达标）；
> 2. **进程树私有内存 ≤ 350 MB —— 暂定目标**（超出必须评审并记录理由，不得静默放过）。
>
> **新增可视面的预算规则**：任何新增 WebView2 可视面（贴图 Pin / 命令面板 / 录制 HUD）按 **+70–135 MB/窗口** 估算；**立项前必须给出测量**，不得沿用「+10 MB」的旧直觉。
>
> **证据**：[W0 真机验收记录（2026-11-09）§2「内存基线」](../../acceptance/W0-real-machine-2026-11-09.md)（主进程 39.4 MB / 进程树私有 293.8 MB 的原始测量与复现方式）。

1. **零学习成本**：保留用户原有截图习惯（Win+PrtScn / Win+Shift+S / Xbox Game Bar），同时提供自己的全局热键截图。
2. **顶栏货架**：截图自动上栏，悬停/热键展开，离开收起；单击复制、长按标注、双击打开、拖出到任意 app/文件夹/回收站、点叉删除。
3. **像 Tendedero 一样有「灵魂」**：玻璃卡片、铝夹、细绳、微风摆动、飞入/坠落动画。
4. **可发布**：NSIS 安装包 + portable zip、签名策略、自动更新、11 语言（V1 先 en/zh-Hans，其余随迭代）、README + press kit。
5. **轻**：安装包目标 ≤ 15 MB，常驻内存目标 ≤ 150 MB，空闲 CPU < 1%。

> **原文，2026-11-09 修订见上。** 第 5 条「常驻内存目标 ≤ 150 MB」为 2026-10-28 原文，保留不改；现行双口径（主进程 ≤150 MB 必须 / 进程树私有 ≤350 MB 暂定）见本节开头的 **Amendment 2026-11-09 — 内存口径修订**。

### 1.4 非目标（V1 不做）

- 自研录屏编码器（V1 监听 Xbox Game Bar 的 `视频\Captures` 目录；自录在 V2 评估）。
- 内置截图编辑器的高级能力（仅 画笔/矩形/箭头/文字/裁剪/撤销；马赛克、序号、滚动截图放 V2）。
- OCR、云同步、账号、移动端、插件系统、多用户。
- 自动更新增量包（V1 全量）。
- App Store / Microsoft Store 上架（V1 只做 GitHub 直发）。

### 1.5 成功标准（V1 验收）

- 一台干净 Windows 11 机器：安装 → 首次启动引导 → 按 `Ctrl+Alt+S` 框选 → 截图飞上顶栏 → 单击复制到微信/浏览器 → 拖到桌面文件夹 → 长按标注写回 → 拖到回收站 → 退出后截图设置无残留。
- 双屏 + 125% / 150% 缩放混合环境无错位。
- `Ctrl+Alt+S` 与常见软件（QQ、微信、Snipaste、浏览器）默认热键不冲突；冲突时托盘菜单提示且可改键。
- 安装包 ≤ 15 MB；冷启动到托盘 ≤ 2 s。

> **内存验收口径（2026-11-09 修订）**：按 §1.3「Amendment 2026-11-09 — 内存口径修订」执行——**主进程 ≤150 MB 必须；进程树私有内存 ≤350 MB 暂定**。原文「常驻内存 ≤150 MB」为 2026-10-28 原文，保留不改；实测数据见 [W0 真机验收记录](../../acceptance/W0-real-machine-2026-11-09.md)。
>
> **按需窗口（搜索窗）的内存验收口径（2026-11-10 增补）**：见 §9「**Amendment 2026-11-10 — 按需窗口与检索**」——搜索窗按需创建、关闭即销毁；关闭后进程树私有内存回落到打开前 **±30 MB**，常驻（未打开）不高于 **293.8 MB**。

---

## 2. 用户与场景

**目标用户**：每天截图 5–50 次的 Windows 用户（产品/设计/开发/运营/客服），已在用 Win+Shift+S 或微信/QQ 截图，桌面或下载文件夹里堆满截图。

**核心场景**

| # | 场景 | 用户动作 | Hangline 行为 |
|---|---|---|---|
| S1 | 临时截图发给同事 | `Ctrl+Alt+S` 框选 → 单击卡片 → 在微信 `Ctrl+V` | 截图入自有目录并上栏；单击写入剪贴板（PNG + 文件 URL） |
| S2 | 截图放桌面备用 | `Win+PrtScn` | 目录监听捕获 `图片\Screenshots` 新文件并上栏 |
| S3 | 用系统截图工具后自动保存 | `Win+Shift+S`（开启自动保存） | 目录或剪贴板链路捕获（去重后只上一条） |
| S4 | 截图标注重发 | 长按卡片 0.45 s | 内置标注器打开 → 画箭头/文字 → 保存 → 原文件原子替换、缩略图刷新 |
| S5 | 截图拖进 Figma/微信 | 按住卡片拖到目标窗口 | 原生文件拖出（`CF_HDROP`），目标拿副本，卡片保留 |
| S6 | 截图归档到文件夹 | 拖到资源管理器文件夹 | 文件被移动 → 卡片 0.6 s 后掉落，**不删除**文件 |
| S7 | 截图不要了 | 拖到回收站 / 点左上角叉 | 移入回收站（可撤销） |
| S8 | 录屏发给同事 | `Win+Alt+R` 录制 → 停止 | `视频\Captures` 新 mp4 上栏，带播放角标；长按进入裁剪 |
| S9 | 全屏游戏/演示时不想被打扰 | — | 检测到全屏应用自动隐藏顶栏 |
| S10 | 找一张一小时前的截图 | 悬停展开 → 双指横向滚动 | 线支持滚轮/触控板横向滚动，容量 25/50/100 |

---

## 3. 产品规格

### 3.1 交互总表

| 操作 | 行为 | 备注 |
|---|---|---|
| 单击卡片 | 复制：写 PNG + 文件 URL 到剪贴板，卡片弹「Copied」胶囊 1.2 s | 单击延迟 `doubleClickInterval` 执行，避免双击首击污染剪贴板 |
| 双击 | 用默认程序打开文件 | 不复制 |
| 长按 0.45 s | 打开标记编辑器（图片）/ 裁剪窗口（视频） | 按下时卡片缩放 0.95 反馈 |
| 右键 | 上下文菜单：复制 / 打开 / 在资源管理器中显示 / 标记（视频：裁剪）/ 取下线（删除文件可选）/ 取下线左侧全部 / 右侧全部 | 菜单打开期间线不收起 |
| 拖动卡片 | 原生文件拖出 | 拖拽期间禁止点击穿透；无人接收时飞回原位 |
| 左上角叉（26 dp 热区） | 移入回收站并从线上移除 | inbox 模式（监听系统目录）下只下线不删 |
| 悬停展开 | Windows 默认**无悬停触发**（可选「顶边停留」）；由热键 / 新截图 / 托盘触发 | 见 3.4 |
| 离开线区域 0.5 s | 收起 | 拖动中、菜单打开、长按中、peek 未结束时不收 |
| 点击线以外任意处 | 收起 | 30 Hz 鼠标轮询检测左键按下 |
| `Ctrl+Alt+S`（默认） | 区域截图：冻结屏幕 → 框选 → 存盘 → 上栏 | 可在托盘改键；冲突时回退并提示 |
| `Ctrl+Alt+T`（默认） | 显示/隐藏线 | 可在托盘改键 |
| 滚轮 / 横向滚动 | 查看更早的卡片（保留 25/50/100 时） | 最新在右，滚轮向左 |

### 3.2 截图来源（三链路 + 去重）

| 来源 | 实现 | 默认 | 说明 |
|---|---|---|---|
| A. 自建热键截图 | Rust：GDI `BitBlt` 抓虚拟屏幕 → 全屏冻结覆盖层（WebView2 显示截图）→ 用户拖框 → `image` 库裁剪存 PNG → 上栏 | 开 | 不修改系统任何设置；支持 Esc 取消；多屏框选 |
| B. 系统截图目录 | `notify` 监听 `%USERPROFILE%\Pictures\Screenshots`（可配置最多 3 个额外目录） | 开 | Win+PrtScn / 截图工具「自动保存」落盘即上线；只认图片扩展名 |
| C. 剪贴板 | `AddClipboardFormatListener` 事件驱动 + 300 ms 轮询兜底 | 开 | Win+Shift+S、微信/QQ 纯图片复制；浏览器「复制图片」默认不收（带 HTML/文本，见 4.6 D）；过滤规则见 4.7 |
| D. 录屏目录 | `notify` 监听 `%USERPROFILE%\Videos\Captures` | 开 | Xbox Game Bar 录制 mp4 上栏 |

**去重规则**（避免同一张图从 B+C 上两次）：

1. 计算文件内容哈希（BLAKE3，64 位截断）与尺寸；维护最近 20 条哈希 LRU。
2. 自建截图 A 在存盘前写入「待认领哈希」，B/C 在 5 s 内命中同哈希则丢弃。
3. 同路径重复事件按 `(path, mtime, size)` 去重。
4. 剪贴板图片若与线上任一条哈希相同且文件仍存在，不重复上栏。

**不监听**：Desktop、Downloads 等用户目录（避免把普通图片挂上）；只监听显式配置的目录。

### 3.3 顶栏面板几何

以「逻辑像素（dp）」设计，渲染时乘以当前显示器 `scaleFactor`：

- 面板位置：目标显示器工作区顶部（`work_area.top`），宽 = `work_area.width`，高 = `photoMaxHeight + 106`。
- 尺寸档：`scale = 0.8 / 1.0 / 1.35` → `cardWidth = 150*scale`（120/150/202）、`photoMaxHeight = 104*scale`（83/104/140）、面板高 189/210/246。
- 卡片间距：`spacing = cardWidth + 24`；每屏容量 `fits = max(3, min(12, floor((panelWidth - 200)/spacing)))`。
- 绳：`ropeTop = 10`，`sag = min(30, panelWidth*0.018)`，二次贝塞尔 `ropeY(t) = ropeTop + 4*sag*t*(1-t)`；三层描边 1.4/1.2/0.4 dp。
- 卡片圆角 16 dp、内衬 4 dp、随机倾斜 ±2.5°、夹子 9×26 dp。
- P2 视觉：照片最大宽 `cardWidth-14`、高 `photoMaxHeight`；卡片尺寸 = 照片 + 8 dp 内衬；夹子顶端在绳上方 9.5 dp、卡片与夹子重叠 12 dp（卡片顶 ≈ 绳 + 4.5 dp）；整组围绕夹子顶旋转。
- P2 交互：悬停缩放 1.035（阴影 0.26/r14/y8，常态 0.18/r10/y5），按下 0.95（450 ms ease-in-out）；复制轻摆 3°；空线时绳中央显示胶囊提示（i18n）。
- 展开位移 `-(panelHeight + 12)`，展开 `spring(response 0.42s, damping 0.82)`，收起 `easeIn 0.22s`；动画结束后窗口 `hide()`（不留隐形窗口）。

### 3.4 展开/收起状态机

状态：`wanted`（应出现）、`revealed`（已滑下）、`pinned`（手动召唤未收起）、`peekUntil`（新截图自动亮到何时）、`dismissedUntilLeave`（点击线外后抑制回弹，直到指针离开触发区）。

**展开路径**

1. 新截图/录屏上线且 `autoReveal=true` → `reveal(peekFor: 2.5s)`；
2. `Ctrl+Alt+T` 或托盘「显示线」→ `reveal(pinned: true)`；
3. 托盘「截图」完成后自动展开（同 1）；
4. 可选（默认关）：指针停留屏幕顶边 0.5 s 且 `topEdgeReveal=true` → 展开。

**收起路径**

1. 指针离开「面板 + 向下延伸 40 dp」区域 0.5 s；
2. 线为空且无 peek → 0.7 s 后收起并隐藏；
3. 点击线以外任意处 → 立即收起并置 `dismissedUntilLeave=true`（直到指针离开触发区才复位）；
4. `Ctrl+Alt+T` 再次按下。

**全屏抑制**：`SHQueryUserNotificationState` ∈ {`QUNS_RUNNING_D3D_FULL_SCREEN`, `QUNS_PRESENTATION_MODE`, `QUNS_BUSY`} 时 `wanted=false`（除非 `pinned` 且用户显式打开「全屏时也显示」）。Space 切换/前台应用变化后延迟 0.8 s 复检。

### 3.5 托盘菜单（完整树）

| 项 | 类型 | 行为 |
|---|---|---|
| 显示线 / 隐藏线（`Ctrl+Alt+T`） | 动作 | toggle，菜单文字随状态变 |
| 全部取下线 | 动作 | 清空线（文件进回收站，仅自有目录） |
| 截图…（`Ctrl+Alt+S`） | 动作 | 区域截图 |
| 处理系统截图 | 勾选 | 目录监听开关（B） |
| 挂起复制的图片 | 勾选 | 剪贴板监听开关（C） |
| 挂起录屏 | 勾选 | 录屏目录监听（D） |
| 线出现方式 ▸ | 子菜单 | 「新截图时」（勾）·「指针停在顶边时」（默认关）·「全屏应用上」 |
| 也监听文件夹 ▸ | 子菜单 | 已监听目录（点击打开）·「添加文件夹…」（上限 3）·「停止监听 ▸」 |
| 保留数量 ▸ | 子菜单 | 放得下多少（勾）/ 25 / 50 / 100 |
| 尺寸 ▸ | 子菜单 | 小 / 中（勾）/ 大 |
| 快捷键 ▸ | 子菜单 | 截图 `Ctrl+Alt+S`（点击修改）· 显示线 `Ctrl+Alt+T`（点击修改） |
| 打开截图文件夹 | 动作 | 资源管理器打开自有目录 |
| —— | 分隔 | |
| 声音 | 勾选 | 复制/丢弃提示音，默认开 |
| 开机启动 | 勾选 | `HKCU\...\Run`，默认关 |
| 检查更新… | 动作 | 手动检查（有更新则提示下载） |
| 语言 ▸ | 子菜单 | 跟随系统（勾）/ English / 简体中文 / … |
| —— | 分隔 | |
| 退出 Hangline | 动作 | 还原状态并退出 |

托盘图标：双击 = 显示线；右键 = 上述菜单；未读截图数用 tooltip 呈现，不改图标（避免视觉噪音）。

### 3.6 设置项与默认值

存储于 `%LOCALAPPDATA%\Hangline\settings.json`（原子写）。

| key | 类型 | 默认 |
|---|---|---|
| `captureHotkey` | string | `"Ctrl+Alt+S"` |
| `toggleHotkey` | string | `"Ctrl+Alt+T"` |
| `watchSystemScreenshots` | bool | `true` |
| `watchClipboard` | bool | `true` |
| `watchRecordings` | bool | `true` |
| `extraFolders` | string[] | `[]`（≤3） |
| `revealOnNew` | bool | `true` |
| `revealPeekMs` | int | `2500` |
| `topEdgeReveal` | bool | `false` |
| `showOverFullScreen` | bool | `false` |
| `keepOnLine` | `"fit"` 或 25 / 50 / 100 | `"fit"` |
| `cardSize` | `"small"\|"medium"\|"large"` | `"medium"` |
| `sound` | bool | `true` |
| `startAtLogin` | bool | `false` |
| `language` | string | `"system"` |
| `theme` | `"system"\|"light"\|"dark"` | `"system"` |
| `autoCheckUpdates` | bool | `true`（每日一次） |
| `lastUpdateCheck` | ISO8601 | `""` |

### 3.7 标记编辑器（替代 macOS Markup）

- 触发：长按图片卡片 0.45 s / 右键「标记」。
- 窗口：无边框深色窗口，尺寸 = 图片等比缩放到工作区 80%，最大 1400×900。
- 工具：**画笔**（3 档粗细、6 色）、**矩形**、**箭头**、**文字**（单击输入，支持多行）、**裁剪**、撤销/重做（≥20 步）、保存/取消。
- 输出：保存时把画布合成 PNG，原子替换原文件（`temp + rename`），然后刷新缩略图并 `onSaved`。
- 快捷：`Ctrl+Z/Y` 撤销重做，`Ctrl+S` 保存，`Esc` 取消（有改动时二次确认）。
- 不做：马赛克、序号、高亮笔（V2）。

### 3.8 录屏与裁剪

- 上栏：`视频\Captures` 新 mp4/mov → 卡片显示首帧 + 播放角标；单击复制文件（`CF_HDROP`，不写 PNG）；双击默认播放器；长按进入裁剪。
- 裁剪窗口：HTML5 `<video>` 预览 + 双滑块（起止时间）+ 时长显示。
- 导出：Rust 调 WinRT `Windows.Media.Editing.MediaComposition`（`MediaClip.CreateFromFileAsync` + `MediaTranscoder`）按范围导出到临时文件，成功后 `ReplaceFile` 原子替换原文件；失败弹原生错误并回退「用默认播放器打开」。
- 上限：≤ 2 GB / ≤ 60 min；超出提示「文件过大，暂不支持裁剪」。
- V1 不自录屏：托盘提供「用 Xbox Game Bar 录制（Win+Alt+R）」提示项。

### 3.9 多显示器与 DPI

- **单面板窗口**，跟随指针所在显示器；显示器变化/换屏时 `set_position` + 重算尺寸。
- 所有窗口坐标使用物理像素；前端以 CSS px（= 逻辑 dp）布局，Rust 在发给前端的 payload 中附 `scaleFactor`，由前端换算。
- 跨不同 DPI 屏幕时**不移动已有卡片状态**（卡片数据与显示器无关），仅重建面板几何。
- 显示器布局变化（`WM_DISPLAYCHANGE`）→ 0.5 s 防抖后重算；若面板所在显示器消失，迁移到主屏。

### 3.10 语言与文案

- i18n 文件：`src/i18n/<lang>.json`，key 使用英文原文（同 Tendedero 习惯），缺失回落 `en`。
- V1 内置：`en`、`zh-Hans`；V1.1 移植 Tendedero 的 `es / gl / fr / pt-BR / ja / ko / tr / az / hi`（MIT，注明出处）。
- 所有 UI 文案禁止硬编码；构建时脚本校验 key 集合一致（`scripts/check-i18n.mjs`），多余/缺失 key 报错。
- 日期/数字用 `Intl`。

### 3.11 错误与空状态

- 空线：不显示卡片、不响应悬停展开（避免「拉下来什么都没有」）；托盘 tooltip「线上 0 张」。
- 文件丢失（用户在资源管理器删了）：`prune` 后卡片掉落（不报错）。
- 截图失败（GDI 失败/权限）：原生消息框 + 日志，不崩溃。
- 目录无权限：托盘菜单对应项变灰 + tooltip 说明。
- 热键冲突：注册失败 → 托盘气泡提示 + 保留旧键；改键窗口内实时校验冲突。

---

## 4. 技术架构

### 4.1 技术栈

| 层 | 选型 | 版本基线 |
|---|---|---|
| 壳 | Tauri 2 | `tauri = "2"`（当前 2.12.x） |
| 后端 | Rust stable（本机 1.90）+ `windows` crate 0.62 | edition 2021 |
| 前端 | React 19 + TypeScript 5 + Vite 7 | pnpm 10 |
| 状态 | 前端 Zustand（轻量）；Rust 为唯一数据源 | — |
| 样式 | 纯 CSS（CSS 变量 + `backdrop-filter` 模拟玻璃） | 无 UI 框架 |
| 动画 | 自研 rAF 弹簧积分器（复刻 Tendedero 参数） | — |
| 打包 | tauri-bundler → NSIS + portable | — |
| 更新 | `tauri-plugin-updater`（GitHub Releases `latest.json`） | — |

**Rust 依赖清单**：`tauri`、`tauri-plugin-global-shortcut`、`tauri-plugin-single-instance`、`tauri-plugin-autostart`、`tauri-plugin-updater`、`tauri-plugin-log`、`serde`/`serde_json`、`image`（PNG/JPEG/WebP 解码与缩略图）、`notify`（目录监听）、`blake3`（去重）、`thiserror`、`anyhow`、`time`、`base64`、`windows`（GDI/Shell/COM/DWM/WinRT Media）、`webview2-com`（如需原生 WebView2 句柄微调）。

### 4.2 进程/窗口模型

- 单进程；Tauri 主线程跑事件循环，Rust 后台线程：目录 watcher、剪贴板监听、鼠标轮询（面板可见时 30 Hz）、缩略图/哈希 IO（`spawn_blocking`）。
- 窗口（`WebviewWindow`）：
  | label | 用途 | 属性 |
  |---|---|---|
  | `panel` | 顶栏 | `transparent, decorations:false, alwaysOnTop, skipTaskbar, focusable:false, resizable:false, shadow:false`，初始 `visible:false` |
  | `fly` | 飞入/坠落覆盖层 | 全屏透明，`click-through`（始终 ignore cursor）；**P2：1 个预热复用窗口**（隐藏时不绘制），页内最多 2 个并发动画 |
  | `capture` | 区域截图 | 全屏、置顶、半透明遮罩，Esc 取消 |
  | `annotate` | 标注 | 普通窗口（可聚焦），`decorations:false` 自绘标题栏 |
  | `trim` | 视频裁剪 | 普通窗口（可聚焦） |
  | `shortcut` | 改快捷键 | 小而居中、置顶、可聚焦 |

- 面板窗口永不成为前台窗口（`WS_EX_NOACTIVATE`），点击卡片通过 WebView2 输入直接处理，不抢焦点。
- 飞入覆盖层：1 个预热复用的全屏透明窗口，页内最多 2 个并发动画；窗口始终点击穿透，不参与命中。P2 修订：避免 WebView2 冷启动造成飞入延迟（原「每飞一个窗口」方案放弃）。
- 单实例：二次启动唤起已有实例并 toggle 线。

### 4.3 模块划分（Rust）

```
src-tauri/src/
├── main.rs            入口、插件注册、setup
├── app_state.rs       AppState（items/settings/monitors/flags），Mutex 保护
├── settings.rs        settings.json 读写 + 默认值 + 校验 + 原子写
├── store.rs           items 持久化（state.json）+ 启动校验 + prune
├── watchers/
│   ├── dirs.rs        notify 监听（系统截图/录屏/自建目录/额外目录）+ 防抖 + 候选过滤
│   └── clipboard.rs   AddClipboardFormatListener + 轮询兜底 + 过滤链 + 保存
├── capture.rs         GDI 屏幕抓取、区域裁剪、PNG 写盘、屏幕矩形回传
├── geometry.rs        显示器/DPI/工作区/面板布局/命中矩形换算（纯函数，易测）
├── hittest.rs         30 Hz 鼠标轮询 → set_ignore_cursor_events
├── dragout.rs         Tauri start_dragging（失败回退自实现 OLE 拖源）
├── clipboard_io.rs    写 CF_DIBV5 + CF_HDROP + "PNG" 自定义格式
├── trash.rs           IFileOperation + FOF_ALLOWUNDO 移入回收站
├── annotate.rs        标注结果 PNG 原子写回 + 缩略图刷新
├── trim.rs            WinRT MediaComposition 裁剪 + ReplaceFile
├── thumbnails.rs      image 解码/缩放 + 自定义协议 `hangline://thumb/...` + LRU 缓存
├── tray.rs            托盘图标 + 完整菜单 + 状态同步
├── hotkeys.rs         注册/改键/冲突检测/回滚
├── fullscreen.rs      SHQueryUserNotificationState + 前台窗口判定
├── fly.rs             飞入窗口生命周期与并发上限
└── ipc.rs             command/event 定义与序列化类型
```

### 4.4 IPC 契约

**Commands（前端 → Rust）**

| command | 入参 | 返回 | 说明 |
|---|---|---|---|
| `get_app_state` | — | `PublicState { items, settings, version }` | items + settings；`monitors` 不走此 command，由 Task 10 的 `get_monitors` 提供 |
| `get_monitors` | — | `MonitorInfo[]` | Task 10：面板几何/多屏放置用，数据源 `geometry::monitors()` |
| `copy_item` | `id` | `Result<()>` | 写剪贴板（图片：PNG+文件；视频：文件） |
| `take_down` | `id, deleteFile: bool` | `Result<()>` | 移除卡片；`deleteFile` 时移入回收站 |
| `take_down_all` | — | `Result<()>` | 清空 |
| `open_item` | `id` | `Result<()>` | 默认程序打开 |
| `show_in_explorer` | `id` | `Result<()>` | `SHOpenFolderAndSelectItems` |
| `start_drag` | `id` | `Result<()>` | 原生拖出 |
| `request_capture` | — | `Result<()>` | 打开 capture 窗口 |
| `finish_capture` | `rect: PhysicalRect` | `Result<Item>` | 裁剪存盘并上栏 |
| `cancel_capture` | — | — | 关闭覆盖层 |
| `open_annotate` | `id` | `Result<()>` | P3：打开/复用标注编辑器窗口（80% 工作区、≤1400×900） |
| `get_editor_target` | `id` | `Result<EditorTarget>` | P3：编辑器加载目标（`{item, owned, theme}`；`owned=false` 为只读） |
| `save_annotation` | `id, pngBase64` | `Result<Item>` | P3：原子写回（限额 12000²/128 MB/64 MB payload；失败原图不动） |
| `close_editor` | — | — | P3：关闭标注编辑器窗口 |
| `open_trim` | `id` | `Result<()>` | P3：打开/复用裁剪窗口 |
| `get_trim_target` | `id` | `Result<TrimTarget>` | P3：`{item, sizeBytes, durationMs, maxBytes, maxDurationMs, theme}` |
| `close_trim` | — | — | P3：关闭裁剪窗口（导出中被拒绝） |
| `report_trim_ready` | — | `Option<String>` | P3：裁剪页握手并取回待显示 item id（目标重放） |
| `get_editor_state` | — | `{annotateOpen, trimOpen}` | P3：面板重载后的 busy 对账 |
| `trim_video` | `id, startMs, endMs` | `Result<Item>` | 异步导出，进度用事件 |
| `set_setting` | `key, valueJson` | `Result<AppState>` | 校验并持久化 |
| `set_hotkey` | `target: "capture"\|"toggle", accelerator` | `Result<()>` | 失败返回冲突 |
| `start_fly` | `id, from?: PhysicalRect, to: PhysicalRect, tiltDeg` | `Result<bool>` | P2：请求飞入；覆盖层忙/不可用返回 false，面板直接落卡 |
| `start_fall` | `id, imagePath, rect: PhysicalRect, tiltDeg` | `Result<bool>` | P2：坠落动画；覆盖层忙返回 false（卡片立即消失） |
| `fly_done` | `id` | — | P2：覆盖层动画结束（飞入成功时通知面板显示卡片） |
| `report_fly_ready` | — | `PhysicalRect` | P2：飞入覆盖层页面握手，标记 ready 并返回虚拟屏幕物理矩形 |
| `take_capture_origin` | `id` | `Option<PhysicalRect>` | P2：取出自建截图的选区物理矩形（飞入起点，取一次，8 条/30 s TTL） |
| `set_hit_rects` | `rects: Vec<PhysicalRect>` | — | 前端布局完成后上报（已换算为物理像素的 panel 内坐标） |
| `report_panel_ready` | `label` | — | 面板初始化完成 |
| `reveal_line` / `tuck_line` | `pinned: bool` | — | 程序化控制 |
| `check_updates` | — | `Result<Option<Version>>` | 手动检查 |
| `quit` | — | — | 还原并退出 |

**Events（Rust → 前端）**

| event | payload | 说明 |
|---|---|---|
| `state` | `AppState` | 全量状态（变更时广播） |
| `items` | `Item[]` | 条目列表（增量渲染用） |
| `reveal` / `tuck` | `{ reason }` | 驱动展开/收起动画 |
| `fly-animation` | `{ animations: FlyAnimation[] }` | P2：覆盖层播放/更新动画（≤2 个，kind = fly｜fall） |
| `fly-landed` | `{ id }` | P2：飞入结束，面板显示该卡片 |
| `fly-geometry` | `{ virtualRect, theme }` | P2：飞入/坠落覆盖层的虚拟屏幕几何与主题 |
| `panel-geometry` | `{ x, y, scale }` | P2：面板窗口物理几何变更（坠落/飞入起点换算用） |
| `capture-ready` | `{ monitorScale }` | 截图覆盖层就绪 |
| `toast` | `{ kind, message }` | 面板内提示 |
| `clipboard-copied` | `{ id }` | 显示 Copied 胶囊 |
| `trim-progress` | `{ id, progress }` | P3：0–1，保证 0.05/0.6/1.0 里程碑（WinRT 细粒度不可用） |
| `editor-state` | `{ annotateOpen, trimOpen }` | P3：编辑器/裁剪窗口开关；面板 busy 据此保持线展开 |
| `trim-state` | `{ open, id? }` | P3：裁剪窗口状态（busy 镜像） |
| `update-available` | `{ version, notes }` | 托盘提示 |

### 4.5 数据模型

```rust
struct Item {
    id: String,            // uuid v4
    path: PathBuf,         // 绝对路径
    thumb_path: Option<PathBuf>, // P2：缩略图缓存路径（IPC thumbPath，缺失时用原图）
    kind: ItemKind,        // Capture | SystemScreenshot | Clipboard | Recording | WatchedFolder
    hash: u64,             // blake3 截断
    width: u32, height: u32,
    created_at: i64,       // 文件创建时间（ms）
    added_at: i64,         // 上栏时间（ms）
    updated_at: i64,       // P3：内容修订时间（ms，旧 state 缺省 0；标注/裁剪后 +1 单调）
}

struct PublicState {          // IPC 快照（Rust 内部 AppState 还持有 Mutex/flags）
    items: Vec<Item>,          // 按 added_at 升序（最新在右）
    settings: Settings,
    version: String,
}
```
（`monitors` 由 `geometry::monitors()` 提供，Task 10 经 `get_monitors` command 暴露，不进入每帧广播的 `PublicState`。）

持久化：`%LOCALAPPDATA%\Hangline\state.json` 只存 `items`（路径/元数据）；启动时逐条 `path.exists()`，不存在则丢弃；**不补挂历史条目**（同 Tendedero baseline 策略）。`settings.json` 独立。

### 4.6 关键算法

**A. 点击穿透（hittest）**

- 面板可见时，Rust 每 33 ms 调 `GetCursorPos`；把光标换算到面板窗口客户区坐标。
- 前端在布局/滚动/换屏后把卡片矩形换算成物理像素，再上报 `set_hit_rects`。
- 命中任一矩形（外扩 4 dp）→ `set_ignore_cursor_events(false)`，否则 `true`。
- 状态只在变化时调用 API；拖拽中、右键菜单打开、标注窗口打开时强制 `false`。
- 面板隐藏时停止轮询（省电）。

**B. 弹簧动画（前端）**

- 积分器：半隐式欧拉，`v += (target - x) * stiffness * dt - v * damping * dt; x += v * dt`，`dt` 上限 32 ms。
- 微风：全局 gust 计时器每 7–16 s 随机触发，卡片各自 `delay 0–0.35 s`、振幅 `1.6–3.4°`、`stiffness 38 / damping 2.4`；入场 `16° / stiffness 46 / damping 2.6`；卡片间无耦合。
- 展开：`stiffness 224 / damping 25`（ω≈15 rad/s → response≈0.42 s，ζ=0.82）；收起 `easeIn 220 ms`。
- 飞入：0.65 s `easeInOutCubic` + `sin(πk)*30 dp` 上拱；尺寸/圆角/夹子透明度同步插值；结束后 0.16 s 交叉淡化交接给面板卡片。
- 坠落：0.55 s、下移 520 dp、`k³` 缓入。
- 飞入/坠落复用同一预热覆盖层窗口（≤2 并发）：覆盖层绘制动画，面板卡片先隐藏（`data-flying`），飞入结束后面板卡片带 2.2° 余摆显示；坠落时卡片立即从线上下线。P2 参考 Tendedero `CaptureFlight.swift`（`easeInOutCubic`、`sin(πk)*30`、120 Hz 采样）。
- 页面不可见（panel hidden）时暂停所有 rAF。

**C. 截图落点（飞入起点）**

自建截图：记录框选矩形（物理像素）→ 飞入起点 = 该矩形中心；
系统截图/剪贴板：无落点信息 → 起点 = 面板中央上方 160 dp（简化为从屏幕顶部中段落下），或屏幕中心（可配置）；V1 取「屏幕中心 → 挂点」。

**D. 剪贴板过滤链**

接受当且仅当：
1. 剪贴板序列号变化；
2. 含 `PNG`/`CF_DIBV5`/`CF_DIB` 之一；
3. 所有 item 均**不含**：`CF_UNICODETEXT`/`CF_TEXT`、`CF_HDROP`、`"ExcludeClipboardContentFromMonitorProcessing"`、`"Clipboard Viewer Ignore"`、`"CanIncludeInClipboardHistory"` 值为 0；
4. 解码后像素 ≤ 80 MP、字节 ≤ 64 MB；
5. 内容哈希未在最近 20 条内。

**补充裁决（2026-10-29，Task 9 实现后）**：带 `HTML Format`/文本的复制（**包括浏览器/Office 内的「复制图片」**）按规则 3/4 一律不收，这是隐私与降噪的取舍，不是缺陷；纯图片来源以 Win+Shift+S、微信/QQ 复制图片、截图工具落盘为主。仅提供 `CF_BITMAP` 的复制（如 .NET `Clipboard.SetImage`）暂不支持（需要 `GetDIBits` 转换），列入 V1.1 评估。

**E. 清理（prune）**

拖放结束 / watcher 变更 / 手动刷新时：对每条 `!exists(path)` 且非飞行中的卡片执行 `drop(id, quiet)`。**拖出的语义（2026-10-29 按 OLE 实现修订；2026-10-30 补 effect 驱动策略）**：对外同时提供 `DROPEFFECT_COPY | DROPEFFECT_MOVE`——拖进任意 app 时对方取 COPY，文件保留、卡片保留；拖进文件夹/桌面时 Explorer 执行 MOVE，拖进回收站时 shell 以删除语义完成（同样表现为原路径文件消失）；`after_drag(id, effect)` 在 600 ms 后先用 `should_prune(effect, exists)` 决策：`move` 且文件仍在时按 400 ms 间隔**重试检查最多 ~2 s**（容纳跨卷/慢速 shell 移动），仍存在则保留卡片并记日志（后续 watcher event 会再 prune）；`copy` 只做一次存在性检查；`effect=none`（取消/无目标）文件若消失也掉落（安全网）。**app 自身不移动/删除被拖出的文件**；`take_down(deleteFile=true)` 的 `IFileOperation` 回收只用于叉按钮。**删除文件只发生在**：`take_down(deleteFile=true)`、超出容量被挤掉的卡片（仅自有目录，且可配置为「保留文件」）。

**P4 实现裁决（2026-11-04）**：`keepOnLine` 的 25/50/100 现在是**硬状态上限**——最旧的卡片从线上移除（仅状态，不删除文件；`Fit` 不设上限）。规格中的“超出容量删除自有文件 / 保留文件开关”在 V1 不启用，避免静默删除；如需自动清理，后续版本作为显式设置加入。

### 4.7 剪贴板格式与写回

- 读取：优先 `PNG` 自定义格式（自描述、无损、alpha 正确），其次 `CF_DIBV5`、`CF_DIB`；用 `image` 解码为 RGBA。
- 写入（复制图片）：`CF_DIBV5` + `"PNG"` + `CF_HDROP`（文件路径）+ `CF_UNICODETEXT`（仅路径，可选关闭以免污染文本粘贴？V1 不写文本）。**注意**：写 `CF_HDROP` 后粘贴到聊天软件会变文件而非图片，因此顺序上把 `"PNG"` 放首位并同时提供 `CF_DIBV5`；实测微信/QQ/浏览器粘贴行为在 P1 验证，必要时提供「复制为文件」右键项。
- 视频复制：只写 `CF_HDROP`。

### 4.8 目录与文件

```
%LOCALAPPDATA%\Hangline\
├── settings.json
├── state.json
├── logs\hangline.log（滚动，5×2MB）
├── Screenshots\Hangline 2026-10-28 10.32.11.png   ← 自建截图
└── Thumbnails\（可选磁盘缓存，LRU 200MB）
```

监听目录（只读）：`%USERPROFILE%\Pictures\Screenshots`、`%USERPROFILE%\Videos\Captures`、用户额外目录（≤3）。

### 4.9 日志与诊断

- **P3 debug-only 测试钩子**：`HANG_RECORDINGS_DIR` 仅 debug 构建生效，用于隔离 E2E 重定向录屏监听目录；release 忽略。不得以覆盖 `USERPROFILE` 的方式隔离（tauri-plugin-log 的已知文件夹解析会 panic）。

- `tauri-plugin-log`：文件 + stdout；级别 info（release）/ debug（dev）。
- 关键事件结构化记录：捕获来源、去重命中、热键注册、拖放结果、裁剪结果、更新检查；**不记录图片内容与用户路径以外信息**。
- 托盘「诊断」不在 V1；日志可在「打开数据目录」中找到。

---

## 5. 平台适配决策

| 问题 | 决策 |
|---|---|
| 全屏应用 | `SHQueryUserNotificationState` + 前台窗口 `GetWindowRect` 覆盖整屏判定；延迟 0.8 s 复检 |
| 任务栏位置 | 用 `work_area` 而非 `screen_rect`，任务栏在顶部/侧边都不遮挡 |
| 开机自启 | `tauri-plugin-autostart`（HKCU Run，per-user，无 UAC） |
| 单实例 | `tauri-plugin-single-instance`，第二次启动 toggle 线 |
| 深色模式 | 跟随系统（`window.theme()` + 前端 `prefers-color-scheme`）；卡片/线各一套配色 |
| 透明窗口 | `transparent:true` + WebView2 透明背景；**P0 验证** Win10/11 + 多 DPI |
| 不抢焦点 | `focusable:false` + `WS_EX_NOACTIVATE`；标注/裁剪窗口例外 |
| 屏幕捕获权限 | GDI `BitBlt` 无需权限；受 DRM 保护窗口会黑屏（文档说明） |
| 通知 | V1 不用系统 Toast：成功用面板内 pill，非致命错误用托盘气泡，致命错误用原生消息框 |
| 崩溃恢复 | panic hook 写日志 + 原生消息框；重启后不恢复卡片（文件仍在） |

---

## 6. 打包、发布与更新

### 6.1 构建

- `pnpm tauri build` → `src-tauri/target/release/bundle/`：
  - `nsis/Hangline_<version>_x64-setup.exe`（per-user 安装，默认 `%LOCALAPPDATA%\Programs\Hangline`）
  - `Hangline_<version>_x64.msi`（可选，V1.1）
  - portable zip（`pnpm tauri build --no-bundle` 后手工 zip，附 `portable` 标记文件）
- WebView2：`webviewInstallMode: downloadBootstrapper`（在线安装）或 `embedBootstrapper`（+1.5 MB，离线可用）。**默认 embedBootstrapper**，保证离线机器可装。
- 图标：SVG/代码绘制 → `pnpm tauri icon` 生成全套；**不复制 Tendedero 图标**。
- 版本号：`package.json` 与 `src-tauri/tauri.conf.json` 同步（构建脚本 `scripts/version.mjs` 校验，不一致即失败）。

### 6.2 签名与发布

- 无商业代码签名证书：NSIS 未签名 → SmartScreen「未知发布者」。
- 缓解：README 说明 + 提供 SHA-256 + GitHub Release 附件 + 后续可选 SignPath/自签（自签仍需用户信任，收益低，V1 不做）。
- 发布流程（脚本化，`scripts/release.mjs`）：
  1. 更新版本号 → `pnpm install --frozen-lockfile` → `pnpm check`（lint + typecheck + 单测 + i18n 校验）
  2. `pnpm tauri build`
  3. 生成 `latest.json`（Tauri updater 格式）+ SHA-256
  4. `gh release create v<version>` 上传 setup/portable/latest.json（需 `gh` 登录）
  5. 更新 README 下载链接
- 自动更新：`tauri-plugin-updater` 指向 GitHub Releases；启动后（若 `autoCheckUpdates`）每日检查一次；**更新是唯一网络行为**，README 与首次启动引导明确声明「除此之外零网络」。

### 6.3 自动更新签名

- `pnpm tauri signer generate` 生成公私钥；私钥仅存本机/CI Secret；公钥写入 `tauri.conf.json`。
- 用户侧：下载新包 → 校验签名 → 静默安装（NSIS `/S`）→ 重启应用；失败回滚到旧版本目录。

---

## 7. 测试计划

### 7.1 自动化

| 层 | 工具 | 覆盖 |
|---|---|---|
| Rust 单测 | `cargo test` | `geometry`（DPI 换算/容量/命中）、`clipboard filter` 决策表、去重哈希、`settings` 迁移与默认值、`trim` 时间范围校验、`prune` 逻辑（临时目录） |
| 前端单测 | Vitest | 弹簧积分器（收敛/稳定）、手势状态机（单击/双击/长按/拖拽阈值）、i18n key 完整性、面板布局计算 |
| 静态检查 | `cargo clippy -D warnings`、`tsc --noEmit`、`eslint` | — |
| 构建冒烟 | `pnpm tauri build` 在 CI（GitHub Actions windows-latest） | 产物存在 + 版本号一致 + 安装包大小阈值 |

### 7.2 手动验收矩阵（每阶段执行）

| # | 场景 | 期望 |
|---|---|---|
| M1 | 单屏 100%，冷启动 | 托盘出现，3 s 内可用 |
| M2 | 双屏 100%+150%，两侧各截一张 | 截图在正确屏幕上线、无错位 |
| M3 | 任务栏在顶部/左侧 | 面板贴工作区顶，不被遮挡 |
| M4 | 全屏游戏/视频 | 线自动隐藏；退出全屏 1 s 内恢复 |
| M5 | Win+PrtScn 与 Win+Shift+S（自动保存开/关） | 均只上一条，无重复 |
| M6 | 复制纯图片（Win+Shift+S / 微信 / Excel 单图）与复制文本 | 纯图片上栏；文本被忽略；浏览器图片（带 HTML/文本）按规则过滤不收 |
| M7 | 密码管理器复制密码 | 不采集 |
| M8 | 单击/双击/长按/Force 快看替代 | 行为与 3.1 一致，单击不污染双击 |
| M9 | 拖到微信/Figma | 对方收到图片；卡片保留 |
| M10 | 拖到桌面文件夹 | 文件移动，卡片 0.6 s 内掉落 |
| M11 | 拖到回收站 | 文件进回收站，卡片掉落 |
| M12 | 标注保存 | 原文件被替换，缩略图立即更新，Ctrl+Z 可用 |
| M13 | 录屏录入 → 裁剪保存 | 时长正确、文件可播放 |
| M14 | 保留 100 张 + 滚轮 | 无卡顿；新截图在右 |
| M15 | 热键冲突（先启动 QQ） | 提示冲突、回滚旧键、可改键 |
| M16 | 退出/重启 | 截图设置无残留；条目按存在性恢复 |
| M17 | 卸载 | 用户数据提示保留/删除选项 |

### 7.3 P0 技术验证（先于编码；程序化判据必须通过，人工项在 P1 验收前闭合）

| 实验 | 判据 |
|---|---|
| E1 透明置顶面板 | 面板显示无黑底/白底；点击卡片区域可接收事件，其余区域穿透到桌面 |
| E2 点击穿透轮询 | 移动鼠标进出卡片，`ignore_cursor_events` 切换无可感延迟/闪烁 |
| E3 全局热键 | 注册/注销/改键/冲突回滚正常；不抢焦点 |
| E4 原生拖出 | 拖到资源管理器/微信得到文件副本；取消时飞回 |
| E5 自建截屏 | 多屏 + 150% 缩放框选正确；PNG 含 alpha 正确 |
| E6 托盘与单实例 | 二级启动不产生第二个进程 |

---

## 8. 阶段计划与验收（DoD）

| 阶段 | 交付 | DoD |
|---|---|---|
| **P0 验证** | 6 个技术实验 + 报告 | E1–E6 程序化判据全过；真实物理拖拽/解锁副屏/生产构建等人工项写入 P0 报告 §3.2 条件，在 P1 验收（Task 17）前闭合；任一程序化判据失败则改方案（兜底 Electron）。**GO 仅表示允许开始 P1 开发，不等于 P0 验收完成。** |
| **P1 核心闭环** | 三来源上栏、热键/自动展开收起、复制、拖出、删除、托盘、持久化 | M1/M5/M6/M8/M9/M10/M11/M16 通过；日常可替换现有截图习惯 |
| **P2 视觉灵魂** | 玻璃卡片/夹子/绳/摆动/飞入/坠落，深浅色 | 与 Tendedero 逐帧对比（录屏并排）；60 fps（面板 ≤ 12 卡） |
| **P3 标记与录屏** | 标注器、Game Bar 监听、裁剪 | M12/M13 通过；标注保存失败率 0（100 次压测） |
| **P4 多屏与设置** | 多屏/DPI、容量/滚动/尺寸、改键窗口、开机自启、声音 | M2/M3/M4/M14/M15 通过 |
| **P5 发布** | i18n（en/zh-Hans）、NSIS+portable、自动更新、README/press kit、CI | 干净 Win11 安装→使用→升级→卸载全链路通过；安装包 ≤ 15 MB |

每阶段结束：更新 `docs/` 下的验收记录 + 截图；P0/P3 额外写实验报告。

---

## 9. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| Tauri 透明窗口 + WebView2 在部分机器黑底/闪烁 | 核心形态失败 | P0 E1 验证；兜底 Electron（前端可复用）；`webview2-com` 直接设透明背景 |
| 点击穿透与拖拽冲突 | 拖拽中卡片消失/穿透 | 拖拽/菜单/长按期间强制 interactive；状态机单点管理 |
| `start_dragging` 在 Tauri 2 的 API 细节（拖拽图像/多文件） | 拖出体验差 | P0 E4 验证；失败回退自实现 `IDropSource`+`IDataObject` |
| WinRT MediaComposition 编解码兼容性 | 裁剪失败 | P3 验证；失败回退默认播放器并提示；V1.1 评估 MF SinkWriter |
| 无代码签名 | SmartScreen 警告、企业环境拦截 | 文档 + SHA-256 + 后续评估 SignPath/Azure Trusted Signing |
| 自动更新被安全软件拦截 | 更新失败 | 提示手动下载；更新包与主包走同一 GitHub Release |
| 多 DPI 坐标错位 | 面板/命中断层 | geometry 纯函数 + 单测；跨屏实测矩阵 |
| WebView2 内存占用高（相对原版） | 违背轻量预期 | 面板隐藏时销毁 fly/capture 窗口、暂停 rAF、缩略图限 320 px |
| 与 QQ/微信/Snipaste 热键冲突 | 用户无法截图 | 默认键位避开常见组合；冲突检测；一键改键 |
| 品牌/许可合规 | 法务风险 | 全新名称/图标；MIT 声明与 NOTICE；README 注明灵感来源 |

> **2026-11-09 口径补充（内存）**：上表「WebView2 内存占用高（相对原版）」一项的验收口径已修订为**双口径**（主进程 ≤150 MB 必须 / 进程树私有内存 ≤350 MB 暂定），并新增「新增可视面按 +70–135 MB/窗口 估算、立项前必须测量」的预算规则。原行保留不改；依据与实测数据见 §1.3「**Amendment 2026-11-09 — 内存口径修订**」及 [W0 真机验收记录](../../acceptance/W0-real-machine-2026-11-09.md)。

> **Amendment 2026-11-10 — 按需窗口与检索**
>
> 适用：Wave 2（目标 v1.3）新增的**搜索窗**、备注/标签与 OCR 文本索引。本 Amendment 只补口径，**不改变** 2026-11-09 的双口径（主进程 ≤150 MB 必须 / 进程树私有 ≤350 MB 暂定）。
>
> **1）搜索窗是「按需生命周期」，不是常驻可视面**
>
> - 搜索窗**打开时创建、关闭时销毁**（不隐藏复用）；只在**打开期间**计入内存峰值，新增窗口仍按 2026-11-09 的 **+70–135 MB/窗口** 估算。
> - 新增验收指标（两者都要过）：
>   1. **关闭搜索窗后，进程树私有内存回落到打开前的 ±30 MB 以内**；
>   2. **常驻（未打开搜索窗）时，进程树私有内存不得高于 Wave 1 基线 293.8 MB**。
> - 面板窗口是 `focusable:false`（「绝不抢焦点」红线），**不承担搜索输入**：搜索框、备注/标签编辑全部放在**可聚焦的搜索窗**内；搜索窗属「编辑器类窗口」例外，与标注/裁剪窗口同类管理。
>
> **2）检索的数据边界（纯文本）**
>
> - 备注：纯文本，单条 ≤ **2000 字符**；标签：每条卡片 ≤ **20 个**、单标签 ≤ **32 字符**，写入时去首尾空白、去重、忽略空值。
> - **不引入富文本**：不做 Markdown/HTML 渲染，备注、标签、文件名、路径一律按纯文本处理（渲染层不得插入 HTML）。
>
> **3）OCR 文本索引（W2.5）的设计约束**
>
> - 只使用系统 **`Windows.Media.Ocr`**：**0 字节安装增量、离线、不联网**；不引入模型下载、不引入第三方推理运行时。
> - 索引是**本地文件、可删除重建**；只索引截图文本用于本地搜索，**不进入任何联网请求**。
> - 上线门槛：中文**关键词召回率 ≥ 80%**（先以此为 PoC 门槛）；**不达标就不上线**，只保留人工备注/标签检索。
>
> **4）网络承诺不变**
>
> - 「**唯一网络请求 = 更新检查**」不因检索能力改变：OCR、索引、搜索全部在本地完成。

---

## 10. 开放问题（不阻塞 V1，记录待决）

1. 是否需要「多屏各一条线」（当前单面板跟随指针；多屏用户可能希望每屏常驻）。
2. 自建截图是否支持窗口捕获/全屏直接截（V1 只做区域框选；全屏可用 `Ctrl+Alt+Shift+S` 扩展）。
3. 是否移植 Tendedero 的 11 语言全部（V1.1 决定）。
4. 是否提供「复制为文件」与「复制为图片」两个菜单项（P1 实测微信/QQ 粘贴行为后定）。
5. 是否上 Microsoft Store（需 MSIX + 签名，V1 不做）。

---

## 11. 许可、归属与文档

- 本项目代码：**MIT**（`LICENSE`）。
- 第三方与灵感：`THIRD_PARTY_NOTICES.md` 注明 Tendedero（MIT，© 2026 Alejandro Buján）为形态灵感来源；若移植其翻译文本，逐语言注明译者（来自其 PR/commit 记录）。
- 图标与 README 插图：全部原创（SVG + `pnpm tauri icon` / Node 脚本渲染），不复用 Tendedero 美术。
- 文档：`README.md`（英）+ `README.zh-Hans.md`；`docs/` 放验收记录与实验报告；`docs/superpowers/specs/` 放本设计文档。

---

## 附录 A：默认值速查

- 截图热键 `Ctrl+Alt+S`；显示线热键 `Ctrl+Alt+T`
- 尺寸档 medium：卡片 150×104 dp，面板高 210 dp
- 容量：`max(3, min(12, floor((panelWidth-200)/174)))`（medium 下 174 dp 间距）
- 展开 0.42 s / 收起 0.22 s；顶边停留展开延迟 0.5 s（可选路径，默认关）；离开收起 0.5 s；新截图 peek 2.5 s；空线 0.7 s 后隐藏
- 长按 0.45 s；拖拽阈值 4 dp；删除热区 26 dp；单击延迟 = 系统双击间隔
- 列容量上限：默认「放得下多少」，可选 25/50/100

## 附录 B：依赖与插件清单

**Tauri 插件**：`single-instance`、`global-shortcut`、`autostart`、`updater`、`log`。
**Rust crates**：`serde`, `serde_json`, `image`, `notify`, `blake3`, `windows`, `thiserror`, `anyhow`, `time`, `base64`, `parking_lot`（可选）、`rayon`（可选，缩略图并行）。
**前端**：`react`, `react-dom`, `zustand`, `clsx`；开发依赖 `typescript`, `vite`, `@vitejs/plugin-react`, `vitest`, `eslint`, `prettier`。
**构建/发布**：`@tauri-apps/cli`, `scripts/check-i18n.mjs`, `scripts/version.mjs`, `scripts/release.mjs`，GitHub Actions（windows-latest）。
