# Contributing to Meridian

Thanks for your interest in Meridian! Contributions of all kinds are welcome — bug reports, fixes, documentation, and features. This guide gets you from a clone to a passing pull request.

By participating, you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md).

---

## Ways to contribute

- **Report a bug** — open a [bug report](https://github.com/Meridiona/meridian/issues/new?template=bug_report.yml). Include your macOS version, what you expected, and what happened.
- **Request a feature** — open a [feature request](https://github.com/Meridiona/meridian/issues/new?template=feature_request.yml). Tell us the problem first, the solution second.
- **Improve docs** — typos, unclear steps, missing context. Docs PRs are always welcome and easy to review.
- **Fix or build** — grab an open issue (look for `good first issue`), or open one to discuss before large changes.

For anything beyond a small fix, **open an issue first** so we can align on the approach before you invest time.

---

## Project layout

Meridian is a monorepo. The major pieces:

| Path | What it is |
|---|---|
| `src/` | The Rust daemon — ETL pipeline, classification, coding-agent ingest, worklog drafting |
| `ui/` | The Next.js dashboard |
| `packages/meridian-mcp/` | The TypeScript MCP server |
| `tray/` | The Tauri menu-bar app |
| `services/` | Python services — the on-device MLX model server and the worklog synthesiser |
| `tests/` | Rust integration tests |

See **[CLAUDE.md](CLAUDE.md)** for the full architecture, the ETL state machine, and per-task recipes. It's the canonical engineering reference.

---

## Development setup

**Requirements:** macOS on Apple Silicon (M1+), Rust 1.93.1 (pinned via `rust-toolchain.toml`), Node 20+, Python 3.11.

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
cp .env.example .env
./install.sh
bash scripts/setup-hooks.sh   # install the git hooks — do this before your first commit
```

`./install.sh` builds the daemon and dashboard and registers the local services. For the iterative dev loop (`meridian dev`, hot-reload, rebuilding individual services), see [SETUP.md → Development](SETUP.md#development).

> **Install the git hooks.** `scripts/setup-hooks.sh` wires up the checks below. Without them your PR will fail CI on something the hooks would have caught locally.

---

## Before you open a pull request

Run the same checks CI runs. All must pass:

```bash
cargo fmt                       # format (CI checks --check)
cargo clippy -- -D warnings     # lint — warnings are errors
cargo test                      # Rust unit + integration tests
cargo build --release           # the daemon builds

cd ui && npm ci && npm run build   # the dashboard builds
```

The git hooks enforce most of this for you:

- **pre-commit** — `cargo fmt --check` + `cargo clippy -- -D warnings`
- **commit-msg** — Conventional Commits format
- **pre-push** — the full suite: fmt + clippy + `cargo test` + UI build + UI tests

**Never bypass the hooks** (`--no-verify`). If a hook fails, fix the cause — don't skip it.

### Touching the ETL pipeline, DB schema, or migrations?

Read **[TESTING.md](TESTING.md)** first. The integration tests in `tests/integration_etl.rs` encode invariants (session boundaries, gap detection, cursor advancement) that must keep passing. Add a test for any new behaviour.

---

## Commit and branch conventions

- **Branch per change.** Name it `type/short-description` — e.g. `feat/linear-sync`, `fix/timeline-gap`, `docs/setup-clarify`.
- **Conventional Commits** for messages: `type(scope): summary`. Common types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `style`.
  - e.g. `fix(etl): detect sleep gaps that span ETL run boundaries`
- **Never push to `main` directly.** Open a PR from your branch.

Releases are automated by release-please from the commit history, so accurate commit types matter.

### File-header convention

Every `.rs`, `.ts`, and `.tsx` file starts with this exact first line:

```
// meridian — normalises screenpipe activity into structured app sessions
```

SQL migrations use the `--` comment form. Python files in `services/agents/` use a `"""…"""` module docstring instead. The hooks expect these.

---

## Opening the pull request

1. Push your branch and open a PR against `main`.
2. Fill out the PR template — what changed, why, and how you tested it.
3. Make sure CI is green.
4. A maintainer will review. We may ask for changes; that's normal and not personal.

We **never** merge our own PRs without review, and we leave the final merge to a maintainer.

---

## Coding conventions (quick reference)

- **Rust** — `anyhow::Result` with `.context("…")` on DB calls; `tracing` with structured fields, not format strings; avoid `unwrap()` outside tests.
- **TypeScript** — no `any` unless justified with a comment; keep UI API routes thin (query, transform, return JSON).
- **SQL** — add a new numbered migration; never edit an existing one.
- **Keep files under ~500 lines** — split when they grow past that.

The deeper rationale for each lives in [CLAUDE.md](CLAUDE.md).

---

## Questions?

Open a [discussion or issue](https://github.com/Meridiona/meridian/issues), or reach out to **akarsh@meridiona.com**. We're happy to help you land your first contribution.
