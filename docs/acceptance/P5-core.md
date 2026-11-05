# P5 发布验收（v1.0.0）

- **范围**：P5 发布：未签名 NSIS 安装包（per-user、WebView2 引导、M17 卸载保留/删除数据）与 portable ZIP；`tauri-plugin-updater`（签名 manifest、每日/手动检查、验签安装、重启）；CI/release 工作流；README/press kit/CHANGELOG；隐私清洗；本机安装→使用→升级→卸载生命周期。
- **结论**：**源码发布就绪（本地产物，未上传）**。安装器/portable/签名 manifest 构建通过；本机脚本化生命周期含**真实 0.4.0 旧二进制 → 1.0.0 升级**（见 §3）。干净 Win11 VM、SmartScreen 点击、GUI 驱动的真实安装/升级/卸载与可听声音仍为 MANUAL-PENDING。
- **版本**：1.0.0；tag `v1.0.0`（源码发布里程碑，非 GitHub 发布）。
- **计划/手册**：[P5 计划](../superpowers/plans/2026-11-04-hangline-windows-p5-release.md)、[发布/更新/签名手册](../RELEASE.md)、[CHANGELOG](../../CHANGELOG.md)、[press kit](../press/)。

## 1. 环境与质量门

Windows 11 Home 10.0.26300 x64；Node 22.17 / pnpm 10；Rust 1.90 MSVC；Tauri 2.12.5；NSIS 3.13（官方 zip，缓存于 `src-tauri/target/p5-tools/`）；WebView2 bootstrapper 1,897,688 B。构建脚本 `scripts/release/build-release.mjs`；Tauri bundler 在本机解压 NSIS 时报 `os error 5`，因此使用仓库内 NSIS 脚本（应用 exe 仍由 `tauri build --no-bundle` 产出）。

| 门 | 结果 |
|---|---|
| `pnpm check` | **PASS**：lint + tsc + **26 files / 210 tests** + i18n 54 + version 1.0.0 |
| `cargo test` | **PASS**：**356 passed / 0 failed / 8 ignored** |
| `cargo check --all-targets` | **PASS**，0 warnings |
| `cargo check --release --all-targets` | **PASS**，0 warnings |
| `node scripts/release/build-release.mjs --sign` | **PASS**（setup 7,315,272 B ≤15 MB；便携 8,430,050 B；真实 updater 签名） |
| `.github/workflows/{ci,release}.yml` | YAML 语法校验通过（PyYAML）；**未在 GitHub 实跑**（无远端/`gh` 登录） |

## 2. 产物（本地 `release/`，git-ignored，未上传）

> 本节数字由 `node scripts/release/build-release.mjs --sign` 在 v1.0.0 上生成后填入；`build-info.json`、`SHA256SUMS.txt` 为权威。

| 产物 | 大小 | SHA-256 | 备注 |
|---|---|---|---|
| `Hangline_1.0.0_x64-setup.exe` | **7,315,272 B (7.32 MB)** | `8BD0BE9C6DD618A6C72FF6D2DEAD3492A9B5B98676217D8D298DBB6E09483C68` | Authenticode **NotSigned**；≤15,000,000 B 门禁 PASS |
| `Hangline_1.0.0_x64-portable.zip` | **8,430,050 B (8.43 MB)** | `3946BE39A62D7C7C12B6D7B493BD2B925C78E51E4331665044DC04C3F887DA1E` | exe + icon + `portable` marker + 双语 README（更新检查说明已与 1.0.0 一致） |
| `latest.json` | — | — | 真实 updater 签名（`windows-x86_64-nsis` + `windows-x86_64`，436 字符）；占位 GitHub endpoint |

构建溯源：产物由 `build-info.json` 记录（`git_commit 59991fa`、NSIS/WebView2 哈希、大小、签名来源；不包含主机绝对路径）。`59991fa` 是**压缩前的私有开发历史**中的最后一个代码/脚本提交；公开仓库以同一代码树的单提交历史发布（`v1.0.0` tag 指向该公开提交），构建后只有文档修改，不改变应用或安装器行为。`build-info.json` 属于本地 git-ignored 发布产物，不随公开仓库发布。

## 3. 本机生命周期（脚本化，隔离）

`scripts/release/verify-installer.ps1`（P5-5）:
- 静默安装到默认 `%LOCALAPPDATA%\Programs\Hangline`，验证 exe/uninstaller/Start-Menu 快捷方式/HKCU Uninstall 项。
- 以隔离 `LOCALAPPDATA` 启动 release exe（不触碰真实用户数据），验证进程存活、数据文件创建、正常退出。
- 重新安装（升级语义）验证文件版本更新、设置/状态保留。
- `/S` 卸载（默认保留数据）→ 文件/快捷方式/注册表清理、数据目录保留；再次安装后分别用 `HANG_UNINSTALL_DATA=delete` 与 `/S /DELETEUSERDATA` 两条显式删除路径 → 测试数据目录删除；升级前创建的数据 marker 跨 0.4.0→1.0.0 保留；无遗留 Run/StartupApproved/进程。
- portable ZIP 解压到 `%TEMP%` 启动测试。
- 证据：`.superpowers/p5-evidence/installer/`（git-ignored）。

