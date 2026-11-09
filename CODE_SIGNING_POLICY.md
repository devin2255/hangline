# Code signing policy

> **Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).**

This is the code signing policy for **Hangline** (晾图绳), the Windows desktop application in this
repository (`https://github.com/devin2255/hangline`, MIT licensed). It states which artifacts are
signed, who is allowed to approve a signing request, how the signed binaries are traced back to this
source code, and what the application does with user data.

## Code signing policy — current status

Hangline is preparing an application to the **SignPath Foundation open-source code signing program**.
This page exists so that the repository satisfies the program's requirements *before* the first
signing request is submitted.

**Honest status: releases published so far are not Authenticode-signed.** Windows SmartScreen shows
an unknown publisher when such an installer is downloaded from the internet; users can verify a
download with `SHA256SUMS.txt`. The signing policy described below takes effect only once SignPath has
accepted the project and a signing policy has been configured; the first signed release will say so
explicitly in its release notes and in the README. Nothing in this repository claims that a past
release was signed.

## Signing scope

| Artifact | Authenticode (SignPath Foundation certificate) | Notes |
|---|---|---|
| `Hangline_<version>_x64-setup.exe` | **Yes**, once the signing policy is enabled | NSIS per-user installer; SHA-256 digest; metadata enforced by the artifact configuration |
| `Hangline.exe` **inside** `Hangline_<version>_x64-portable.zip` | **Yes**, once enabled | The portable ZIP's application executable is signed before the ZIP is packaged |
| `Hangline_<version>_x64-portable.zip` (the container) | **No** | ZIP archives have no Authenticode signature. The ZIP is covered by `SHA256SUMS.txt` only |
| `latest.json`, `SHA256SUMS.txt`, `build-info.json` | **No** | Data files, not executable code. `latest.json` carries a *Tauri updater* (minisign) signature instead, see below |
| `MicrosoftEdgeWebView2Setup.exe` (embedded in the installer) | Not signed by this project | Microsoft proprietary redistributable, Authenticode-signed by Microsoft; see *Third-party binaries* |

Two independent signature mechanisms exist in a Hangline release, and they are not interchangeable:

1. **Authenticode** (SignPath Foundation certificate) — proves the *publisher* of the `.exe` to
   Windows and SmartScreen. This is what this policy is about.
