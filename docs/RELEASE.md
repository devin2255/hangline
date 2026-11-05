# Release, update and signing notes (P5 / v1.0.0)

Hangline 1.0.0 is a **source release**. The repository builds a per-user installer and a portable
ZIP locally; nothing is uploaded by a developer machine. This document is the reviewer/operator
runbook for producing and publishing a build, and records the release constraints.

## Artifacts (v1.0.0, built by scripts/release/build-release.mjs)

| Artifact | Expectation |
|---|---|
| `Hangline_1.0.0_x64-setup.exe` | per-user NSIS installer, ≤15 MB (actual 7,315,272 B), Authenticode **NotSigned** |
| `Hangline_1.0.0_x64-portable.zip` | `Hangline.exe`, `Hangline.ico`, empty `portable` marker, bilingual README |
| `SHA256SUMS.txt` | SHA-256 of the two artifacts |
| `latest.json` | Tauri v2 updater manifest; signed with the updater key (never the Authenticode key) |
| `build-info.json` | build provenance (versions, tool hashes, sizes, signature source; no key material) |

All artifacts are git-ignored under `release/`.

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

- **Authenticode:** V1 is unsigned (no commercial certificate). Windows SmartScreen shows
  “unknown publisher”; README/release notes disclose it and SHA-256 lets users verify downloads.
  Signing with SignPath/Azure Trusted Signing is a post-V1 option; never fake a signature.
- **Updater signature:** `pnpm tauri signer generate -w .superpowers/release-keys/hangline.key`
  creates a minisign key pair; the public key lives in `src-tauri/tauri.conf.json`
  (`plugins.updater.pubkey`) and the private key/password stay in `.superpowers/release-keys/`
  (git-ignored). Back the key up: losing it means installed clients cannot verify future updates;
  leaking it means anyone can sign updates. CI uses repository secrets
  `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; the release script never
  prints them. The script signs with `--app-version 1.0.0`, so `requireSignedVersion` (if enabled
  later) binds the manifest version to the signature.
- `latest.json` carries the signature for both `windows-x86_64-nsis` and `windows-x86_64` (the
  updater probes the NSIS-specific key first, then the generic one) and points at the tagged
  release asset URL. The endpoint in `tauri.conf.json` is a placeholder
  (`github.com/hangline-app/hangline`); rename it when the public repository is created.

## Publishing a release (operator)

1. Bump `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `Cargo.lock` to the
   same x.y.z; update `CHANGELOG.md`; run `pnpm check` and `cargo test`.
2. `node scripts/release/build-release.mjs --sign` (local key) and verify:
   - setup size ≤15,000,000 bytes, SHA-256 in `SHA256SUMS.txt`;
   - `Get-AuthenticodeSignature` is `NotSigned` (expected);
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