**结果：PASS（84/84 检查，0 failures，12 steps）**。测试安装器由同一 `installer.nsi` 以 `/D` 覆盖安装目录、数据目录、快捷方式目录及 **HKCU Uninstall/Run/StartupApproved 测试键** 到 `%TEMP%` 与 `*P5Test` 键编译，**不触碰真实安装、真实注册表项或真实用户数据**；真实安装/数据/卸载键/自启项在运行前后快照一致。升级基线使用 `.superpowers/p5-evidence/old-0.4.0/` 中归档的 **0.4.0 portable 内的真实旧 exe**（20,358,656 B, FileVersion 0.4.0）。

清理后 `release/` 只保留 1.0.0 可发布产物；0.4.0 基线仅存在于 git-ignored 证据目录。

| 场景 | 判定 |
|---|---|
| 静默安装 + 文件/快捷方式/HKCU 项 | 84/84 PASS：exe/uninstaller/icon/Start-Menu 快捷方式/HKCU Uninstall（DisplayVersion、InstallLocation、QuietUninstallString）齐全；HKCU 写入的是 `HanglineP5Test` 测试键 | **PASS** |
| 隔离启动（进程/数据/退出） | 安装版 exe 存活 6 s、创建 isolate `LOCALAPPDATA\Hangline` 数据、强制结束清理成功；真实用户数据未变 | **PASS** |
| 版本升级（0.4.0 → 1.0.0，真实旧二进制） | 用归档 0.4.0 portable 中的真实旧 exe 编译基线测试安装器 → 安装后 exe/DisplayVersion **0.4.0**；升级前创建数据 marker；再覆盖 1.0.0 → exe/DisplayVersion **1.0.0**、数据 marker 保留 | **PASS** |
| 卸载保留数据 | 卸载后安装目录/快捷方式/注册表清理，数据目录与 marker 保留；真实安装/数据/自启未变 | **PASS** |
| 卸载删除数据（显式 flag） | 重新安装 → `HANG_UNINSTALL_DATA=delete` `/S` 卸载 → 测试数据目录删除、注册表清理；真实路径未变 | **PASS** |
| portable 解压运行 | ZIP 解出 exe/icon/`portable` marker，隔离 `LOCALAPPDATA` 启动存活 | **PASS** |
| 真实干净 Win11 VM / SmartScreen 点击 / GUI 驱动安装-升级-卸载 | **MANUAL-PENDING**（脚本化旧二进制升级已 PASS） |

## 4. M16 / M17 / 更新

| 项 | 证据 | 判定 |
|---|---|---|
| M16 退出/重启：设置无残留、条目按存在性恢复 | P1 持久化单测 + 安装版隔离启动/退出 | **PASS（程序化）**；真实重启人工 |
| M17 卸载保留/删除数据提示 | installer.nsi `un.ResolveDataPolicy`（M17 label fallthrough 已在 bc8580b 修复；静默默认保留，仅显式 `HANG_UNINSTALL_DATA=delete` || `/DELETEUSERDATA` 删除）；本机测试安装器 keep/delete（env 与 `/DELETEUSERDATA` 两条显式路径）**84/84 PASS** | **PASS（程序化）**；真实 UI 点击人工 |
| 更新检查（手动/每日门控、离线提示、零其它网络） | `updater.rs` 15 单测 + `scripts/release/update-check-test.mjs` 本地端点 **30/30 PASS**（有更新/无请求/24h/离线/假包被签名拒绝）；`latest.json` 以轮换后的新密钥真实签名（436 字符） | **PASS（程序化）** |
| 真实签名更新：下载→passive 安装→重启 | 需要已安装旧版 + 托管 `latest.json`；本地已生成签名 manifest（新密钥），更新检查 30/30（fixture appVersion 0.4.0；逻辑证据，未在 1.0.0 构建重跑） | **CI/MANUAL-PENDING** |
| 升级 tag/版本一致性、release workflow | release.yml 校验 tag==version；YAML 校验通过 | **待 CI 实跑（无远端）** |

## 5. 隐私与发布卫生

