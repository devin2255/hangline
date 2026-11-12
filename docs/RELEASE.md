# Release, update and signing notes (P5 / v1.0.0)

Hangline 1.0.0 is a **source release**. The repository builds a per-user installer and a portable
ZIP locally; nothing is uploaded by a developer machine. This document is the reviewer/operator
runbook for producing and publishing a build, and records the release constraints.

## Artifacts (v1.0.0, built by scripts/release/build-release.mjs)

| Artifact | Expectation |
|---|---|
| `Hangline_1.0.0_x64-setup.exe` | per-user NSIS installer, **≤8.5 MB** decimal (`SETUP_LIMIT_BYTES = 8,500,000`; 1.0.0 build 7,315,272 B), Authenticode **NotSigned** for now (SignPath Foundation signing planned — see `CODE_SIGNING_POLICY.md`) |
| `Hangline_1.0.0_x64-portable.zip` | `Hangline.exe`, `Hangline.ico`, empty `portable` marker, bilingual README |
| `SHA256SUMS.txt` | SHA-256 of the two artifacts |
| `latest.json` | Tauri v2 updater manifest; signed with the updater key (never the Authenticode key) |
| `build-info.json` | build provenance (versions, tool hashes, sizes, signature source; no key material) |

All artifacts are git-ignored under `release/`.

## Size budget (W0.6)

The installer gate is a hard **8,500,000-byte (8.5 MB decimal)** limit enforced by
`SETUP_LIMIT_BYTES` in `scripts/release/build-release.mjs`. The 1.1.0 packaging baseline measured
**7,315,272 B**: ~86% of the gate, ~1.18 MB / ~16% headroom over the current build. The old 15 MB
limit would have tolerated a ~2x regression before failing, so it was not a real gate. The portable ZIP has a separate 25 MB sanity guard and is *not* compared against the 8.5 MB
installer budget (it carries the uncompressed exe, so it can legitimately be larger).

After the artifact table the script prints a **Size budget** block with the four numbers behind the
gate (a missing optional input is reported as `skipped`, never invented):

| Report item | What it measures | Notes |
|---|---|---|
| `application exe` | Uncompressed `src-tauri/target/release/hangline.exe` — the raw Rust + WebView2 app | Not gated; the first thing to compare when the setup grows |
| `NSIS setup` | The compressed installer and the **gated** artifact; it **includes** the embedded WebView2 bootstrapper | Gate 8,500,000 B; 1.1.0 baseline 7,315,272 B (~86%), **1.2.0 baseline 7,404,534 B (87.11% used, ~1.10 MB headroom)**, **1.3.0 baseline 7,466,233 B (87.84% used, ~1.03 MB headroom; published CI build 7,326,593 B / 86.19%, toolchain difference)**, **1.3.1 baseline 7,475,103 B (87.94% used; published CI build 7,327,500 B / 86.21%)**, **1.4.0 baseline 7,452,381 B (87.68% used)** |
| `WebView2 bootstrapper` | The ~1.9 MB Microsoft Evergreen bootstrapper that NSIS embeds. It is not a separate release file, so it is measured from the cached download and labelled `embedded in setup, not separately measurable` | Already part of the setup number — do not subtract it twice |
| `portable ZIP` | The same exe deflated with icon, `portable` marker and bilingual README | Guarded at 25 MB only; ~8.4 MB is normal and may exceed 8.5 MB |

Each row shows `Bytes`, `MB` and a detail/status; the gate lines also show usage and headroom:

```text
setup gate: 7,315,272 / 8,500,000 bytes = 86.06% used, headroom 1,184,728 bytes (13.94%) -> PASS
portable guard: 8,430,050 / 25,000,000 bytes = 33.72% used, headroom 16,569,950 bytes (66.28%) -> PASS
```

