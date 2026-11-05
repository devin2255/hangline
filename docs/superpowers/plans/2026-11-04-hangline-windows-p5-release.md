# Hangline P5 Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Ship Hangline 1.0 as a source release: unsigned NSIS installer (per-user, WebView2 bootstrap, uninstall data prompt M17) + portable zip, updater plugin wired to GitHub Releases, CI/release workflows, README/press kit/CHANGELOG, privacy-scrubbed committed text, local install→upgrade→uninstall lifecycle evidence and P5 acceptance. Installer ≤ 15 MB.

**Architecture:** `pnpm tauri build --no-bundle` produces the app exe (already works); a checked-in NSIS script compiled with the official NSIS 3.13 toolchain builds the installer, because this workstation's Tauri bundler fails during its NSIS download/extract step with `os error 5` (env/Defender issue, not project code). Release scripts cache and verify the NSIS zip + WebView2 bootstrapper, produce `setup`, portable zip, SHA-256 and `latest.json`, enforce the size gate, and sign the manifest with a Tauri updater key whose private half stays out of git/CI logs.

**Tech Stack:** Tauri 2.12 + `tauri-plugin-updater` 2, NSIS 3.13 (makensis), PowerShell 7/5.1 scripts, Node 22 release script, GitHub Actions windows-latest, optional code signing (none in V1).

**Spec:** `docs/superpowers/specs/2026-10-28-hangline-windows-design.md` §3.10 (i18n), §4.8 (directories), §6.1–6.3 (build/release/update), §7 (M16/M17), §8 (P5), §9 (risks).

## Global Constraints

- Windows 10 22H2/11 x64; no new runtime dependencies except `tauri-plugin-updater` (spec-mandated); no installer > 15 MB.
- **Network behavior:** the app makes no network requests except the updater (daily if `autoCheckUpdates`, or manual tray). README/RELEASE/第一次启动文案必须明确这一点。
- **Unsigned V1:** NSIS setup is unsigned → SmartScreen “unknown publisher”; mitigate with SHA-256, GitHub Release attachment, docs + optional SignPath/Azure Trusted Signing later (spec §6.2). Never fake a signature.
- **User data safety:** uninstall asks keep/delete (M17); default keep; silent uninstall defaults keep unless `HANG_UNINSTALL_DATA=delete`/`/DELETEUSERDATA` is explicitly passed; never delete screenshots without explicit choice.
- Installer per-user (`RequestExecutionLevel user`, default `%LOCALAPPDATA%\Programs\Hangline`), no admin; WebView2 bootstrapper embedded for offline Win10/11 (1.9 MB, size budget OK).
- Evidence privacy: committed screenshots app-UI/neutral only; committed JSON must be sanitized (`%USERPROFILE%`, no `<user>`/host paths); git history rewrite remains a documented pre-public step.
- Gates per task: `pnpm check`, `cargo test`, `cargo check --all-targets` 0 warnings, `cargo check --release --all-targets`, `pnpm tauri build --no-bundle`; commits on `feat/core`; clean tree. Implementers do not edit `docs/superpowers/specs|plans` (controller owns those).
- Manual acceptance (real clean VM/SmartScreen/upgrade over old version) is never claimed PASS; programmatic/local lifecycle evidence is labelled separately.

## Reference data

