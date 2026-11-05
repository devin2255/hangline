# P2 参考测量：Tendedero 视觉/运动参数与官方演示 GIF 帧分析

- **任务**：P2-1「Motion tokens and reference measurements」
- **分支 / 基线**：`feat/core`，P2-1 提交 `d17d419`（父提交 `244f746`；计划随后于 `01c3117` 按本任务实测更正）
- **日期**：2026-11-02
- **结论**：P2 的运动/视觉 token 以本地只读的 Tendedero Swift 源码为唯一数值来源（下表逐条列出文件与行号）；官方演示 GIF（130 帧 × 40 ms = 5.2 s，960×340）用本仓库的 `scripts/p2-reference/analyze_demo.py` 做了帧级测量，得到「展开 1.32 s 出现、1.44 s 到达静止高度、1.52 s 过冲 29.3 px、2.00 s 收敛；收起 4.80 s 开始、4.96 s 完全干净；3 张卡几何与采样帧 58 倾角 -1.1°/+1.4°/-3.5°（2000–4800 ms 稳态窗口中位数 -1.72°/+1.05°/-3.08°）；Copied 胶囊 3.20–3.84 s」等可复现数字。演示 GIF 是宣传动画，其过冲与倾角幅度与源码 token 不完全一致，只用于时序/几何/运动轨迹交叉验证，不用于校准 spring 数值。

> **证据口径**：【源码】= 本地只读参考副本 `research/ui/src/*.swift` 的实际数值；【GIF 实测】= 本报告第 4 节由 `analyze_demo.py` 从官方 GIF 重算的数字（同一输入字节级可复现）。未测过的项不写成已测。

## 1. 目的与范围

为 P2「视觉与动效」提供可引用的参考数据，并验证 demo 的关键时序：

1. 把 Tendedero 源码里的 spring / 曲线 / 几何数字固化成 `src/lib/motion.ts` 的 token（供 P2-2 起的卡片运动状态机、面板编排、飞入/坠落复用）；
2. 测量官方演示 GIF 的展开/收起时序、卡片几何、倾角轨迹和 Copied 胶囊出现区间，作为 P2 实现后的对照基准；
3. 记录参考资产、版权边界和本机无法复现的部分（无 macOS 录制环境）。

不在本任务：任何应用运行时改动（`src/lib/motion.ts` 之外的 `src/` 文件）、飞入/坠落窗口实现、性能测试；这些属于 P2-2 之后的子任务。

## 2. 参考来源

| 来源 | 位置 | 用途 |
| --- | --- | --- |
| Tendedero 源码（只读副本） | `research/ui/src/{Line.swift,LineView.swift,PeggedView.swift,CaptureFlight.swift}`（工作区根目录下的本地副本，不属于本仓库） | 数值唯一来源：spring 参数、时长、几何、颜色 |
| 官方演示 GIF（浅色） | `.superpowers/p2-reference/demo-light.gif`（960×340，130 帧 × 40 ms = 5.2 s） | 帧级时序/几何/倾角测量 |
| 官方宣传图 | `.superpowers/p2-reference/{hero,bento}-{light,dark}.png`、`icon.png`、`demo-dark.gif` | 仅本地留存，供后续 P2-8 材质/配色人工比对（本任务未做材质测量） |
| 参考工具 | `scripts/p2-reference/fetch.mjs`、`scripts/p2-reference/analyze_demo.py` | 下载资产、GIF 测量（开发用，不随应用分发） |

## 3. 源码参数表（参考数据）

> 行号对应本地参考副本 `research/ui/src/`。这些数值是 `src/lib/motion.ts` 中 `SPRINGS` / `FLY` / `FALL` / `TILT` / `GUST` / `EDGE_FADE` / `CARD` 的直接依据。

