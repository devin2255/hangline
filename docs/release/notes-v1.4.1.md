# Hangline 1.4.1 — 国内网络也能检查更新

这个版本只修一件事：**不开 VPN 也能检查更新、下载更新**。

## 为什么之前不行

「检查更新」原来是直连 GitHub Releases。GitHub 的清单和安装包下载域在部分中国大陆网络不可达，
所以应用会提示「检查更新失败」。这不是应用逻辑坏了，是网络通道断了。

## 现在怎么做

- **四个更新源依次尝试**：官方 GitHub → `ghfast.top` → `gh-proxy.com` → `ghproxy.net`。
  前一个连不上就自动换下一个，国内网络直接走镜像，海外网络仍优先官方源。
- **检查快、下载稳**：检查清单时每个源最多等 5 秒（被墙的官方源快速跳过）；下载安装包不设总
  超时，慢一点的网络也能把约 7 MB 的包下完。
- **镜像不可作恶**：三份镜像清单与原清单签名完全一致，安装包仍做 minisign 验签；同时开启了
  `requireSignedVersion`，镜像无法用"更高版本号 + 旧版安装包"骗你降级。
- **新增的 3 个地址只用于更新检查/下载**：没有遥测、没有账号、没有其他联网行为。

## 国内用户首次安装（重要）

**v1.4.0 及更早版本没有镜像通道**，所以国内网络下它们查不到这个更新——需要手动安装一次
v1.4.1。三个加速下载地址（任选其一，下载后请用 `SHA256SUMS.txt` 校验）：

- 官方：<https://github.com/devin2255/hangline/releases/download/v1.4.1/Hangline_1.4.1_x64-setup.exe>
- ghfast：<https://ghfast.top/https://github.com/devin2255/hangline/releases/download/v1.4.1/Hangline_1.4.1_x64-setup.exe>
- gh-proxy：<https://gh-proxy.com/https://github.com/devin2255/hangline/releases/download/v1.4.1/Hangline_1.4.1_x64-setup.exe>
- ghproxy.net：<https://ghproxy.net/https://github.com/devin2255/hangline/releases/download/v1.4.1/Hangline_1.4.1_x64-setup.exe>

安装 v1.4.1 之后：托盘「检查更新…」或每日自动检查都能在国内网络正常工作，以后升级不用再手动下载。

## 验证

- 三份镜像清单与原清单：版本、notes、pub_date、签名逐字节一致，安装包 URL 分别指向对应加速前缀；
- 签名可信注释含 `version:1.4.1`，`requireSignedVersion` 校验通过；
- 发布构建通过 8.5 MB 体积门，`--selftest` PASS 9 / FAIL 0；
- 本地更新 harness 新增「死端点 → 镜像端点」回退场景（`scripts/release/update-check-test.mjs`）；
- 无 VPN 环境下通过三个代理分别下载安装包并核对 SHA-256 一致（发布后核验）。

## 升级说明

- 直接覆盖安装即可，设置与数据兼容；
- 仍未做 Authenticode 签名，SmartScreen 可能提示未知发布者；请用 `SHA256SUMS.txt` 校验；
- 唯一网络请求仍是更新检查（现在多了一条镜像兜底路径）。