- HEAD 文本已清洗：主机用户名路径统一改为 `%USERPROFILE%` 或 `C:\Users\Example`；`git grep -i` 无残留主机路径/用户名。
- 提交截图均为应用 UI 或中性合成 fixture；`docs/press/` 只含这些。
- **公开仓库已用压缩后的单提交历史发布**（`github.com/devin2255/hangline`）：原始开发历史（含早期本机路径）只保留在本地，未推送；历史 tag v0.1.0-core…v0.4.0-settings 也未推送，公开仓库只有 `v1.0.0`。提交截图抽查为应用 UI/中性 fixture（黑色背景、Temp 目录 Explorer、合成卡片），无真实桌面内容。
- 无安装包/私钥/`release/`/`.superpowers/` 入库；`release-keys` 仅本地 git-ignored；0.4.0 基线产物归档在 `.superpowers/p5-evidence/old-0.4.0/`（git-ignored），`release/` 只保留 1.0.0 可发布产物。
- 签名密钥已在 P5 收尾时轮换（早期构建脚本曾让 pnpm 回显 `-p` 临时参数，密码进入本地临时日志；已改为只通过子进程环境变量传递，未发布任何密钥/密码，新 pubkey 已写入 `tauri.conf.json` 并重新签名产物）。
- i18n en/zh-Hans 54 keys 对齐；安装器双语页面（English/SimpChinese）。

## 6. 人工验收清单（用户执行）

> 全部 MANUAL-PENDING；请在干净 Windows 11（或新用户配置）真实桌面执行。

1. 双击 `Hangline_1.0.0_x64-setup.exe`：SmartScreen「未知发布者」→ 仍可继续；安装到 `%LOCALAPPDATA%\Programs\Hangline`；开始菜单/桌面快捷方式出现。
2. 首次启动：托盘出现；截图→上栏→复制→标注→录屏→裁剪抽查；无旧数据残留。桌面快捷方式是可选项（安装组件页默认不勾选）。
3. 在**没有 WebView2** 的 Win10 22H2 上安装：会提示/静默补装运行时（需要网络）；断网时给出明确错误且不留下半装应用。
4. 升级：先装 1.0.0，再运行新版本 setup；运行中的 Hangline 被关闭并替换；设置/线上条目/保留数量保留。
5. 托盘「检查更新…」：无网络/无 Release 时给出可理解提示、不崩溃；有签名更新时提示版本→安装→重启（需发布后/CI 环境）。
6. 卸载选择「保留数据」：设置/截图仍在，重装恢复；选择「删除数据」：`%LOCALAPPDATA%\Hangline` 被删除；无快捷方式/注册表/进程残留。
7. portable ZIP 任意目录可运行，`portable` marker 存在；数据位置与安装版一致（V1 说明）。
8. SHA-256 与 `SHA256SUMS.txt`/Release 附件一致；未签名说明可见；中文/英文安装界面语言正确。
9. 干净 Win11 VM 完成安装→使用→升级→卸载全链路；真实文件安全（杀软）抽查。
10. 真实可听声音、真实全屏/托盘/热键（P1–P4 人工清单）抽查。

## 7. 已知限制

- **未签名安装包**：SmartScreen 未知发布者；无 Authenticode，仅 SHA-256 + updater 签名（两套独立机制）。V1 不提供代码签名/自签。
- **WebView2 引导需要网络**：内置的是 Evergreen bootstrapper（1.9 MB），不是离线 runtime；无 WebView2 且断网的机器会安全中止（exit 3）。
- **portable 模式**：V1 的 portable ZIP 仍把数据存放在 `%LOCALAPPDATA%\Hangline`；`portable` marker 只是标记，不是真正便携数据目录（post-V1）。
- **公开下载/CI/自动更新实网**：无 GitHub 远端与 `gh` 登录，本会话不运行 release workflow、不发布 Release；更新端点/仓库为占位，需运营者创建仓库并配置签名 secrets。
- **历史隐私**：HEAD 已清洗，但历史中的本机路径/P0 截图需重写或 squash 后才能公开。
- **安装环境**：本机为 Windows 11 + WebView2；Win10 22H2/干净 VM/真实 SmartScreen 点击、杀软文件保护、11 语言扩展均未验证。生命周期测试使用重定向路径与 `HanglineP5Test`/`RunP5Test` 测试注册表键（卸载键/自启键均隔离），GUI 真实安装/卸载流程仍需人工确认。
- **Tauri bundler 环境缺陷**：本机 `pnpm tauri build` 的 NSIS 解压报 `os error 5`，故使用自定义 NSIS 脚本；CI 使用脚本自身下载的 NSIS，不依赖 Tauri bundler。

## 8. 证据文件

- 本文档、[发布手册](../RELEASE.md)、[CHANGELOG](../../CHANGELOG.md)、[press kit](../press/)。
- 构建产物与日志：`release/`（git-ignored）；构建证据 `.superpowers/p5-evidence/`（git-ignored）。
- 更新检查证据：`.superpowers/p5-evidence/update-check/update-check-test.json`（30/30）。
- 提交文本/截图不含本机路径；历史重写要求见 §5/§7。
