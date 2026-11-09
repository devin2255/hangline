# Wave 0 执行计划与状态（还债与信任）

> **决策依据**：产品负责人已确认「**Wave 0 先行**」——先清偿 1.1.0 的交付债与信任债，再进 Wave 1 的卡片操作面。
> **日期**：2026-11-09
> **目标版本**：**v1.1.1 已切版**（版本号/CHANGELOG/文档已更新；本地提交与构建验证完成，`push`/`tag` 由维护者决定）
> **关联文档**：[SnowAir 借鉴评估与迭代建议](2026-11-09-snowair-benchmark-iteration-plan.md) · [人工验收剧本](2026-11-09-wave0-manual-acceptance-runbook.md)

---

## 0. 一句话状态

**Wave 0 已收口：全量验证通过（前端 210 测试；Rust 库 369 + 二进制 31，0 失败 / 10 ignored——此前的 7 个 autostart 失败确认为沙箱限制，全权限下全绿）。自动化真机验收 15 项 PASS，维护者于 2026-11-09 完成剩余真人验收。代码签名按「等 4–6 周再申请」定档，材料已就绪。版本已切到 v1.1.1，本地提交与发布构建验证完成，`push`/`tag` 待维护者决定。**

| Wave 0 项 | 状态 | 说明 |
|---|---|---|
| W0.0 质量门修复 | ✅ 完成 | `.research/` 进 `.gitignore` 与 eslint ignore，`pnpm check` 的 lint 侧恢复绿 |
| W0.1 IPC 表面收敛 | ✅ 完成 | event 24→21、command 41→40；删除 P0 脚手架 `drag_demo_file`；保留 `hotkey-error` 并注明用途 |
| W0.2 真机验收 | ✅ **完成** | 自动化 15 项 PASS（[记录](../acceptance/W0-real-machine-2026-11-09.md)）+ 维护者 2026-11-09 完成真人物理输入验收（拖拽/托盘/真实应用/Game Bar/声音），见 P1 附录 C |
| W0.3 内存/环境基线 | ✅ **已实测（并发现目标口径问题）** | 主进程 RSS 39.4 MB / 私有 12.7 MB；**含 7 个 WebView2 子进程的进程树：私有 293.8 MB、RSS 571 MB** → 「≤150 MB」只在只算主进程时成立 |
| W0.4 配置三层兜底 | ✅ 完成 | `.bak` 轮转与恢复、读失败拒写；新增 8 个单测（settings 4 + store 4） |
| W0.5 `--selftest` | ✅ 完成 | 10 项现场诊断、退出码=FAIL 数、stdout+日志双通道、JSON 输出；31 个单测 |
| W0.6 体积门 8.5 MB | ✅ 完成 | 门禁 15MB→8.5MB + 四项体积报告；CI/脚本/文档旧 15MB 口径已统一 |
| W0.7 代码签名 | 🟡 **方案已定：等 4–6 周再申请** | 材料已齐（策略页/README/申请包/artifact 草稿/Actions 模板）；维护者待确认 MFA，并向 SignPath 确认 WebView2 bootstrapper 条款 |

---

## 1. 范围（做什么 / 不做什么）

**做**：质量门、IPC 表面、配置容错、现场诊断、体积门、人工验收剧本、签名决策备忘。

**不做（仍属 Wave 1/后续，本轮一行没碰）**：卡片右键菜单、hover 动作胶囊、标注新工具、搜索/标签、OCR、贴图 Pin、命令面板、任何新窗口、任何联网能力、任何 UI 改版。

**也不做**：版本号 bump、README 功能描述改写、发布产物重打。原因：Wave 0 的验收闸没关，此时改版本会让"1.1.1"名不副实。

---

## 2. 工程变更清单（文件级）

