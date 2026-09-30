# CI/CD architecture

## Pull requests: fast merge gate

Every pull request to `pre-main` or `main` runs `.github/workflows/ci.yml`.
It checks formatting, builds, lints, and tests the portable Rust crates, then
tests and type-checks the UI when relevant. It deliberately does not build a
DMG or Windows installer. The workflow has a three-minute timeout to protect the
three-minute feedback objective; a cache miss must be investigated rather than
quietly becoming the normal experience.

`CI` is the single branch-protection check to require for pull requests.

## Integration branches: platform validation

A push to `pre-main` or `main` runs `.github/workflows/main-validation.yml`.
This is the existing full macOS and Windows workspace matrix, plus release and
license policy checks. It detects platform-specific regressions after changes
have integrated, without charging every feature branch for both platform builds.

## Nightly: regression detection

`.github/workflows/nightly.yml` runs the complete workspace lint and test suite
on macOS on weekdays and can also be started manually. Add slow integration,
upgrade, and end-to-end tests here as they are created.

## Releases: artifact validation and publishing

`release-prepare.yml` decides the version and dispatches `release-build.yml`
from `main`. `release-build.yml` builds signed platform artifacts in parallel,
verifies them, notarizes the macOS deliverable, and publishes the release.
Release artifacts are therefore validated separately from source-level tests.