| Item | Value | Source |
|---|---|---|
| App version | 1.0.0 (package/tauri/Cargo/lock synced) | spec §6.1 |
| Installer | `Hangline_1.0.0_x64-setup.exe`, per-user, default `%LOCALAPPDATA%\Programs\Hangline` | spec §6.1 |
| Installer size | target ≤ 15 MB (bare LZMA test: 4.43 MB + WebView2 1.90 MB ≈ 6.5 MB) | measured; spec §1/§8 |
| Portable | `Hangline_1.0.0_x64-portable.zip` with `Hangline.exe`, `Hangline.ico`, `portable` marker, README | spec §6.1 |
| WebView2 | embedded bootstrapper (`MicrosoftEdgeWebView2Setup.exe` ≈ 1.90 MB), `/silent /install` if missing | spec §6.1 |
| Server URL | `https://github.com/hangline-app/hangline/releases` (placeholder repo; user must create/rename) | actions.rs `RELEASES_URL` |
| `latest.json` | Tauri v2 updater format: `{version, notes, pub_date, platforms: {"windows-x86_64": {signature, url}}}` | tauri-plugin-updater docs |
| Signing | `pnpm tauri signer generate`; public key in `tauri.conf.json` `plugins.updater.pubkey`; private key git-ignored / CI secret | spec §6.3 |
| Uninstall data | prompt keep/delete; silent default keep; `HANG_UNINSTALL_DATA=delete` for tests | spec M17 |
| M16 exit/restart | settings/items restored by existence; no leftover settings changes | spec §7.2 |
| M17 uninstall | user data keep/delete choice; no orphan files/shortcuts/registry | spec §7.2 |

## File structure

```
scripts/release/installer.nsi          # checked-in NSIS installer/uninstaller (new)
scripts/release/build-release.mjs      # build exe + setup + portable + sha256 + latest.json (new)
scripts/release/fetch-tools.ps1        # cache NSIS 3.13 + WebView2 bootstrapper (new)
scripts/release/README.md              # release runbook (new)
scripts/release/verify-installer.ps1   # silent install/upgrade/uninstall smoke (new)
src-tauri/src/updater.rs               # updater integration: daily check, commands/events (new)
src-tauri/src/actions.rs               # real "Check for updates…" (modified)
src-tauri/src/tray.rs                  # update state/menu (modified)
src-tauri/src/lib.rs                   # plugin init + commands (modified)
src-tauri/Cargo.toml                   # tauri-plugin-updater (modified)
src-tauri/tauri.conf.json              # version 1.0.0, updater config/pubkey (modified)
src-tauri/capabilities/*.json          # updater permissions for panel windows (modified)
src/windows/panel/Panel.tsx            # update-available toast/action (modified)
src/lib/ipc.ts, types.ts               # update commands/events (modified)
.github/workflows/ci.yml               # size/installer gate (modified)
.github/workflows/release.yml          # tag-triggered release (new)
docs/acceptance/P5-core.md             # M16/M17/P5 matrix + manual checklist (new)
docs/RELEASE.md                        # release/update/signing/privacy runbook (new)
docs/press/                            # press kit (logo, screenshots, blurbs) (new)
CHANGELOG.md                           # v1.0.0 (new)
README.md, README.zh-Hans.md           # install/download section, P5 status (modified)
```

---

### Task P5-1: NSIS installer + portable zip + size gate

**Files:** `scripts/release/{installer.nsi,fetch-tools.ps1,build-release.mjs}`, tests/runbooks; no app code changes yet.

