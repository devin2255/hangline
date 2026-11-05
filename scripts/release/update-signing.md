# Updater signing and release flow (P5-2 handoff to P5-3)

Hangline uses `tauri-plugin-updater` 2 with the Tauri/minisign updater signature. The updater
endpoint is `https://github.com/hangline-app/hangline/releases/latest/download/latest.json`
(placeholder repository until the user publishes; see `docs/superpowers/plans/2026-11-04-hangline-windows-p5-release.md`).

This document is the signing contract for `scripts/release/build-release.mjs` (P5-1) and the
release workflow (P5-3). The private key is **never** committed, printed in CI logs, or copied into
an artifact.

## 1. Keypair generation (already done for this checkout)

The keypair was generated interactively with the official CLI (the command prompts for the password; see below):

```powershell
# Interactive: the CLI prompts for the password twice. Do NOT pass -p on the command line and do
# NOT use --ci here (--ci skips prompting and creates an empty-password key, not the documented
# encrypted key). Store the same password in password.txt / the CI secret.
pnpm tauri signer generate -w .superpowers/release-keys/hangline.key
```

Outputs:

| File                                         | Contents                                           | Location                |
| -------------------------------------------- | -------------------------------------------------- | ----------------------- |
| `.superpowers/release-keys/hangline.key`     | encrypted private key (password in `password.txt`) | local only, git-ignored |
| `.superpowers/release-keys/hangline.key.pub` | base64 minisign public key, one line               | public                  |
| `.superpowers/release-keys/password.txt`     | the private-key password                           | local only, git-ignored |

`.gitignore` ignores both `.superpowers/` and the explicit `.superpowers/release-keys/` rule.
Verify with `git check-ignore -v .superpowers/release-keys/hangline.key`.

> Losing the private key or password makes existing installations unable to accept future updates
> (the public key is pinned in the shipped config). Back the key up offline in a password manager /
> encrypted vault. To rotate, ship a release signed with the old key that changes the pinned public
> key, then switch the CI secret.

## 2. Public key in the app configuration

`src-tauri/tauri.conf.json`:

```json
"plugins": {
  "updater": {
    "endpoints": ["https://github.com/hangline-app/hangline/releases/latest/download/latest.json"],
    "pubkey": "<contents of .superpowers/release-keys/hangline.key.pub>",
    "windows": { "installMode": "passive" }
  }
}
```

The value is the **entire one-line content** of the `.pub` file (base64; not a path, not the
`untrusted comment:` text alone). The public key may be committed; the private key may not.

No `updater:default` capability is granted: the frontend calls Hangline's own
`check_updates` / `install_update` / `get_pending_update` commands, and the Rust side owns the
workflow. The plugin's JS commands stay unreachable.

## 3. Signing the update artifact (P5-3)

The updater signature covers the **installer artifact**, not `latest.json`. The plugin verifies it
with the pinned public key before launching the installer.

```powershell
# Sign the built setup (the .sig file is written next to it; its content is the latest.json
# `platforms."windows-x86_64".signature` value).
$env:TAURI_SIGNING_PRIVATE_KEY_PATH = "$PWD\.superpowers\release-keys\hangline.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "<read from your password manager, never commit>"
pnpm tauri signer sign --app-version <version> release/Hangline_<version>_x64-setup.exe
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PATH, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

Then `release/latest.json` (Tauri v2 static-manifest shape):

```json
{
  "version": "<version>",
  "notes": "…",
  "pub_date": "…",
  "platforms": {
    "windows-x86_64": {
      "signature": "<contents of Hangline_<version>_x64-setup.exe.sig>",
      "url": "https://github.com/hangline-app/hangline/releases/download/v<version>/Hangline_<version>_x64-setup.exe"
    }
  }
}
```

`build-release.mjs` writes a **layout-only** `release/latest.json`
(`"signature": "UNSIGNED-PLACEHOLDER"`, placeholder URL) unless `--sign` is passed; with `--sign`
(or the CI `TAURI_SIGNING_PRIVATE_KEY*` secrets) it writes the real signature and the tagged
GitHub-release asset URL for both `windows-x86_64-nsis` and `windows-x86_64`. Never publish the
placeholder file: the plugin parses it but signature verification fails during install, and the
download would 404.

Use `--app-version <version>` when signing by hand. `tauri build` records it automatically, but the
custom NSIS pipeline of P5-1 signs after packaging, so the flag is required to keep
`requireSignedVersion` usable later. The shipped config leaves `requireSignedVersion` at its default
(`false`) for the first release; turning it on later requires re-signing every still-supported
artifact with `--app-version`.

## 4. CI secrets (P5-3)

| Secret                               | Value                                                                                                                 |
| ------------------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| `TAURI_SIGNING_PRIVATE_KEY`          | literal contents of `hangline.key` (or use `TAURI_SIGNING_PRIVATE_KEY_PATH` with a file materialised from the secret) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | contents of `password.txt`                                                                                            |

The workflow must not `echo` either secret, must sign only `release/Hangline_*_x64-setup.exe`, and
must upload `latest.json` + the setup asset in the same GitHub release (`releases/latest/download`
must resolve to the newest `latest.json`). The private key must never be uploaded as a workflow
artifact or cached between runs.

## 5. Local-only verification (no public repository needed)

`node scripts/release/update-check-test.mjs` serves a localhost `latest.json` and starts the
**debug** build with `HANG_UPDATE_ENDPOINT` (compiled into debug builds only, ignored by release
builds). It proves the automatic/manual gating, the `update-available` event/notice and the
`lastUpdateCheck` persistence without a real release. The fixture signature is intentionally fake,
so download/install/restart is not exercised; that remains CI/manual-pending (a real signed build
over an installed old version).

## 6. Manual/CI checklist for a real update

1. Build and sign a newer version; host `latest.json` + the setup asset (GitHub Release).
2. Install the older build in a clean VM; keep its settings and a card on the line.
3. Trigger the tray manual check (and/or wait for the daily startup check); confirm the panel
   offers _Install update_ and the version/notes.
4. Click install; confirm the passive installer verifies the signature, replaces the app, restarts
   it, and that settings/cards survive. A failed/unsigned artifact must show _Update failed_ and
   leave the old version installed and runnable.
5. Record the result in the P5 acceptance evidence; never claim it before it was run.