| 文件 | 变更 | 影响面 |
|---|---|---|
| `.gitignore` / `eslint.config.js` | 忽略只读调研副本 `.research/` | 质量门恢复可一键通过 |
| `src-tauri/src/lib.rs` | 删除 `clipboard-copied` / `second-instance` 发射与 `drag_demo_file` 注册 | IPC event 24→21、command 41→40 |
| `src-tauri/src/fullscreen.rs` | 删除 `fullscreen-state` 事件与 `FullscreenEvent`/`Serialize` 派生，改由日志 + `reveal::set_fullscreen_suppressed` 单一通道 | 行为不变（该事件原本无监听者） |
| `src-tauri/src/hotkeys.rs` | 保留 `hotkey-error`，注释说明保留给 Wave 1 设置可解释化 | 无行为变化 |
| `src-tauri/src/dragout.rs` | 删除 P0 演示文件脚手架（`drag_demo_file`/`ensure_demo_file`/`DEMO_*`） | 无前端调用，删除后 0 dead_code |
| `src/lib/types.ts` | 删除 `ClipboardCopiedEvent` 类型 | 与 Rust 侧一致 |
| `src-tauri/src/settings.rs` | 三层持久化：`.bak` 轮转 → corrupt 恢复 → 默认值；目标文件存在但不可读时**拒写** | 用户设置不再可能被"洗掉"；恢复时保留 `.corrupt` 诊断样本 |
| `src-tauri/src/store.rs` | 同上（`state.json`） | 线上条目列表具备同样的三层兜底 |
| `src-tauri/src/main.rs` | 仅新增 `--selftest` 参数分派；无 flag 时与原来逐行一致 | 正常运行路径零影响 |
| `src-tauri/src/selftest.rs` | 新增（二进制 crate，`#[path]` 引入）：10 项探针 + 31 单测 | 新增文件，不改 lib/Cargo.toml |
| `scripts/release/build-release.mjs` | `SETUP_LIMIT_BYTES` 15,000,000→8,500,000；新增 Size budget 四项报告（exe/setup/WebView2/portable + 余量百分比） | 超限仍 fail-fast；未新增构建步骤 |
| `docs/RELEASE.md` / `scripts/release/README.md` / `verify-installer.ps1` / `.github/workflows/{ci,release}.yml` | 体积口径统一到 8.5MB | 避免 CI 报错文案与门禁自相矛盾 |

> **历史文档不追改**：`CHANGELOG.md` 的 1.1.0 条目、`docs/acceptance/P5-core.md`、旧 spec/plan 里的「15 MB」是当时事实，保持原样。

---

## 3. 验证证据（本机实跑，2026-11-09）

| 门 | 命令 | 结果 |
|---|---|---|
| ESLint | `pnpm exec eslint src scripts vite.config.ts eslint.config.js` | **exit 0** |
| TypeScript | `pnpm typecheck` | **exit 0** |
| i18n / 版本 | `node scripts/check-i18n.mjs`、`version.mjs` | 通过（54 keys / 1.1.0） |
| 发布脚本语法 | `node --check scripts/release/build-release.mjs`、`--help` | exit 0，6 个参数不变 |
| Rust 编译 | `cargo check --all-targets` | **0 error / 0 warning** |
| Rust 库测试 | `cargo test --lib` | **362 passed / 7 failed / 8 ignored** |
| Rust 二进制测试 | `cargo test --bin hangline` | **31 passed / 0 failed / 2 ignored** |
| `--selftest` 端到端 | `cargo run --bin hangline -- --selftest` | **exit 1**（PASS 8 / WARN 1 / FAIL 1），证明不初始化 Tauri/窗口 |

**7 个失败的解释（不是回归）**：全部是 `autostart::tests::*` 写 `HKCU\...\Run` 被本会话沙箱拒绝（`os error 5 / PermissionDenied`）。与改动前审计基线（355 passed / 7 failed / 8 ignored）同源、同一批用例。
**数量对账**：355（基线）+ 8（settings/store 新增单测）− 1（删除的 `fullscreen-state` 序列化测试）= **362**，与实测一致。

**`--selftest` 实跑明细**（沙箱环境）：

```
[FAIL] data-dir    C:\Users\...\AppData\Local\Hangline is not writable (os error 5)   ← 沙箱拒绝，非代码问题
[PASS] settings-json  ok (490 B, 18 keys)
[PASS] state-json     ok (1.8 KB, 4 items)
[PASS] disk-free      31.0 GB free of 400.9 GB
[PASS] dpi            96 dpi (100%), awareness unaware
[PASS] display        2 monitor(s); primary 1920x1080; virtual desktop 3840x1080 at (-1920,0)
[PASS] webview2       157.0.4341.42
[PASS] memory         working set 11.7 MB (peak 11.7 MB), 155 handles
[WARN] autostart      not registered (Start at login is off)
[PASS] version        v1.1.0 (debug, x86_64-windows)
PASS 8  WARN 1  FAIL 1  exit code 1
```

> 注意：release 构建是 Windows 子系统、通常无控制台，**现场以 `%LOCALAPPDATA%\Hangline\logs\selftest.log` 为准**（模块文档已写明；日志写不进时 stderr 会明确提示）。

---

## 4. 仍未闭合：两件只能由人做的事

### 4.1 真机验收（已完成：自动化 + 维护者真人）

已按剧本由 AI 在真实解锁桌面上用**合成输入**执行一轮，结果见 [W0 真机验收记录](../acceptance/W0-real-machine-2026-11-09.md)：**PASS 15 项 / BLOCKED 2 项 / SKIP 7 项**。