- `fetch-tools.ps1`: download the official NSIS 3.13 zip from the Tauri binary-releases URL (retry, SHA/zip sanity), extract via `Expand-Archive` into `src-tauri/target/p5-tools/nsis-3.13` (git-ignored), download `MicrosoftEdgeWebView2Setup.exe` (~1.9 MB) into `target/p5-tools/`, verify `makensis /VERSION`.
- `installer.nsi`: Unicode; per-user; install dir default `%LOCALAPPDATA%\Programs\Hangline`; pages (welcome, directory, install, finish with “run Hangline”); copy `Hangline.exe`, icon; create Start-Menu shortcut + optional desktop shortcut; write HKCU Uninstall key (`DisplayName Hangline`, version, publisher, icon, `UninstallString "$INSTDIR\uninstall.exe"`, `QuietUninstallString ... /S`); install uninstaller; WebView2 detection (registry `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F1E7...}`/`EdgeUpdate` keys; if missing run bundled bootstrapper `/silent /install` and wait); upgrade path: close a running Hangline (`nsExec::Exec 'taskkill /IM hangline.exe /F'`) before replacing; uninstall: remove shortcuts/registry/files, then **data prompt** (keep/delete): keep default; delete only when chosen (`MessageBox` in UI, silent env `HANG_UNINSTALL_DATA=delete` or `/DELETEUSERDATA`), delete `%LOCALAPPDATA%\Hangline` (settings/state/thumbnails/screenshots) and optionally the install dir; never delete user files otherwise. Include English + SimpChinese MUI language.
- `build-release.mjs` (Node): `pnpm check`; `pnpm tauri build --no-bundle`; copy exe; call `fetch-tools.ps1`; copy installer assets; `makensis` compile to `release/Hangline_<version>_x64-setup.exe`; build portable `release/Hangline_<version>_x64-portable.zip` (exe + icon + `portable` marker + `README-portable.txt`); compute SHA-256 for both; size gate **≤15 MB** (fail hard); write `release/latest.json` (signed later in P5-2/P5-3; initially unsigned manifest with `signature:"PLACEHOLDER"` only for layout test, clearly marked); print artifact table.
- Tests/evidence: run `node scripts/release/build-release.mjs` on this machine; installer size ≈6.5 MB; `signtool` absent (unsigned), `Get-AuthenticodeSignature` = NotSigned; portable zip contains expected files; NSIS compile logs.
- Commit `feat: P5 release packaging (NSIS + portable)`.

### Task P5-2: Updater integration (tauri-plugin-updater)

**Files:** `src-tauri/src/updater.rs` (new), `lib.rs`, `actions.rs`, `tray.rs`, `tauri.conf.json`, capabilities, `src/lib/{ipc,types}.ts`, `Panel.tsx`; tests.

- Add `tauri-plugin-updater = "2"`; configure `plugins.updater` `endpoints: ["https://github.com/hangline-app/hangline/releases/latest/download/latest.json"]`, `windows.installMode: "passive"`, `pubkey` from a locally generated signer key (private key under `.superpowers/release-keys/`, git-ignored; document CI secret). Generate via `pnpm tauri signer generate -w <path>` (verify command availability; if the CLI refuses, use the generated key format from the plugin docs).
- `updater.rs`: `check(manual: bool)` gating: manual always; automatic only when `autoCheckUpdates` and last check >24 h (`last_update_check` ISO); update settings timestamp; call plugin `app.updater()?.check()`; on update emit `update-available {version, notes}` + tray/toast; on none/log offline quietly (manual shows a tray hint). `download_and_install` command with progress event → `update-progress {chunk, total}`; after install `app.restart()`. Errors never panic; `manual` offline shows “couldn’t check”; never another network call.
- Tray: “Check for updates…” uses real check (manual) instead of opening Releases URL; add “Restart to update”/state row if an update was downloaded (keep menu small; at least log + toast). `update-available` event handled in panel with a toast/action.
- Tests: `last_check_allows` pure (hour boundaries, clock back), `update-available` event mapping, offline error classification; `cargo check`/unit tests. Runtime test without a public endpoint: point the plugin at a **local file/HTTP fixture in a debug-only test** or inject a `HANG_UPDATE_ENDPOINT` debug override (cfg debug) to serve a signed `latest.json` + fake update; verify manifest parsing/signature failure handling. If true update install cannot be exercised without a new signed build, document as MANUAL/CI-pending and do not claim PASS.
- Commit `feat: P5 updater integration`.

### Task P5-3: CI + release workflow

**Files:** `.github/workflows/{ci.yml,release.yml}`, `scripts/release/README.md`; maybe `scripts/check-release-instrumentation` reuse.

