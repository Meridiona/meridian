# Documentation

Reference docs that don't need to sit in the repository root. Start with the
[README](../README.md) for what Meridian is, or [ARCHITECTURE.md](../ARCHITECTURE.md)
for how it works.

| Doc | Read it when |
|---|---|
| [privacy.md](privacy.md) | You want to know exactly what does and does not leave the machine. |
| [testing.md](testing.md) | **Required before touching ETL, the DB schema, or migrations.** It lists the invariants the integration tests enforce. |
| [notifications.md](notifications.md) | You're adding a toast or an interactive nudge - the outbox lifecycle, category registry, and the packaged-build test recipe. |
| [runbook.md](runbook.md) | Something shipped that shouldn't have. Operational procedures, currently focused on rollbacks. |
| [vision.md](vision.md) | You're making a product decision and need the principles behind it. |
| [images/](images/) | Repo images - the social card and README screenshots, with a spec for each. |

## What stays in the root, and why

The root is kept to files that either GitHub treats specially or that a first-time
visitor should not have to hunt for:

- `README.md`, `LICENSE`, `CHANGELOG.md` - conventional, and `LICENSE` **must** be at
  the root to be detected and included in clones.
- `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md` - GitHub community health
  files. It searches `.github/`, then the root, then `docs/`; the root keeps them
  visible in the file listing. `SUPPORT.md` lives in `.github/` alongside the issue
  templates, which GitHub requires there.
- `ARCHITECTURE.md` - the conventional entry point for someone reading the codebase.
- `SETUP.md` - user-facing install and configuration. It stays put because
  `.github/ISSUE_TEMPLATE/config.yml` publishes it as a `blob/main/SETUP.md` URL, and
  moving it would break a link already in circulation.
- `CLAUDE.md` - must be at the root; that is where Claude Code looks for it.

Everything else belongs here.