If the setup exceeds the gate the build **stops** with
`installer exceeds the 8,500,000-byte (8.5 MB) budget` before signing, `SHA256SUMS.txt` and
`latest.json`, so no over-budget artifact is treated as releasable. Diagnose in this order:

1. **WebView2 bootstrapper** — a refreshed Microsoft bootstrapper is the usual legitimate jump
   (+0.2–0.5 MB). Re-run `scripts/release/fetch-tools.ps1` and check `build-info.json`
   (`webview2.bytes`) against the previous build.
2. **Setup vs exe delta** — if the compressed setup grew but the uncompressed exe barely moved, the
   change is packaging: `installer.nsi`, added/renamed embedded files, compression settings.
3. **Application exe** — real code/dependency growth: new crates/Cargo features, newly bundled
   fonts/images/frontend assets, or a debug/unstripped profile instead of the release profile.
4. **Portable ZIP** — only fails its 25 MB guard when deflate is broken or extra files were staged;
   a portable ZIP above 8.5 MB is expected.

Do not raise `SETUP_LIMIT_BYTES` just to make a build pass: either shrink the regression or change
the published budget deliberately in the same commit (script constant + this document + the
acceptance notes).

## Install / uninstall behavior

- Per-user install to `%LOCALAPPDATA%\Programs\Hangline`; no UAC. Start-menu shortcut always, desktop
  shortcut optional. HKCU Uninstall entry, version/icon/quiet uninstall string.
- WebView2: an embedded **Evergreen bootstrapper** is bundled. If the runtime is missing it runs
  `/silent /install`; this requires **network** on a machine without WebView2. On Windows 11 the
  runtime is present, so normal installs are offline.
- Uninstall asks **keep or delete user data** (`%LOCALAPPDATA%\Hangline`); default/keep is safe.
  Silent uninstall keeps data unless `HANG_UNINSTALL_DATA=delete` (or `/S /DELETEUSERDATA`) is
  explicitly passed. Deleting is a deliberate choice; the app never removes screenshots otherwise.
- Upgrade: closes a running Hangline, replaces files, leaves user data and settings intact.
  Auto-update invokes the same setup with `/P /UPDATE /R` (passive, restart) and requires the new
  setup to carry a valid Tauri updater signature.

## Signing and keys

- **Authenticode:** releases up to v1.1.1 are unsigned (no certificate yet). Windows SmartScreen shows
  “unknown publisher”; README/release notes disclose it and SHA-256 lets users verify downloads.
  Hangline is preparing an application to the **SignPath Foundation** free OSS code signing program.
  The policy for reviewers and users is [`CODE_SIGNING_POLICY.md`](../CODE_SIGNING_POLICY.md). Until a
  SignPath signing policy is enabled and wired into the release workflow, the published binaries stay
  unsigned — never fake a signature.
- **Code signing policy (SignPath Foundation).** The public policy page is
  [`CODE_SIGNING_POLICY.md`](../CODE_SIGNING_POLICY.md) (linked from both READMEs); it carries the
  required statement *“Free code signing provided by SignPath.io, certificate by SignPath Foundation”*,
  the role/approver list, signing scope, build provenance and metadata rules, and the project's
  privacy policy. Supporting material for the application lives in `docs/release/`:
  [`signpath-application.md`](release/signpath-application.md) (answers, evidence, risk, submission
  steps), [`signpath/artifact-configuration.xml`](release/signpath/artifact-configuration.xml) (draft
  SignPath artifact configuration) and
  [`signpath/github-actions-signing.yml`](release/signpath/github-actions-signing.yml) (workflow
  template — **not** installed in `.github/workflows/` until the account/project exists).
  Open point to confirm with SignPath: the installer embeds Microsoft's WebView2 bootstrapper
  (proprietary upstream redistributable) — see the application pack's *Open compliance question*.
