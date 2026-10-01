# Meridian Release Runbook

Operational procedures for the packaged desktop app, currently focused on **rollbacks**.
Read alongside `docs/ci-cd.md` for the current CI/release architecture and
`CLAUDE.md` (§ "Make a DMG release mandatory") for the post-stable emergency update floor.

> **Note:** Meridian no longer ships a separate on-device model runtime. Since
> the Python/MLX removal, the only model Meridian downloads is the candle-based
> BGE embedder (used solely for the distiller's semantic dedup), which is fetched
> from HuggingFace on demand — there is no runtime release channel to roll back.
> Generation runs through the user's chosen CLI provider (`src/llm/`). This
> runbook therefore covers the **app/DMG channel only**.

---

## Background: how app updates flow

The app updates through the GitHub `latest` release channel, which the updater
compares **strictly forward** — this is the whole reason a rollback ships the
good code under a *higher* version rather than downgrading.

| Component | Channel | Compare rule | Downgrade installed clients? |
|---|---|---|---|
| **App** (`.app`/DMG) | GitHub `latest` release → `latest.json` | `tauri-plugin-updater` default: **strictly forward** (`new > current`) | **No** — must ship the old code under a *higher* version |

Key code:

- App updater: `tray/src-tauri/src/update.rs` — `updater.check()` with no custom
  `version_comparator`, so it is forward-only. A `latest.json` with a *lower*
  version returns "up to date" and nothing installs.
- App forced-install floor: `update.rs::enforce_minimum_version` +
  `.github/workflows/minimum-version.yml` (post-stable policy; edits only the
  latest release's `latest.json` notes).
- App release wiring: `.github/workflows/release-prepare.yml` creates an
  explicit RC candidate and dispatches `.github/workflows/release-build.yml`.
  The RC build signs/notarizes/packages and exact-artifact-smokes macOS + Windows.
  Stable promotion then reuses those exact proven binaries and rewrites only
  updater/release metadata. Stable does not rebuild the product.

---

## A. Roll back the app (behavior back, version forward)

Because the updater is forward-only, you roll back by shipping the **good code
under a higher version number**. semantic-release computes the bump from the
conventional-commit message.

### A1 — Normal rollback (consent-based)

```bash
git fetch origin
git checkout -b fix/rollback-<desc> origin/main

# Revert the bad change. For a squashed PR *merge* commit, use -m 1:
git revert -m 1 <bad-merge-sha>
#   (or plain commits:  git revert <sha1> <sha2> …)
# Keep the message conventional so semantic-release cuts a release, e.g.:
#   revert: <what and why>        → patch bump (moves the version forward)

git push -u origin fix/rollback-<desc>
gh pr create --base main --title "revert: <desc>" \
  --body "Rolls back <bad change>; ships the reverted code as a forward version."
```

Then: merge → wait for post-merge validation → cut and smoke a new RC → promote
that proven RC to stable. Stable promotion publishes a `latest.json` with a
**higher** version while reusing the RC's exact signed binaries, and installed
apps update normally (the in-app banner/card + tray-menu check).

### A2 — Forced rollback (evict the bad build within <=6 h)

Do A1 first and publish the rollback as a new stable version. After that stable
release is live and verified, open GitHub Actions -> **Set minimum supported
version**, choose `set`, and enter the rollback's stable version.

The workflow verifies that version is a published stable release, patches only
the current latest stable `latest.json`, and arms `Minimum-Version: X.Y.Z`.
Installed DMG apps below the floor force-install the latest stable release and
relaunch. No new binary or release is created.

To end forced enforcement later, run the same workflow with `clear`. Stable
promotion automatically carries an armed floor forward until it is cleared.

> **Invariant:** `main` is the single development trunk. Reverts and hotfixes go through a PR to `main`; releases are separate explicit RC/stable promotions. Minimum-version enforcement happens only after stable publication.

---
## Quick decision guide

| Situation | Action |
|---|---|
| Bad app build, not urgent | **A1** — revert → forward version |
| Bad app build, must evict now | **A2** — revert + `tray/minimum-version` floor |

## Post-rollback checklist

- [ ] App: confirmed `latest.json` on the GitHub `latest` release carries the
      higher (rollback) version, and `verify-release-bundle.sh` passed.
- [ ] App (forced): run **Set minimum supported version** with `clear` once
      forced enforcement is no longer required.
- [ ] Root cause captured (issue/ticket) so the reverted change can return
      safely.

---

## Scope notes

- These procedures affect the **DMG channel** only. npm/CLI installs update via
  `meridian update`.
- RC builds are channel-neutral final-version binaries. They must pass
  exact-artifact macOS and Windows smoke before stable promotion.
- There is no staging updater/runtime channel in the current release model.