- `ci.yml`: keep install/check/test; add `pnpm tauri build --no-bundle`; run `node scripts/release/build-release.mjs --ci` (uses NSIS download; no signing key → unsigned artifacts + manifest placeholder is allowed only for CI validation); assert setup exists and ≤15 MB; upload installer/portable/SHA as workflow artifacts.
- `release.yml`: `on: push: tags: ['v*']`; windows-latest; build; sign manifest/artifact with `TAURI_SIGNING_PRIVATE_KEY` (+password) secret; generate `latest.json` with real signature; create GitHub Release with `gh release create` (GITHUB_TOKEN) uploading setup, portable, latest.json, SHA256SUMS; fail if size >15 MB or version != tag; notes from tag/CHANGELOG.
- Local release runbook (`scripts/release/README.md` + `docs/RELEASE.md`): version bump, `pnpm check`, build, sign, sha256, `gh release create`, verify download, SmartScreen note, update key rotation.
- Validate workflows with `actionlint`? Not installed; at minimum YAML parse via PowerShell/`python yaml` not installed; use `pnpm exec prettier --check`? Document manual review. Do not claim CI ran.
- Commit `ci: P5 release workflow and size gate`.

### Task P5-4: Privacy scrub, README/press kit, CHANGELOG

- Sanitize committed text paths: replace `%USERPROFILE%` / `<user>\<user>` in docs/spec/tests with `%USERPROFILE%` / `C:\Users\Example` (tests only assert Windows path handling; do not weaken them); scan all tracked text for host paths/usernames/private temp paths; list any committed P0 images that show the desktop and quarantine/remove them from the release docs (keep git-ignored copies). Document that **git history** still contains old paths/screenshots → public release requires a fresh repo or history rewrite (RELEASE.md), because rewriting shared history is outside P5.
- README EN/ZH: install/download section (GitHub Release placeholder), SmartScreen/unsigned note, update behavior (only network), portable usage, data locations, uninstall keep/delete, requirements (Win10 22H2+/11, WebView2), build-from-source fallback; P5 status + links; keep app-UI screenshots.
- `CHANGELOG.md`: v1.0.0 with P1–P5 highlights; `docs/RELEASE.md`: release/update/signing/privacy/rollback/versioning runbook; `docs/press/`: logo SVG/PNG (from original icon), 3–5 selected app-UI screenshots, EN/ZH feature blurb, contact placeholder, license/attribution text.
- Check i18n completeness (48 keys), no hardcoded user strings in new UI; version 1.0.0 bump in package/tauri/Cargo/lock + `scripts/version.mjs` green.
- Commit `docs: P5 release docs, press kit and privacy scrub`.

### Task P5-5: Install→use→upgrade→uninstall lifecycle (M16/M17) + portable smoke

**Files:** `scripts/release/verify-installer.ps1`, `docs/acceptance/P5-core.md` (draft evidence); no app code unless a lifecycle bug appears.

- Silent install to the per-user default: `setup.exe /S`; assert exit 0, `%LOCALAPPDATA%\Programs\Hangline\Hangline.exe` + `uninstall.exe`, Start-Menu shortcut, HKCU Uninstall key/version/icon, WebView2 check skipped (present).
- Launch smoke: start exe, wait for tray/panel process; create a fixture screenshot in the isolated data dir? Release app uses real `%LOCALAPPDATA%`; **do not disturb the user's real Hangline data** — use a temporary `LOCALAPPDATA` for launch? The release app reads `LOCALAPPDATA` env for its data dir (capture.rs), so start with an isolated LOCALAPPDATA + APPDATA args, verify process stays up + data files created + graceful quit; do not test real user data.
- Upgrade: build a second version (e.g. 1.0.1 test artifact) or bump/hot-patch the setup version; install over 1.0.0; assert exe/version updated, settings/state preserved in the isolated profile, shortcuts/registry intact, running app closed/replaced as designed. (If building another full release is too slow, use a 1.0.1 installer generated with the same script and a bumped test version; revert version afterwards.)
- Uninstall keep: `uninstall.exe /S`; assert files/shortcuts/registry removed, data dir kept; then uninstall delete: install again, `uninstall.exe /S` with `HANG_UNINSTALL_DATA=delete`, assert data dir removed; no `Hangline` Run value (autostart), no stray processes.
- Portable zip: extract to `%TEMP%`, run exe with isolated LOCALAPPDATA; assert launches; `portable` marker present; document that V1 portable still stores data in `%LOCALAPPDATA%\Hangline` (spec’s marker is informational; a true portable mode is post-V1).
- Record JSON evidence + exact commands; mark real clean-Win11 VM/SmartScreen/real-file install as MANUAL-PENDING.
- Commit `test: P5 install/upgrade/uninstall lifecycle evidence`.