| 项目 | 数值 | 来源 |
| --- | --- | --- |
| 卡片倾斜 | 随机 ±2.5°，锚定在条目上，夹子+卡片整体旋转 | 【源码】`Line.swift:14`、`PeggedView.swift:23` |
| 照片框 | 最大宽 `cardWidth-14`、最大高 `photoMaxHeight`，保持宽高比；卡片 = 照片 + 8 dp | 【源码】`PeggedView.swift:38-49` |
| 卡片圆角 / 内衬 | 16 dp / 4 dp；照片圆角 12 | 【源码】`PeggedView.swift:158-160` |
| 夹子 | 9×26 dp，圆角 3.5，水平 4 段金属渐变，线槽 5×1.4 位于顶部 8.5，阴影 black .30/r2/y1.5 | 【源码】`PeggedView.swift:184-213` |
| 夹子/绳/卡片关系 | 夹子顶 = ropeY − 9.5 dp；卡片顶 = 夹子顶 + 14 dp（与夹子重叠 12 dp）；卡片视觉宽 = 照片宽 + 8 | 【源码】`LineView.swift:28-29`、`PeggedView.swift:18-26` |
| 悬停 / 按压 | 悬停 scale 1.035、阴影 `.26/r14/y8`；按压 0.95、450 ms ease-in-out；常态阴影 `.18/r10/y5` | 【源码】`PeggedView.swift:82-85` |
| 入场 | 卡片从 −46 dp 落下，摆角 16° → 0；y 用 spring(response .42 / ζ .72)；旋转用 `interpolatingSpring(46, 2.6)` | 【源码】`PeggedView.swift:127-136` |
| 微风 / 阵风 | 轻摆 3°（复制）、2.2°（落位）；阵风振幅 1.6–3.4°、延迟 0–350 ms、回正 spring(38, 2.4)；每 7–16 s 一次 | 【源码】`PeggedView.swift:140-155`、`Line.swift:304-310` |
| 展开 / 收起 | 展开 `spring(response .42, ζ .82)` = k≈224 / c≈25；收起 `easeIn 220 ms`；隐藏偏移 `-(panelHeight+12)` | 【源码】`LineView.swift:86-88` |
| 条目重排 | `spring(response .55, ζ .78)` ≈ k≈130.5 / c≈17.8 | 【源码】`LineView.swift:82` |
| 飞入 | 0.65 s `easeInOutCubic`，弧线 `sin(πk)*30`；尺寸/圆角/内衬/夹子透明度插值（玻璃与镀铬从 k=0.35 淡入）；倾角插值；结束后 0.16 s 交叉淡化交接；120 Hz 采样 | 【源码】`CaptureFlight.swift:31-33,190-223` |
| 坠落 | 0.55 s、下移 520 dp、`k³`，附加倾角 `tilt*7+20`，透明度 `1-e` | 【源码】`CaptureFlight.swift:169-179` |
| 边缘淡出 | 卡片透明度 `min(1, max(0, min(x, width−x)/90))` | 【源码】`LineView.swift:78` |
| 空线提示 | 胶囊、12 pt rounded medium 文本，位于绳中点下方 +34 | 【源码】`LineView.swift:65-69,96-105` |

## 4. 官方演示 GIF 帧分析（实测）

数据文件：`.superpowers/p2-reference/demo-measurements.json`（由 `analyze_demo.py` 生成，仅本地、不入库；同一 GIF 连续运行输出字节一致）。其中 `cards[].boxPx` / `cards[].centerPx` 已是 GIF 像素；`referenceCenterYPx` 与 `tiltTracks` 内的坐标是分析区（掩码）坐标，换回 GIF 像素需 x+8、y+42。

### 4.1 基本信息

| 项 | 实测值 |
| --- | --- |
| 帧数 | 130 |
| 尺寸 | 960×340 GIF 像素 |
| 帧间隔 | 40 ms（全部 130 帧一致） |
| 总时长 | 5200 ms |
| 分析区域 | x 8–952、y 42–316（避开假菜单栏和录屏圆角白边） |
| 卡片数 | 3（便签卡 / 照片卡 / 图表卡） |

### 4.2 展开 / 收起时序

| 阶段 | 帧号 | 时间 | 判据 |
| --- | --- | --- | --- |
| 展开开始（卡片像素首次出现） | 33 | 1320 ms | 分析区内容面积 0 → 15532 px² |
| 到达静止高度（首次越过） | 36 | 1440 ms | 三张卡质心全部到达/越过静止 y |
| 展开过冲峰值 | 38 | 1520 ms | 质心中位偏移 +29.3 px |
| 展开收敛 | 50 | 2000 ms | 质心与静止 y 差 ≤2 px 并保持 10 帧 |
| 收起开始 | 120 | 4800 ms | 三张卡质心同时上移 >3 px |
| 收起末帧（仍有卡片像素） | 123 | 4920 ms | 内容面积 342 px²（前一帧 22413） |
| 画面完全干净 | 124 | 4960 ms | 内容面积 119 px²（只剩游标） |

计划原始文本（`244f746`）的「reveal 在 1.2–1.6 s 完全落下」与实测一致（1.44 s 到达静止高度）；原始文本的「tuck complete by 3.64 s」与实测不符（实测 4.80 s 才开始收起）。计划已在 `01c3117` 按本任务实测更正，差异记录见 §4.6。

