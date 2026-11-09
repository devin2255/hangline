# SignPath Foundation — application pack (Hangline)

Status: **prepared, not submitted.** No form has been filled in and no account has been registered
from this repository's tooling. The repository owner (`@devin2255`) submits when the go/no-go criteria
in §5 are met. Do not paste the answers below before that: see the risk assessment — the current
reputation signal is the main rejection risk, not the paperwork.

Related files:

- Public policy page (required by SignPath to exist *before* applying):
  [`../../CODE_SIGNING_POLICY.md`](../../CODE_SIGNING_POLICY.md)
- Artifact configuration draft: [`artifact-configuration.xml`](signpath/artifact-configuration.xml)
- GitHub Actions signing template (not active): [`signpath/github-actions-signing.yml`](signpath/github-actions-signing.yml)
- Release/update runbook: [`../RELEASE.md`](../RELEASE.md)

Program terms: <https://signpath.org/terms.html> (accessed 2026-11-09) — all quotes below are from
that page. Application form: <https://signpath.org/apply>.

---

## 1. Project facts

These are the values to keep consistent between the SignPath project record and the repository
(SignPath's public project pages show exactly this: handle, homepage, repository, license).

| Field | Value |
|---|---|
| Project name | **Hangline** (晾图绳) |
| SignPath project handle (suggested) | `hangline` |
| Homepage / repository | <https://github.com/devin2255/hangline> |
| Download / release page | <https://github.com/devin2255/hangline/releases> |
| License | MIT (`LICENSE`), no commercial dual-licensing, no closed component |
| Application type | Windows x64 desktop utility — screenshot clothesline (region capture, clipboard/system screenshot watching, markup editor, recording trim) |
| Stack | Tauri 2 (Rust) + React 19 + TypeScript + Vite; WebView2 runtime; NSIS installer |
| Current version | 1.1.1 prepared (`package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`); v1.1.0 is the latest public release |
| Public releases | v1.0.0, v1.1.0 — **both currently unsigned** |
| Artifacts per release | `Hangline_<v>_x64-setup.exe` (NSIS, per-user, ≤8.5 MB gate, baseline 7,315,272 B), `Hangline_<v>_x64-portable.zip` (~8.4 MB), `latest.json` (Tauri updater manifest, minisign-signed), `SHA256SUMS.txt` |
| Build system | GitHub Actions, `windows-latest`, triggered by `v*` tag; `.github/workflows/release.yml` |
| Maintainer / approver | `@devin2255` (single maintainer; holds all roles) |
| Policies | Code signing policy: `CODE_SIGNING_POLICY.md`; privacy: README §Privacy; third-party notices: `THIRD_PARTY_NOTICES.md` |
| Repository age / visibility | Public repository created **2026-11-05**; 0 stars at the time of writing (see §4) |

---

## 2. Form answers (draft)

The live form at <https://signpath.org/apply> is rendered client-side; its exact fields are not
publicly scrapeable. The answers below are written against the questions such applications ask in
practice (`/apply` and the terms' checklist: description, license, roles, privacy, MFA, build
verifiability). **Adjust field labels to the live form; do not add claims that are not in this file.**

### 2.1 Short description (one or two sentences)

> Hangline is a Windows screenshot tool with a different metaphor: screenshots are hung on a thin
> "clothesline" at the top of the screen, where they can be copied, dragged out, marked up or taken
> down. It is a local-only MIT-licensed Tauri 2 + Rust + React application, built from source by
> GitHub Actions, with one optional network request (the update check).

### 2.2 Why does the project need code signing?

> Hangline is distributed as a Windows installer that users download from GitHub Releases. Without
> an Authenticode signature, Windows SmartScreen shows an "unknown publisher" warning, and the
> project has no budget for a commercial code-signing certificate (single maintainer, no revenue, no
> commercial tier). A SignPath Foundation certificate would let users see a verified publisher and
> reduce the "is this a trojan?" friction that every unsigned screenshot utility faces.
> The build already produces consistent PE metadata (`ProductName`/`ProductVersion`/`FileVersion`) and
> a reproducible-ish CI build; the missing piece is only the signature.

### 2.3 How can the build be verified?

> - Source, build scripts and CI workflows are all public in the repository; there is no vendored
>   binary component and no local build in the release path.
> - Releases are produced by `.github/workflows/release.yml` on GitHub-hosted `windows-latest`
>   runners from a `v*` tag; the workflow refuses a tag that does not match `package.json` version.
> - Packaging is done by in-repo scripts: `scripts/release/build-release.mjs` (artifact assembly,
>   size gate, updater manifest), `scripts/release/installer.nsi` (NSIS installer with explicit
>   `VIProductVersion`/`VIAddVersionKey` metadata) and `scripts/release/fetch-tools.ps1` (pins the
>   official NSIS 3.13 toolchain and the Microsoft WebView2 bootstrapper, with SHA-256 recorded in
>   `build-info.json`).
> - The installer is built with `makensis` from the pinned toolchain; the application executable is
>   `cargo`-built from `src-tauri/`.
> - The project is prepared for the SignPath GitHub integration: artifact configuration at
>   `docs/release/signpath/artifact-configuration.xml` (root `<zip-file>`, PE metadata restrictions,
>   `authenticode-sign`), workflow template at `docs/release/signpath/github-actions-signing.yml`.
> - Every release will require a manual signing-request approval by the approver before anything is
>   published.

### 2.4 Who reviews and who approves releases?

> Single-maintainer project: `@devin2255` is the only person with write access, reviews every pull
> request, and approves every signing request manually. Full role table in
> `CODE_SIGNING_POLICY.md` §Team roles and members.
> MFA: **to be confirmed** by the maintainer before applying — the policy marks it as a TODO rather
> than claiming it is enabled (see §8 checklist).

### 2.5 What does the application do with user data? (privacy)

> The application is local-only. The only network request is the update check: it contacts the
> project's GitHub Releases endpoint, either manually from the tray or automatically once per 24 h
> when `autoCheckUpdates` is enabled (the user can turn it off; then the app makes zero requests).
> No accounts, telemetry, analytics, cloud sync or crash reporting. Screenshots and clipboard content
> are never uploaded; logs and state files stay under `%LOCALAPPDATA%\Hangline`.
> The policy page carries the program's fallback statement verbatim and links README §Privacy:
> > This program will not transfer any information to other networked systems unless specifically
> > requested by the user or the person installing or operating it.
> Runtime UI is rendered by Microsoft Edge WebView2 (Microsoft privacy statement linked in the
> policy); dependency licenses are in `THIRD_PARTY_NOTICES.md`.

### 2.6 Expected release frequency

> Currently two public releases (v1.0.0, v1.1.0) within the first two weeks; target cadence is roughly
> monthly, with patch releases as needed. Every release is built from a tag by CI and needs one manual
> approval.

### 2.7 Roles (for the SignPath project setup)

| Role | Member |
|---|---|
| Authors / committers | `@devin2255` |
| Reviewers | `@devin2255` (reviews all external PRs) |
| Approvers | `@devin2255` |

### 2.8 Anything else reviewers should know?

> - The installer embeds/executes Microsoft's WebView2 Evergreen bootstrapper; its proprietary status
>   is the one open compliance question — see §7 (it is being raised with SignPath before or with the
>   application).
> - The project is young and small (see §4). All the artifacts the program needs already exist:
>   policy page, release history, CI-built artifacts, metadata restrictions and a signing workflow
>   template.
> - Hangline is an independent implementation inspired by the macOS app Tendedero; no copied code or
>   art, and the Tendedero name/icon restriction is respected (`README` §License and attribution,
>   `THIRD_PARTY_NOTICES.md`).

---

## 3. Evidence checklist

| Item | State | Where |
|---|---|---|
| OSI-approved license, no dual licensing | ✅ MIT | `LICENSE` |
| Public repository with full source | ✅ | <https://github.com/devin2255/hangline> |
| Released in the form to be signed (≥1 public release) | ✅ 2 releases | <https://github.com/devin2255/hangline/releases> |
| CI build workflow (tag → artifacts) | ✅ | `.github/workflows/release.yml`, `.github/workflows/ci.yml` |
| Packaging scripts in-repo | ✅ | `scripts/release/build-release.mjs`, `installer.nsi`, `fetch-tools.ps1` |
| Artifact size + SHA-256 evidence | ✅ | release asset `SHA256SUMS.txt`; build log in `docs/acceptance/P5-core.md` §2 |
| PE metadata (ProductName/Version/FileVersion/CompanyName/…) | ✅ (see policy §Artifact metadata for the two small gaps) | `scripts/release/installer.nsi:109-115`; `src-tauri/tauri.conf.json` |
| Code signing policy on home page **and** download/release page | ✅ prepared (needs a push + a release to make the release-note link live) | `CODE_SIGNING_POLICY.md`; README §Code signing policy; `release.yml` release notes |
| Privacy policy | ✅ | README §Privacy; policy page |
| Uninstallation facility | ✅ NSIS uninstaller, keep/delete-data prompt | `docs/RELEASE.md` §Install/uninstall behavior |
| Updater signature (separate mechanism) documented | ✅ | `docs/RELEASE.md` §Signing and keys; `scripts/release/update-signing.md` |
| Artifact configuration | 🟡 draft, needs calibration against real artifacts | `docs/release/signpath/artifact-configuration.xml` |
| Signing workflow | 🟡 template only, not active | `docs/release/signpath/github-actions-signing.yml` |
| MFA on GitHub + SignPath accounts | ❓ **unknown / must be confirmed** | policy TODO |
| SignPath GitHub App installed | ❌ not yet (only needed at integration time) | <https://github.com/apps/signpath> |
| `build-info.json` attached to releases (provenance) | ❌ produced locally but not uploaded | recommendation: add it to the release assets |
| Project reputation (stars, age, external usage) | ⚠️ weak (see §4) | — |

---

## 4. Honest risk assessment: "verifiable reputation"

The terms are explicit that this is a gating criterion for applications that users *run*:

> "we cannot sign binaries based on source code that nobody knows. For executable programs that may be
> downloaded and executed based on our signature, we require a certain **verifiable reputation**. (Not
> for developer libraries/components/packages though.)"
> — <https://signpath.org/terms.html> §Common misunderstandings

Facts about this project at the time of writing:

- public repository created **2026-11-05** (about four days before this audit);
- **0 stars**, no external contributors, no third-party mentions known;
- **2 releases**, both unsigned, download counts not yet meaningful;
- single maintainer, no organisation backing;
- the category (screen capture utility) is a common malware disguise, which makes reviewers more
  cautious, not less.

For comparison, the program's public project list (<https://signpath.org/projects>) already contains
screenshot/capture tools (`flameshot`, `cap`, `Inkeys` — a screen annotation tool) and Tauri 2 + React
19 applications (`Floral Notepaper`, `Motrix Next`). Those projects have a long public history and
many stars; **the delta between Hangline and an accepted project in the same category is reputation,
not technology**. The technical prerequisites (CI-built artifacts, metadata, policy page) are already
in place or prepared.

SignPath also notes that it will not litigate the decision:

> "We're under no obligation to accept your project … We will listen to arguments about your specific
> situation, but we generally don't discuss policy."
> — <https://signpath.org/terms.html> §Common misunderstandings

There is no published threshold and **no documented penalty for re-applying after a rejection**, but a
premature rejection is a poor first data point for a later application.

---

## 5. Recommendation: apply now, or wait? — **Wait (do not submit yet)**

Two options, honestly weighed:

**A. Submit now (as soon as the checklist in §8 is done).**

- Pro: zero direct cost; the review takes time, and the project would be in the queue; a rejection does
  not obviously preclude re-applying; signing the very next release would remove SmartScreen friction
  earlier.
- Con: the reputation signal is at its weakest possible point (4-day-old repo, 0 stars, 2 releases).
  Per the terms, this is the one criterion explicitly aimed at executables, and the reviewer sees
  exactly that. The WebView2 proprietary-binary question (§7) is also still unresolved, which is a
  second, independent reason a first review could come back negative. Rejection is likely.

**B. Wait 4–6 weeks, accumulate verifiable signals, then submit.**

- Pro: every criterion in §8 can be satisfied; releases keep accumulating (e.g. v1.1.x/v1.2.0), the
  repository gains age, commit history and (hopefully) stars/downloads/issue activity — the kind of
  "verifiable reputation" the terms ask for. The WebView2 question can be answered by SignPath in
  parallel (§7), so the first application is already clean. Cost of waiting: SmartScreen keeps warning
  for a few more weeks, which the README already discloses and `SHA256SUMS.txt` mitigates.
- Con: delays the nice-to-have; a few weeks of users seeing "unknown publisher".

**Conclusion (explicit): do not submit now. Target submission after the preconditions below, i.e. no
earlier than ~4 weeks from now (first half of December 2026 at the earliest) and after at least one
more public release.** Concretely, flip to "submit" when *all* of these hold:

1. §8 checklist complete (MFA confirmed, policy page pushed, metadata follow-ups done, artifact
   configuration calibrated against a real artifact upload).
2. ≥3 public releases spanning ≥4 weeks of visible maintenance (v1.0.0, v1.1.0 + at least one more),
   with release notes and SHA-256.
3. At least one *external* verifiable usage signal — realistically: stars moving off 0 (the program does
   not publish a threshold; a low-double-digit count plus real download counts and issue activity is a
   reasonable heuristic, not a SignPath rule), or an established package/community channel.
4. Answer from SignPath on the WebView2 question (§7), or a decision to remove the embedded
   bootstrapper before applying.
5. The code signing policy page and its links are live on `main` and visible from the latest release
   notes (SignPath wants the policy on home **and** download/release pages).

If a scheduling reason forces an early submission, submit options ordered by expected value:
first resolve §7 (email), then submit; expect to be asked about reputation and have the answers in
§2.3/§2.4 ready.

---

## 6. Submission steps (for the repository owner)

### a) Before the form

1. Log in to <https://signpath.io> with the **GitHub account that owns the repository**
   (`@devin2255`) — the reviewer will verify repository control.
2. Make sure the checklist in §8 is complete on `main`; publish the policy page link in the latest
   release notes (the release.yml notes now include it).

### b) Fill the application (≈15 minutes)

- Use §1 facts table and §2 answers. Keep the description one or two sentences (see §2.1).
- Point at: repository, releases page, `CODE_SIGNING_POLICY.md`, `README` §Privacy, `LICENSE`.
- Answer the "third-party components" question with the WebView2 statement from §7 (do not hide it).
- Mention that the build is GitHub Actions based and that the artifact configuration/workflow template
  already exist in `docs/release/signpath/` (shows readiness).

### c) After approval — parameters and wiring

SignPath will provide (or create) three slugs; record them as repository variables:

| Value | Where it comes from | Used as |
|---|---|---|
| **Organization ID** | SignPath organization settings (UUID-like string) | `SIGNPATH_ORGANIZATION_ID` secret/var |
| **Project slug** | SignPath project (suggest e.g. `hangline`) | `SIGNPATH_PROJECT_SLUG` |
| **Signing policy slug** | The signing policy created for the project (e.g. `release-signing`) | `SIGNPATH_SIGNING_POLICY_SLUG` |
| Artifact configuration slug | Upload `docs/release/signpath/artifact-configuration.xml` in the project settings | `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` |

Also required:

- **Secrets:** `SIGNPATH_API_TOKEN` (submitter API token), plus the existing
  `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- **SignPath GitHub App** installed and granted access to the repository (for audit-log evaluation).
- Adopt `docs/release/signpath/github-actions-signing.yml` into `.github/workflows/` (it is a template;
  check the `Collect signed artifacts` step against what SignPath actually returns on the first run).
- First signed release: verify `Get-AuthenticodeSignature` reports `Valid` and the SignPath Foundation
  subject, and that the updater still installs (the minisign signature must be recomputed over the
  signed setup — the template enforces that order).
- Only then update `README`/`docs/RELEASE.md` from "planned" to "signed for release X".

---

## 7. Open compliance question: embedded WebView2 bootstrapper vs "No proprietary code"

**Facts.** `src-tauri/tauri.conf.json` sets `bundle.windows.webviewInstallMode = "embedBootstrapper"`.
The resulting NSIS setup embeds `MicrosoftEdgeWebView2Setup.exe` — Microsoft's Evergreen **bootstrapper**
(~1.9 MB, measured 1,897,688 B in `docs/acceptance/P5-core.md`) — and runs it `/silent /install` when the
WebView2 Runtime is missing. The bootstrapper is:

- Microsoft proprietary software (free to redistribute under Microsoft's terms, but **not open source**);
- Authenticode-signed by Microsoft (unchanged upstream bytes);
- not built from this repository, and the only non-OSS payload in the signed package.

**The clause.** <https://signpath.org/terms.html>:

> "**No proprietary code:** The project may not contain any proprietary, non open-source component …
> However, you may include **System Libraries** in signed packages. For a useful definition of System
> Libraries, see section 1 of the GPL v3 license."

Other relevant wording from the same page: upstream **OSS** binaries "may be included in signed
packages", and "we reserve the right to require that signed packages only include signed program and
library files in the future". A Microsoft bootstrapper is *signed* but *proprietary*, so neither
sentence settles it: the question is whether an OS-vendor installer/bootstrapper counts as a System
Library / permitted upstream component.

**Options (analysis only — the decision belongs to the maintainer, after SignPath answers):**

| Option | What changes | Pros | Cons / risks | Effort |
|---|---|---|---|---|
| **A. Stop embedding** — set `webviewInstallMode` to `downloadBootstrapper` (Tauri's default) | `src-tauri/tauri.conf.json`; rebuild; update `docs/RELEASE.md` and README wording | The signed artifact contains **no** proprietary bytes; cleanest compliance story; setup shrinks ~1.9 MB (more headroom under the 8.5 MB gate); matches the Tauri 2 precedent already accepted by SignPath (`Floral Notepaper` uses the default, no `webviewInstallMode`) | Machines **without** WebView2 **and** offline now fail during install instead of running the local bootstrapper first (note: with `embedBootstrapper` the runtime download *also* needs network, so the behavioural delta is small); the installer gains a download step at install time; install instructions/acceptance must be re-verified | ~1 hour config+docs, plus one acceptance pass |
| **B. Keep embedding, declare it** — argue System Library / upstream redistributable in the application | none (only wording in policy/application) | No engineering change; behaviour and docs stay as accepted | Depends on an interpretation SignPath has not confirmed; a reviewer may treat the bootstrapper as proprietary code → rejection or later constraints; SignPath says it does not discuss policy | ~0 |
| **C. `skip`** — require WebView2 preinstalled, embed nothing | `tauri.conf.json`; docs | Smallest artifact, no proprietary bytes | Worst UX on a fresh Windows 10 machine without WebView2; increases support burden; not recommended | ~1 hour + risk |
| **D. Ask SignPath first** (support@signpath.io) with the exact situation before applying | none | Authoritative answer, cheap, avoids a rejection on this ground; also signals diligence | Response time unknown | ~15 minutes |

**Draft question to SignPath (send before or with the application):**

> Subject: OSS application — question about an upstream Microsoft bootstrapper in the signed installer
>
> We are preparing an application for Hangline (MIT, GitHub: devin2255/hangline), a Tauri 2 Windows
> app. Our NSIS installer currently embeds Microsoft's WebView2 Evergreen bootstrapper
> (`MicrosoftEdgeWebView2Setup.exe`, ~1.9 MB, unchanged, Authenticode-signed by Microsoft) and runs it
> only when the WebView2 Runtime is missing. It is not built from our source and is not open source.
> Under the "No proprietary code" condition, is embedding that bootstrapper in the signed installer
> acceptable (e.g. as a System Library / upstream redistributable), or should we switch to
> `downloadBootstrapper` so the signed artifact contains no Microsoft binary? We can change it before
> submitting if that is preferred. Thank you.

**Explicitly not decided here.** The maintainer decides after SignPath's answer. If the answer is
negative (or absent at submission time), Option A is the low-cost path that removes the question
entirely; if the answer permits embedding, Option B is fine and no engineering work is needed. This
document only records the trade-offs.

---

## 8. Pre-submission checklist (repository work)

Blocking (do before filling the form):

- [ ] **MFA confirmed** on the GitHub account (and enabled on the future SignPath account) —
  `CODE_SIGNING_POLICY.md` keeps this as a TODO until the maintainer confirms.
- [ ] Push `CODE_SIGNING_POLICY.md` + the README/RELEASE.md links to `main` **before** applying
  (SignPath checks the live pages).
- [ ] Add `bundle.publisher = "Hangline"` and `bundle.copyright = "MIT License"` to
  `src-tauri/tauri.conf.json` (out of scope for the documentation task that produced this pack):
  today the portable `Hangline.exe` derives `CompanyName` `hangline` from the bundle identifier and has
  no `LegalCopyright`. Both signed binaries should report identical publisher metadata.
- [ ] Optional but recommended: add `VIAddVersionKey "OriginalFilename" …` to
  `scripts/release/installer.nsi` so the `original-filename` metadata restriction can be enforced.
- [ ] Attach `build-info.json` to releases (provenance: toolchain hashes, commit, sizes) — currently
  produced locally only.
- [ ] Upload the next release's artifacts once into the SignPath backend to calibrate
  `artifact-configuration.xml` (metadata values, nested `<zip-file>` handling).

After approval (integration):

- [ ] Configure `SIGNPATH_API_TOKEN`, `SIGNPATH_ORGANIZATION_ID`, the three slugs, install the SignPath
  GitHub App.
- [ ] Adopt `docs/release/signpath/github-actions-signing.yml` into `.github/workflows/`.
- [ ] First signed release: verify Authenticode (`Valid`, SignPath Foundation subject) and that the
  updater accepts the regenerated `latest.json`.
- [ ] Update README/RELEASE.md/policy from "planned" to "signed", with the first signed version number.

---

## 9. Sources

- SignPath Foundation terms (all quotes): <https://signpath.org/terms.html> — accessed 2026-11-09.
- Application entry point: <https://signpath.org/apply>; accepted project examples:
  <https://signpath.org/projects> (screenshot/annotation: flameshot, cap, Inkeys; Tauri 2: Floral
  Notepaper, Motrix Next).
- Artifact configuration syntax: <https://docs.signpath.io/artifact-configuration/syntax>;
  reference (metadata restrictions, `authenticode-sign`): <https://docs.signpath.io/artifact-configuration/reference>;
  examples: <https://docs.signpath.io/artifact-configuration/examples>.
- GitHub integration (upload-artifact ZIP requirement, hosted-runner requirement, action inputs):
  <https://docs.signpath.io/trusted-build-systems/github>; action:
  <https://github.com/signpath/github-action-submit-signing-request>.
- Structure reference used for the draft: <https://github.com/sylin-org/koi/blob/main/docs/release/signpath-artifact-configuration.xml>.
- Repository evidence: `README.md` (§Install, §Privacy, §Code signing policy), `docs/RELEASE.md`,
  `docs/acceptance/P5-core.md`, `.github/workflows/release.yml`, `scripts/release/*`.

---

## 10. Decision log

### 2026-11-09 — apply later, not now

**Decision (maintainer): wait 4–6 weeks before submitting the application.**

Rationale: every repository-side prerequisite is now in place, but SignPath requires *verifiable
reputation* for executable programs and the repository is four days old with 0 stars and two
releases. Applying today would most likely be rejected; re-applying later is allowed but wastes the
first impression.

**Re-apply window:** earliest **2026-12-07**, preferred **2026-12-21** (allowing 1.1.1 to be
published and one or two further releases to land).

**Go/no-go checklist before submitting (all must be true):**

- [ ] `v1.1.1` (or later) is published on GitHub Releases with real assets.
- [ ] At least **3 releases** across **≥ 4 weeks** of history.
- [ ] At least one externally visible signal (downloads, an issue/PR from a non-maintainer, a
      community post, or press coverage).
- [ ] Maintainer confirms **MFA** is enabled on the GitHub account (the policy page still carries
      `TODO(devin2255)`).
- [ ] Answer from SignPath on the **WebView2 bootstrapper** question, or a decision to ship the
      signed build with `downloadBootstrapper` instead of embedding it.
- [ ] Release workflow still builds both artifacts from source on GitHub-hosted runners and the
      8.5 MB gate is green.

**Open items (owner: maintainer):** MFA confirmation; send the WebView2 clarification e-mail
(draft in §7 of this document); after approval, set `SIGNPATH_API_TOKEN` /
`SIGNPATH_ORGANIZATION_ID` secrets and fill the three slugs in the workflow template.