### Task P5-6: P5 acceptance, final review, tag v1.0.0

- `docs/acceptance/P5-core.md`: environment, gates, artifact table (setup/portable sizes, SHA-256, Authenticode NotSigned), install/upgrade/uninstall matrix (M16/M17), updater behavior (manual/daily/offline; real update install CI/manual-pending), privacy/history note, manual checklist (clean Win11 VM, SmartScreen, real user-data keep/delete, real update over an installed old version, tray/updater restart), known limitations.
- README/RELEASE/CHANGELOG final pass; version 1.0.0; run all gates + final release script; **Lead** runs final whole-branch review (Rust/frontend/docs/release artifacts), then tags **`v1.0.0`** and reports to the user with download artifact paths (not uploaded anywhere; GitHub release requires the user’s repo/token).
- Do not claim CI ran or a public release exists; artifacts are local under `release/` (git-ignored) with exact sizes/hashes documented.

---

## Manual acceptance checklist (user-run; goes into `docs/acceptance/P5-core.md`)

1. 干净 Windows 11（或全新用户配置）双击 setup：SmartScreen「未知发布者」可见但可继续；安装到 `%LOCALAPPDATA%\Programs\Hangline`，开始菜单/桌面快捷方式可用。
2. 首次启动：无残留旧数据；托盘出现；Win10 22H2（无 WebView2）机器上安装器会静默补装 WebView2。
3. 使用：截图→上栏→复制→标注→录屏→裁剪 全链路正常（复用 P1–P4 清单抽查）。
4. 升级：先安装旧版（1.0.0），再运行新版 setup；运行中的 Hangline 被正确关闭/替换；设置、线上条目、保留数量保持。
5. 更新检查：托盘「检查更新…」在无网络/无 Release 时给出可理解提示、不崩溃、不产生其它网络请求；有更新时提示版本/说明并可下载安装、重启后版本变化（发布后/CI 验证）。
6. 卸载保留数据：设置/截图仍在，重装后恢复；卸载删除数据：`%LOCALAPPDATA%\Hangline` 被移除、快捷方式/注册表/进程无残留。
7. portable zip 解压到任意目录可运行，`portable` 标记存在；数据位置与安装版一致（V1 说明）。
8. 断电/崩溃后数据一致性抽查；退出重启（M16：截图设置无残留、条目按存在性恢复）。
9. SHA-256 与 release 附件校验一致；无代码签名说明已在 README/RELEASE 披露。
10. 中文/英文安装界面与运行时语言一致。

## Self-review checklist (controller)

- Spec coverage: §6.1 build/artifacts, §6.2 signing/release process, §6.3 updater signing, §7 M16/M17, §8 P5 DoD (clean lifecycle, ≤15 MB), §9 risks (unsigned, AV, update fallback), §3.10 i18n.
- No placeholders except deliberately documented: updater endpoint/repo is a placeholder until the user publishes a GitHub repo; public release/CI run is user-gated.
- Safety: uninstall data prompt/default keep; isolated launch does not touch the user's real data; no private screenshots committed.
- Consistency: version 1.0.0 everywhere; artifact names/hashes in docs match `release/`; tag v1.0.0 by Lead only.