### 4.3 卡片几何（采样帧 58 = 2320 ms，此时已收敛且未悬停）

| 卡片 | 包围盒 `[x0,y0,x1,y1]`（GIF px） | 宽×高（px） | 中心（px） | 采样帧 58 倾角 | 内容像素 |
| --- | --- | --- | --- | --- | --- |
| 0 便签卡 | `[146, 58, 350, 196]` | 205×139 | (248.0, 127.0) | **−1.10°** | 18806 |
| 1 照片卡 | `[364, 62, 593, 213]` | 230×152 | (478.5, 137.5) | **+1.39°** | 32511 |
| 2 图表卡 | `[612, 58, 801, 186]` | 190×129 | (706.5, 122.0) | **−3.49°** | 18515 |

- 相邻卡片净间距：**13 px / 18 px**（中心间距 231 px / 228 px）。
- 三张卡质心的静止 y（= JSON `referenceCenterYPx` + 42）：135.2 / 139.3 / 119.3 px。
- 表中倾角是采样帧 58（2320 ms）的单帧值；若取 2000–4800 ms 稳态窗口的倾角中位数，得到 **−1.72° / +1.05° / −3.08°**，与采样帧值分别相差 0.62° / 0.34° / 0.42°（最大 0.62°），两种口径一致。
- 三张卡宽度不同（205/230/190 px），与源码「卡片尺寸 = 照片尺寸 + 8 dp 内衬」一致：照片宽高比不同 → 卡片宽不同。
- GIF 为宣传素材，未标注录屏缩放比例，因此几何只按 GIF 像素记录，不换算 dp（避免把未知缩放当成测量结论）。

### 4.4 入场摆动与稳态摇摆（倾角轨迹）

倾角由卡片像素的 PCA 主轴角得到，屏幕坐标顺时针为正，与 SwiftUI `.rotationEffect` 同号。

| 卡片 | 采样帧 58 倾角 | 入场摆动范围（1440–2000 ms） | 稳态摇摆范围（2000–4800 ms） |
| --- | --- | --- | --- |
| 0 便签卡 | −1.10° | −7.86° … +2.26° | −3.01° … −0.55° |
| 1 照片卡 | +1.39° | −1.45° … +8.00° | +0.64° … +2.06° |
| 2 图表卡 | −3.49° | −10.17° … −0.48° | −5.55° … −0.68° |

- 入场阶段可见明显的**欠阻尼摆动**：卡片出现后的头几帧角偏移达 6–10°，随后反向、衰减，约 0.7 s 内基本收敛（此后只剩轻微余摆）——定性符合源码「16° 摆角 + `interpolatingSpring(46, 2.6)`（ζ≈0.19）」。但 GIF 是 25 fps 采样，且三张卡的摆动方向/幅度各不相同（宣传动画按卡片各自编排），因此**不能用它反推 16° 初值或精确刚度**；token 仍取源码值。
- 收敛后仍有轻微摇摆：相对采样帧 58 倾角约 ±2°，图表卡最大约 2.8°（大致落在源码阵风振幅 1.6–3.4° 的量级内），可视为演示里的 breeze 余摆。
- 实测采样帧 58 倾角（−3.49° / −1.10° / +1.39°）中，图表卡略超源码随机范围 ±2.5°，说明该 GIF 是近似还原的宣传动画，而非逐帧等价录屏；这与 §4.2 的过冲偏大互相印证。

### 4.5 Copied 胶囊

| 项 | 实测值 |
| --- | --- |
| 出现区间 | frame 80–96 |
| 起止时间 | **3200 ms – 3840 ms** |
| 持续 | 680 ms |
| 位置 | 照片卡下方居中（玻璃胶囊 + 对勾 + “Copied” 文本） |

注意：Tendedero 源码/应用里 Copied 胶囊实际保留 **1.2 s**（`Line.swift:166-167` 的 `asyncAfter(deadline: .now() + 1.2)`；本仓库 `src/lib/pillTimer.ts:9` 的 `COPIED_PILL_MS = 1200` 与之对齐）。宣传 GIF 把它缩短到 680 ms 属于演示节奏，**Hangline 的胶囊寿命必须保持 1.2 s，不得采用 GIF 的 680 ms**。

计划文本写的「Copied pill 在 2.4 s / 3.2 s」中，2.4 s 附近目视为**指针进入照片卡、卡片开始悬停放大**（人工逐帧观察，未纳入脚本测量，故此处不引用像素差值），胶囊只在 3.2 s 后出现一次；以实测为准。

### 4.6 与计划原始 demo reference 的差异（计划已按实测更正）