2. **Tauri updater signature** (minisign key pair, public key in `src-tauri/tauri.conf.json`,
   private key only in the maintainer's encrypted key store / GitHub Actions secrets) — proves that an
   *update payload* was produced by the project and binds it to a version. It is verified by the app
   before installing an update. It is not an Authenticode signature and does not make SmartScreen
   happy.

Signing a file with Authenticode changes its bytes. Therefore, when both are used, the Authenticode
signature is applied **first**, and the updater signature is computed over the final, signed setup.
This ordering is documented in [`docs/release/signpath/github-actions-signing.yml`](docs/release/signpath/github-actions-signing.yml).

## Build provenance

Signed binaries are built from this repository only, by GitHub-hosted runners, with no local build in
the signing path:

- **Source:** `https://github.com/devin2255/hangline` — all application source (Rust in `src-tauri/`,
  TypeScript/React in `src/`) and all build scripts are in the repository and MIT licensed.
- **Build system:** GitHub Actions, `windows-latest` runners. The signed job is `.github/workflows/release.yml`,
  triggered by a `v*` tag; the same build also runs on every push/PR as `.github/workflows/ci.yml`.
- **Packaging scripts (in-repo, reviewed like source):** `scripts/release/build-release.mjs` (artifact
  assembly, size gate, manifest), `scripts/release/installer.nsi` (NSIS installer), and
  `scripts/release/fetch-tools.ps1` (pins and SHA-256-records the official NSIS 3.13 toolchain and the
  Microsoft WebView2 bootstrapper; versions/hashes are recorded in `build-info.json`).
- **Version binding:** the release workflow refuses to build unless the pushed tag equals the
  `package.json` version.
- **Approval gate:** every release requires a **manual signing-request approval** by the approver
  listed below. The release workflow uploads the unsigned artifact to SignPath, waits for the
  approval, downloads the signed artifact, and only then computes `SHA256SUMS.txt`, the updater
  manifest and the GitHub Release. A build that is not approved is never published.
- **No out-of-band binaries:** release assets are produced by that workflow from the tagged commit;
  locally built installers are for testing only and are git-ignored (`release/`).

## Artifact metadata

SignPath requires that signed binaries carry consistent product metadata and that the signing
configuration *enforces* it. Hangline's release binaries carry:

| Attribute | `Hangline_<version>_x64-setup.exe` | `Hangline.exe` (portable) |
|---|---|---|
| `ProductName` | `Hangline` | `Hangline` |
| `ProductVersion` | `<version>` (three-part, same value every build) | `<version>` |
| `FileVersion` | `<version>` | `<version>` |
| `CompanyName` | `Hangline` | `hangline` (derived from the bundle identifier; see note) |
| `LegalCopyright` | `MIT License` | not currently set |
| `FileDescription` | `Hangline Setup` | `Hangline` |

The installer metadata is set explicitly in `scripts/release/installer.nsi` (`VIProductVersion` and the
`VIAddVersionKey` block, lines 109–115); the application executable's metadata comes from
`src-tauri/tauri.conf.json` (`productName`, `version`) and from the `tauri-build` resource generator.
The planned enforcement (allowed `product-name`, `product-version`, `file-version`, `company-name`)
is the draft artifact configuration in
[`docs/release/signpath/artifact-configuration.xml`](docs/release/signpath/artifact-configuration.xml),
which is imported into SignPath and calibrated before the first signing request.

**Known cosmetic inconsistency:** the two binaries currently report `CompanyName` with different letter
case (`Hangline` vs `hangline` — the app executable derives it from the bundle identifier
`app.hangline.Hangline`), and the portable executable has no `LegalCopyright`. Both binaries belong to
the same project and report the same product version; nevertheless, setting `bundle.publisher` and
`bundle.copyright` in `src-tauri/tauri.conf.json` is planned so that both carry identical values before
the first signed release. Until then the artifact configuration only enforces attributes that are
actually present in the binaries.

## Team roles and members

Hangline is a **single-maintainer project**. The maintainer holds all roles below; there are no other
committers, reviewers or approvers today.

| SignPath role | Member(s) | Notes |
|---|---|---|
| Authors / committers | `@devin2255` | Only account with write access to the repository |
| Committers and reviewers | `@devin2255` | Every pull request from a non-committer is reviewed by the maintainer; direct pushes are the maintainer's own reviewed changes |
| Approvers | `@devin2255` | Each signing request is approved manually, per release |

- **MFA:** `TODO(devin2255): confirm and enable multi-factor authentication on the GitHub account and
  on the SignPath account before the first signing request.` This document deliberately does **not**
  assert that MFA is already enabled — only the maintainer can confirm it, and SignPath's terms require
  it for all team members.
- The role assignments above must be kept in sync with reality; if a second maintainer joins, this
  table and the SignPath team/role settings are updated in the same change.

## Privacy policy

The application's privacy behavior is documented in the README's **Privacy** section:
[`README.md` §Privacy](README.md#privacy) (Chinese: [`README.zh-Hans.md` §隐私](README.zh-Hans.md#隐私)).

In short, and as the program's fallback statement requires:

> This program will not transfer any information to other networked systems unless specifically
> requested by the user or the person installing or operating it.

- The **only** network request the app makes is the update check: the user can trigger it manually from
  the tray, or let it run automatically once per 24 h when `autoCheckUpdates` is enabled (default
  behavior can be turned off; with it off and no manual click the app performs zero requests). It
  contacts the project's GitHub Releases endpoint (`github.com/devin2255/hangline`) and nothing else.
- No accounts, no telemetry, no analytics, no cloud sync, no crash reporting.
- Screenshots and recordings stay on the user's disk; the app never uploads image or clipboard content.
- Local logs and state files (paths, event metadata, no image data) stay on the user's machine.
- Opening a link in the default browser only happens when the user chooses it.

**Third-party components and their privacy policies.** Hangline is built with open-source components
(Rust crates, React, Tauri) listed in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). At runtime
the UI is rendered by **Microsoft Edge WebView2**, a Microsoft component with its own privacy
statement (https://privacy.microsoft.com/privacystatement). The installer may run Microsoft's WebView2
bootstrapper to install the runtime; that interaction is between Microsoft's software and the user's
machine.

## Third-party binaries in signed packages

The installer embeds `MicrosoftEdgeWebView2Setup.exe`, Microsoft's **Evergreen bootstrapper** for the
WebView2 Runtime (~1.9 MB). It is:

- **not built from this project's source** and not covered by this project's MIT license;
- a Microsoft redistributable, Authenticode-signed by Microsoft, included unchanged and only executed
  to install the WebView2 Runtime on machines that lack it;
- recorded with its SHA-256 in `build-info.json` so the exact upstream bytes are auditable.

SignPath's OSS terms permit including *System Libraries*/upstream binaries in signed packages, but the
"no proprietary code" clause and the exact status of a Microsoft bootstrapper is something this project
intends to **confirm with SignPath** rather than assume. The alternatives (not embedding the bootstrapper
at all, or shipping a different WebView2 install mode) are analyzed in
[`docs/release/signpath-application.md`](docs/release/signpath-application.md) §*Open compliance
question*; the decision will be recorded there before the first signing request.

## Conditions of use (why this project qualifies)

For reviewers, the program's conditions and where they are met:

| SignPath condition | Hangline |
|---|---|
| No malware / no potentially unwanted programs | Local-only screenshot tool; no installers beyond NSIS; no bundled adware |
| OSI-approved OSS license, no commercial dual-licensing | MIT for the whole repository (`LICENSE`); no paid tier, no closed component |
| No proprietary code | All application code is MIT and in-repo; the only non-OSS payload is the upstream Microsoft WebView2 bootstrapper described above (verification with SignPath pending) |
| Maintained | Active development; v1.0.0 and v1.1.0 released, v1.1.1 prepared at the time of writing; CI gates on every push |
| Released in the form that should be signed | Public releases exist with the exact artifact types (`setup.exe`, `portable.zip`) that are to be signed |
| Documented | Feature and usage documentation in `README.md` / `README.zh-Hans.md`; runbook in `docs/RELEASE.md` |
| Respects privacy | See *Privacy policy*; no telemetry; the single optional network request is documented |
| Announces system changes | No admin rights; writes only HKCU (uninstall entry, optional Run key when the user enables autostart) and its own `%LOCALAPPDATA%\Hangline` data directory |
| Provides uninstallation | NSIS uninstaller; keep/delete user data prompt (default keep) |
| No hacking tools | Not applicable — a screenshot/clothesline utility |
| MFA + role assignment | Roles above; MFA confirmation is the open TODO for the maintainer |

## Related documents

- Application pack (ready-to-submit answers, evidence, risk assessment, submission steps):
  [`docs/release/signpath-application.md`](docs/release/signpath-application.md)
- Artifact configuration draft: [`docs/release/signpath/artifact-configuration.xml`](docs/release/signpath/artifact-configuration.xml)
- GitHub Actions signing template (not active yet): [`docs/release/signpath/github-actions-signing.yml`](docs/release/signpath/github-actions-signing.yml)
- Release/update/signing runbook: [`docs/RELEASE.md`](docs/RELEASE.md)

## Contact

Maintainer: [@devin2255](https://github.com/devin2255) — via
[GitHub Issues](https://github.com/devin2255/hangline/issues) (security reports: please use a private
advisory / the repository's security contact instead of a public issue).

_Last reviewed: 2026-11-09. This page is updated when the signing state changes, when roles change, or
when the artifact configuration changes._
