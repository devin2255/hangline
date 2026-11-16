# Hangline 1.5.2 — 换个新图标

这次只换外观：**应用图标重新画了一版。**

## 新图标

- 深墨绿丝光底，暖金「真丝绳」从两侧垂下，末端带黄铜绳帽；
- 中间一枚黄铜衣夹，夹着一张落日色的山景明信片；
- 保留 Hangline 的识别符：**照片被晾在绳子上**，并让 16–32 px 的小尺寸轮廓更干净。

旧版图标（深色底 + 暖色绳 + 山景拍立得）用了整个 1.x；新版把绳子的弧度和材质、衣夹的金属感、卡片的投影都重新做了一遍，更贴近 App 内的「绳子材质」体系。

## 技术上做了什么

- 新增可复现的 1024 px 源：`src-tauri/app-icon.png`（由 `scripts/design/app-icon.py` 渲染）；
- 用 `pnpm tauri icon src-tauri/app-icon.png` 重新生成全套：Windows `.ico`（16/24/32/48/64/256）、macOS `.icns`、Microsoft Store 图标、托盘图标；
- 安装器与便携版里嵌入的 `Hangline.ico` 同步更新；press kit 的 `docs/press/hangline-logo.png` 也换成了新标；
- 未改任何功能：拖入、绳子、搜索、标注、更新通道都和 1.5.1 一样。

## 验证

- 图标在 16/32/48/64/128/256 px 下逐尺寸检查（任务栏、Alt+Tab、开始菜单、托盘）；
- `pnpm check`、`cargo test`、体积门、`--selftest` 与 CI 结果随发布核验（见 Release 页）；
- 图标文件为原始 PNG/ICO/ICNS 资产，不涉及运行时逻辑。