下表记录更正前的计划文本（`244f746`）与实测的差异；计划已在 `01c3117` 按本任务结果更新为实测值，此处保留差异以便追溯。

| 项 | 计划原始文本（`244f746`） | GIF 实测 | 处理 |
| --- | --- | --- | --- |
| 展开完成 | 1.2–1.6 s | 1.44 s 到达静止高度（1.32 s 出现、2.00 s 收敛） | 一致，保留 |
| 收起完成 | by 3.64 s | 4.80 s 开始、4.92 s 末帧、4.96 s 干净 | 以实测为准；推测原文本为早期测量/笔误，偏差约 1.2 s；计划已于 `01c3117` 更正 |
| Copied 胶囊 | 2.4 s / 3.2 s | 3.20–3.84 s 单次 | 以实测为准；2.4 s 实为悬停开始；计划已于 `01c3117` 更正 |
| 整线下落过冲 | 未提 | +29.3 px（约卡高 1/5） | 演示动画夸张；源码 reveal spring(ζ .82) 过冲仅约 1%，token 取源码值 |
| 倾角幅度 | ±2.5°（源码随机范围） | 采样帧 58：−3.49°/+1.39°/−1.10°；稳态中位数：−3.08°/+1.05°/−1.72° | 演示近似；token 取源码 ±2.5°（计划 `01c3117` 引用的是采样帧 58 值） |

## 5. 与 `src/lib/motion.ts` token 的对应

| token | 数值 | 参考依据 |
| --- | --- | --- |
| `SPRINGS.reveal` | k=224 / c=25 | 源码 reveal spring(response .42, ζ .82)（换算 k≈223.8 / c≈24.5） |
| `SPRINGS.item` | `springFromResponse(.55, .78)` ≈ k=130.5 / c=17.8 | 条目重排 |
| `SPRINGS.arriveY` | `springFromResponse(.42, .72)` | 入场 y |
| `SPRINGS.hover` | `springFromResponse(.3, .6)` | 悬停/按压缩放 |
| `SPRINGS.arriveSwing` | k=46 / c=2.6 | 16° → 0 的入场摆角 |
| `SPRINGS.breeze` | k=38 / c=2.4 | 阵风回正 |
| `FLY` | 650 ms / 弧 30 dp / chromeStart .35 / handoff 160 ms / fade 160 ms | 飞入 |
| `FALL` | 550 ms / 520 dp | 坠落 |
| `TILT` | ±2.5° / 入场 16° / 落位 2.2° / 复制 3° | 倾斜与轻摆 |
| `GUST` | 7–16 s / 延迟 ≤350 ms / 振幅 1.6–3.4° | 阵风 |
| `EDGE_FADE` | 90 dp | 边缘淡出 |
| `CARD` | 入场偏移 −46 dp | 入场起始位置 |

## 6. 方法（如何得到上面的实测数字）

1. **读取帧**：Pillow 逐帧 `seek` 后 `convert("RGB").copy()`（GIF 为 130 帧、每帧 40 ms；复制避免下一帧 `seek` 覆盖缓冲）。
2. **内容掩码**：在固定分析区（x 8–952、y 42–316）内，按固定颜色规则取“卡片内容”像素——近白（min(r,g,b) > 246）、暗色（max(r,g,b) < 115）或高饱和（max−min > 70）。该规则同时会命中白色描边黑色填充的鼠标游标，因此：
3. **卡片列检测**：逐列统计内容像素，连续列计数 ≥8 且区间宽 ≥40 px 才算卡片（游标仅约 15 列，不形成卡片 run）。
4. **固定 x-band 跟踪**：取稳定帧的 3 个卡片 run，以相邻 run 间隙中点为边界固定三条带。这样即使展开/收起时整线滑动、相邻卡片因 25 fps 运动模糊并成一个 run，也能逐卡跟踪。
5. **几何**：带内每列的首/末内容像素 y 取**中位数**（游标最多影响十几列，中位数稳健），x 范围取内容列；得到卡片包围盒、宽高、中心与净间距。
6. **倾角**：带内内容像素求二阶中心矩，PCA 主轴角即卡片旋转角（顺时针为正）。
7. **时序**：用各卡质心 y（旋转时质心基本不动）判断——首次出现、首次到达/越过静止 y、过冲峰值、±2 px 保持 10 帧视作收敛、收起开始（三卡同时上移 >3 px）与末帧。
8. **胶囊**：照片卡下方固定区域内暗色文字像素 ≥50 视作胶囊可见。
9. **确定性**：无随机数、无时间戳、阈值全部为常量；连续两次运行、以及宿主 Python 3.13 与 DSH 自带 Python 3.12 之间，输出 JSON 的 SHA-256 相同（本机验证一致）。

