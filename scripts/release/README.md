# Hangline release pipeline (P5)

Local + CI release tooling. Artifacts are produced under `release/` (git-ignored); nothing is
published automatically from a developer machine.

## Prerequisites

- Windows 11/10 x64, Node 22 + pnpm 10, Rust 1.90 MSVC.
- NSIS 3.13 + WebView2 bootstrapper: `scripts/release/fetch-tools.ps1` downloads and caches them
  under `src-tauri/target/p5-tools/` (official Tauri binary-releases NSIS zip + Microsoft fwlink).
- Updater signing key: `pnpm tauri signer generate -w .superpowers/release-keys/hangline.key`
  (password in `.superpowers/release-keys/password.txt`). Both are git-ignored. The public key is
  in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). **Losing the private key blocks
  updates for installed users**; back it up in a password manager and store the CI secret pair.

## Local build (unsigned layout check)

```powershell
node scripts/release/build-release.mjs --skip-build --skip-check --skip-tools
```

Produces `Hangline_<version>_x64-setup.exe`, `Hangline_<version>_x64-portable.zip`,
`SHA256SUMS.txt`, `build-info.json` and a **layout-only** `latest.json` (clearly marked
`UNSIGNED-PLACEHOLDER`). The setup size gate is 8,500,000 bytes (W0.6: tightened from
15,000,000; see `docs/RELEASE.md` §"Size budget").

## Local signed build

```powershell
node scripts/release/build-release.mjs --sign
# or fast iteration:
node scripts/release/build-release.mjs --skip-build --skip-check --skip-tools --sign
```

`latest.json` then contains a real Tauri updater signature for both `windows-x86_64-nsis` and
`windows-x86_64`, pointing at
`https://github.com/devin2255/hangline/releases/download/v<version>/Hangline_<version>_x64-setup.exe`.
The repo name is a placeholder until the user publishes their GitHub repository; update
`src-tauri/tauri.conf.json` `plugins.updater.endpoints` and the URL template in
`build-release.mjs` when renaming. Authenticode signing is NOT included (V1 ships unsigned; see
README/SmartScreen notes). The updater signature is independent of Authenticode.

## CI

- `.github/workflows/ci.yml`: every push/PR runs `pnpm check`, `cargo test`, `pnpm build` (release
  instrumentation gate) and an **unsigned** `build-release.mjs --ci`, asserting the installer
  exists and is ≤8.5 MB; artifacts are uploaded as `hangline-windows-unsigned`.
- `.github/workflows/release.yml`: on a `v*` tag it verifies tag == `package.json` version, runs
  the gates, builds with `--ci --sign` using repository secrets
  `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, verifies the signed
  manifest/URL/size, then creates a GitHub Release with setup + portable + `latest.json` +
  `SHA256SUMS.txt` via `gh release create`.

Required repository secrets: `TAURI_SIGNING_PRIVATE_KEY` (key file content) and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. No other secrets are needed; the unsigned setup is uploaded
as-is.

## Version/tag

1. Bump `package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml` (and `Cargo.lock`)
   to the same x.y.z (scripts/version.mjs checks).
2. Update `CHANGELOG.md`, run the gates and a signed build locally.
3. Commit, create an annotated tag `v<version>`, push tag → release.yml builds/publishes.
4. Verify the Release assets, SHA-256 and that `latest.json` points at the tagged setup.

See `docs/RELEASE.md` for signing/privacy/rollback/versioning details and SmartScreen notes.