- 已完成（自动化验证）：冷启动与热键、区域截图（400×250 精确）、点击复制（**实际写入 5 种剪贴板格式**）、双击不污染剪贴板、长按编辑器 58 ms、点击穿透与不抢焦点、多屏 100%↔150% 跟随（面板高 210→315）、跨 DPI 截取、全屏抑制 + **890 ms** 恢复、100 卡摄入 3.6 s 与滚轮、重启持久化、内存基线。
- 仍需真人（约 20–30 分钟）：**拖拽到微信/QQ、文件夹、桌面、回收站**（OLE 拖拽在合成输入下必然被看门狗取消）；托盘图标点击与菜单；真实 `Win+Shift+S`/微信复制 → 上栏与粘贴回微信；Game Bar 录制链路；声音 / HEIC / 150% 视觉主观确认。

**通过标准**：上述真人项完成后，把 `docs/acceptance/P1-core.md` 的 M1/M5/M6/M8/M9/M10/M11/M16 与条件 a–e 中已确认真机项改为 `PASS`；合成输入得到的结论记为「自动化验证」，不替代物理输入项。

### 4.2 签名方案拍板（阻塞"可发布"）

见 §5。决定后需要：新增 `CODE_SIGNING_POLICY.md` 或 README 章节、CI 增加人工审批步骤、发布脚本接入签名服务。

---

## 5. 代码签名方案备忘（W0.7）

| 方案 | 成本 | 门槛 | 优点 | 风险 |
|---|---|---|---|---|
| **A. SignPath Foundation（免费 OSS）** | ¥0 | OSI 许可（MIT ✅）、无闭源组件（✅）、**已发布**（✅ 1.1.0）、**可验证声誉**（⚠️ 仓库 0★ 新项目）、主页需放 Code signing policy 页、团队角色（Author/Reviewer/Approver）、MFA、**每次发布人工审批** | 免证书费；Windows 用户不再见"未知发布者"；签名由基金会持有、可追溯 | 声誉门槛是硬门槛，可能被拒；证书 issuer 是 SignPath Foundation 而非 Hangline；流程增加人工审批环节 |
| **B. Azure Trusted Signing** | 付费订阅（按 Azure 定价页计费） | 需组织身份验证（企业/组织主体）；个人开源项目门槛高 | 微软生态、可长期积累 SmartScreen 信誉 | 需公司主体与持续费用；对个人项目不经济 |
| **C. 自签证书** | ¥0–低 | 无 | 可立即签 | **用户仍需手动信任根证书**，SmartScreen 照样拦截；等于自己做 CA，收益低、观感差 |
| **D. 继续不签** | ¥0 | 无 | 零成本 | SmartScreen「未知发布者」持续损失下载转化；被安全软件误报概率更高 |

**建议（PO 立场）**：先走 **A**——它是唯一"零成本 + OSS 友好"的路径，且 Hangline 的隐私承诺（唯一网络=更新）完全符合基金会的用户隐私条款。声誉门槛用两件事补：① 把 1.1.x 的发布节奏与演示视频做出来；② 申请时附 GitHub 仓库、Release、README/privacy 与 press kit 作为"已被使用"的证据。**A 被拒再评估 B；不要做 C。**

> 依据：[SignPath Foundation OSS 条件](https://signpath.org/terms.html)（检索 2026-11-09）：OSI 许可、无专有组件、需已发布、需可验证声誉、每个发布需人工审批、需 MFA 与角色分工、主页需 Code signing policy。

---

## 6. Wave 0 完成定义（DoD）

- [x] 代码门全绿（ESLint / tsc / i18n / cargo check / 单测基线）
- [x] IPC 表面收敛且不丢诊断能力
- [x] 配置/状态具备"主文件 → `.bak` → 默认值"三层兜底，且不可读文件永不被覆盖
- [x] `--selftest` 可现场自证并输出日志
- [x] 体积门生效（8.5 MB）且报告四项体积
- [x] 内存基线已实测并落盘（主进程 RSS 39.4 MB；进程树私有 293.8 MB → 150 MB 目标需重新定义口径）
- [x] 真机验收：自动化 15 项 PASS + 维护者真人验收完成（2026-11-09，P1 附录 C）
- [ ] 签名方案拍板并落地（或明确延后到 1.1.2 并写清理由）

**7 项已全打勾**：v1.1.1 已切版本（`package.json` / `tauri.conf.json` / `Cargo.toml` 均为 1.1.1），CHANGELOG 已转正，本地提交与构建验证已完成；`push` / `tag` 由维护者决定。

---

## 7. 下一步（W1 启动条件与顺序）

W1 的第一项是**卡片右键菜单**（M 级，《借鉴评估》§7 Wave 1 W1.1）。启动前需确认：

1. Wave 0 的 DoD 7 项全绿（尤其真机验收，因为右键菜单会新增手势入口，必须在手势仲裁基线干净后再动）；
2. E 块内存基线已记录（新增菜单本身不加窗口，但 Wave 1 之后的 Pin/命令面板要用）；
3. `hotkey-error` 的保留决定在 W1「设置可解释化」时兑现（把注册失败真正呈现给用户）。