- **Updater signature:** `pnpm tauri signer generate -w .superpowers/release-keys/hangline.key`
  creates a minisign key pair; the public key lives in `src-tauri/tauri.conf.json`
  (`plugins.updater.pubkey`) and the private key/password stay in `.superpowers/release-keys/`
  (git-ignored). Back the key up: losing it means installed clients cannot verify future updates;
  leaking it means anyone can sign updates. CI uses repository secrets
  `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; the release script never
  prints them. The script signs with `--app-version 1.0.0`, so `requireSignedVersion` (if enabled
  later) binds the manifest version to the signature.
  The updater signature and Authenticode are independent: Authenticode changes the setup bytes, so
  once signing is enabled the updater signature must be computed **after** Authenticode over the
  final setup (ordering documented in `docs/release/signpath/github-actions-signing.yml`).
- `latest.json` carries the signature for both `windows-x86_64-nsis` and `windows-x86_64` (the
  updater probes the NSIS-specific key first, then the generic one) and points at the tagged
  release asset URL. The endpoint in `tauri.conf.json` is a placeholder
  (`github.com/devin2255/hangline`), matching the public repository.

## Publishing a release (operator)

1. Bump `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `Cargo.lock` to the
   same x.y.z; update `CHANGELOG.md`; run `pnpm check` and `cargo test`.
2. `node scripts/release/build-release.mjs --sign` (local key) and verify:
   - setup size ≤8,500,000 bytes (8.5 MB; see *Size budget*), SHA-256 in `SHA256SUMS.txt`;
   - `Get-AuthenticodeSignature` is `NotSigned` while SignPath signing is not yet enabled; after it is
     enabled the setup must report `Signed` with the SignPath Foundation certificate (see
     [`CODE_SIGNING_POLICY.md`](../CODE_SIGNING_POLICY.md));
   - `release/latest.json` has a real `dW50cnVzdGVk…` signature and the tagged URL.
3. Commit, `git tag -a v<version>`, push tag; `.github/workflows/release.yml` rebuilds with CI
   secrets, verifies tag == version, signs the manifest, and creates the GitHub Release with setup,
   portable, `latest.json` and `SHA256SUMS.txt`.
4. Download the published assets once and verify SHA-256 + a signed manifest entry. Publish release
   notes that disclose the unsigned installer and the WebView2 network requirement.

## Automatic updates

- Checks run manually (tray “Check for updates…”) or automatically once per 24 h when
  `autoCheckUpdates=true`; this is the **only** network behavior of the app (no telemetry/analytics).
- Checks never auto-download; the user explicitly chooses Install update. Updates are
  signature-verified, installed passively, and the app restarts.
- If the endpoint/repository is not published yet, the manual check fails with a readable notice
  and the app stays offline otherwise.

## Privacy / repository hygiene before a public release

- The public repository (`https://github.com/devin2255/hangline`) was published from a **fresh,
  squashed single-commit history**; the original development history (with early local paths) stays
  local and was not pushed. Historical tags v0.1.0-core…v0.4.0-settings were not pushed either;
  only the public `v1.0.0` tag exists.
- HEAD/current tree text is sanitized (`%USERPROFILE%`, `C:\Users\Example`); the committed evidence
  screenshots are app UI / neutral fixtures (no real desktop content). Keep future commits under the
  same rule: never commit `release/`, `.superpowers/`, `src-tauri/target/`, logs, keys/passwords, or
  user screenshots that are not neutral/app-UI only. If history is ever pushed again, re-check it.
- `LICENSE` (MIT) and `THIRD_PARTY_NOTICES.md` must ship with the release; Tendedero is credited as
  the design inspiration with its MIT + name/icon restriction.

## Rollback

- Keep the previous signed setup and `latest.json` in the GitHub Release assets. To roll back the
  update channel, publish a newer version whose manifest points at a fixed build; never reduce
  `version` (the updater ignores downgrades by default).
- For a bad release, unpublish/disable the release assets, publish a patched version, and post the
  SHA-256 of the working build. Existing installations remain on the last installed version.
