# Meridian Release Runbook

Operational procedures for the DMG app — currently focused on **rollbacks**.
Read alongside `CLAUDE.md` (§ "Make a DMG release mandatory").

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
  `scripts/package-updater.sh` (reads `tray/minimum-version`, bakes a
  `Minimum-Version:` line into `latest.json`'s notes).
- App release wiring: `.github/workflows/release.yml`'s `semantic-release` step
  runs `npx semantic-release`, whose `@semantic-release/exec` `prepareCmd`
  (`.releaserc.json`) chains `… && bash scripts/package-updater.sh <ver> && bash
  scripts/verify-release-bundle.sh <ver>`. `@semantic-release/github` then
  publishes `latest.json` as a release asset. `verify-release-bundle.sh` fails
  the release if `latest.json` is missing (guards a silent updater-sign no-op).

---

## A. Roll back the app (behavior back, version forward)

Because the updater is forward-only, you roll back by shipping the **good code
under a higher version number**. semantic-release computes the bump from the
conventional-commit message.

### A1 — Normal rollback (consent-based)

```bash
git fetch origin
git checkout -b fix/rollback-<desc> origin/pre-main

# Revert the bad change. For a squashed PR *merge* commit, use -m 1:
git revert -m 1 <bad-merge-sha>
#   (or plain commits:  git revert <sha1> <sha2> …)
# Keep the message conventional so semantic-release cuts a release, e.g.:
#   revert: <what and why>        → patch bump (moves the version forward)

git push -u origin fix/rollback-<desc>
gh pr create --base pre-main --title "revert: <desc>" \
  --body "Rolls back <bad change>; ships the reverted code as a forward version."
```

Then: merge → validate on staging → **a maintainer promotes `pre-main → main`**.
The `main` release publishes a `latest.json` with a **higher** version, and
installed apps update normally (the in-app banner/card + tray-menu check).

### A2 — Forced rollback (evict the bad build within ≤6 h)

Do everything in A1, **plus** arm the minimum-version floor in the *same* PR so
old apps force-install without waiting for a click:

```bash
# Floor = the version this rollback will ship as.
# A `revert:` is a patch bump, so: floor = <current released X.Y.Z, patch + 1>.
printf '1.71.1\n' > tray/minimum-version        # example value
git add tray/minimum-version
```

- `package-updater.sh` bakes `Minimum-Version:` into `latest.json`;
  `enforce_minimum_version` force-installs + relaunches (checked 30 s after
  launch, then every 6 h), with **no consent prompt**.
- `package-updater.sh` **fails the release loudly** if the floor exceeds the
  release version, so an over-set value can't slip through.
- **After it has propagated, empty `tray/minimum-version`** in a follow-up PR —
  the floor ships with **every** release while the file has content, so leaving
  it set keeps forcing later releases too.

> **Invariant:** keep `main ⊆ pre-main`. Do the revert on `pre-main` and
> promote. If a true emergency forces a direct `main` hotfix, back-merge the
> same revert to `pre-main` immediately so the invariant is restored.

---

## Quick decision guide

| Situation | Action |
|---|---|
| Bad app build, not urgent | **A1** — revert → forward version |
| Bad app build, must evict now | **A2** — revert + `tray/minimum-version` floor |

## Post-rollback checklist

- [ ] App: confirmed `latest.json` on the GitHub `latest` release carries the
      higher (rollback) version, and `verify-release-bundle.sh` passed.
- [ ] App (forced): `tray/minimum-version` emptied in a follow-up PR once the
      fleet has moved.
- [ ] Root cause captured (issue/ticket) so the reverted change can return
      safely.

---

## Scope notes

- These procedures affect the **DMG channel** only. npm/CLI installs update via
  `meridian update`.
- The staging channel (`pre-main` → staging DMG updater) mirrors production and
  should be exercised first when time allows.