## 7. 局限

1. **没有 macOS 录制环境**：本机是 Windows 11，无法复现 Tendedero 的 macOS 屏幕录制/Retina 采样，因此对比是**帧时序 + 几何 + 运动轨迹**（数值）层面的，不是像素级视觉回归或逐帧 diff；材质（毛玻璃、金属、阴影）只能靠宣传图人工目测，不构成验收证据。
2. **GIF 只有 25 fps**：所有时间量的分辨率是 40 ms；倾角在展开首帧/收起末帧存在截断误差，正文中的摆动极值应视为 ±0.5° / ±40 ms 量级。
3. **演示 GIF 是宣传动画而非等价录屏**：其整线下落过冲（29.3 px）与采样帧 58 倾角（−3.49°）都大于源码 token 的理论值，说明它对运动做了夸张/近似；因此它只用于「定性 + 时序」交叉验证，不用于校准 spring。
4. **GIF 量化**：256 色调色板 + 抗锯齿会让颜色规则在卡片边缘产生 1–2 px 抖动；本报告用中位数/固定 run 宽度规避，但包围盒仍可能有 ±2 px 误差。
5. **缩放比例未知**：宣传素材未标注录屏/导出缩放，故几何只按 GIF 像素给出，不换算 dp。

## 8. 版权与资产边界

- Tendedero 以 **MIT + 名称/图标限制**发布：可以参考其源码测量数值，但不得使用其名称、图标作为 Hangline 的品牌元素，也不得把其宣传资产打包进应用或仓库。
- 本任务只**下载/读取**官方仓库 `docs/images/` 下的宣传资产到本地 `.superpowers/p2-reference/`；该目录由 `.git/info/exclude` 忽略（`git status --ignored` 显示 `!! .superpowers/`），**不提交、不随应用分发**。
- 提交进仓库的只有：`scripts/p2-reference/fetch.mjs`（下载器）、`scripts/p2-reference/analyze_demo.py`（测量脚本）、`docs/experiments/P2-reference.md`（本报告，仅含数值与文本）和 `src/lib/motion.ts` / `src/lib/motion.test.ts`（token 与测试）。
- 图片/演示中的界面元素、文案（如 “Copied”）属于通用 UI 词汇，未复制其图标或美术资源。

## 9. 复现

环境：Windows 11 x64；Node **22.17.1**（脚本要求 ≥ 22 的内置 `fetch`）；Python **3.13.5** + Pillow **12.3.0**（脚本只用 Pillow）。测量脚本在宿主 Python 3.13.5 与 DSH 自带 Python 3.12.14 上各运行一次，输出的 `demo-measurements.json` SHA-256 相同（`389C91E709EBC332…`）。

```powershell
# 1) 下载官方参考资产（已存在则跳过；开发用，永不进 CI）
node scripts/p2-reference/fetch.mjs

# 2) 生成 GIF 测量结果（确定性；输入不是 960×340 / 130 帧时会以清晰错误退出）
python scripts/p2-reference/analyze_demo.py

# 3) 产物（git-ignored）
#    .superpowers/p2-reference/demo-measurements.json
# 4) token 单测
pnpm exec vitest run src/lib/motion.test.ts
```

`demo-measurements.json` 关键字段：`frames` / `size` / `durationsMs` / `totalMs` / `revealFrame` / `revealDownFrame` / `revealOvershootFrame|Px` / `revealSettledFrame` / `tuckStartFrame` / `tuckEndFrame` / `tuckHiddenMs` / `cards[]`（包围盒、宽高、中心、倾角、像素数）/ `cardGapPx` / `copiedPill` / `busyRatios` / `tiltSummary` / `tiltTracks`（每帧倾角与质心）。

## 10. 产物

| 文件 | 说明 |
| --- | --- |
| `src/lib/motion.ts` | P2 motion tokens 与曲线（spring/飞入/坠落/倾斜/阵风/边缘淡出/卡片偏移） |
| `src/lib/motion.test.ts` | token 数值与曲线单测（8 条） |
| `scripts/p2-reference/fetch.mjs` | 官方参考资产下载器（Node ≥22 内置 fetch，无依赖，跳过已存在文件） |
| `scripts/p2-reference/analyze_demo.py` | 演示 GIF 测量脚本（Pillow、确定性 JSON） |
| `docs/experiments/P2-reference.md` | 本报告 |
