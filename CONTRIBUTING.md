# Contributing to Meridian

Thanks for your interest in contributing. This guide gets you from a clone to a passing pull request.

By participating you agree to our [Code of Conduct](CODE_OF_CONDUCT.md).

---

## Ways to contribute

- **Report a bug** — open a [bug report](https://github.com/Meridiona/meridian/issues/new?template=bug_report.yml). Include your macOS version, what you expected, and what happened.
- **Request a feature** — open a [feature request](https://github.com/Meridiona/meridian/issues/new?template=feature_request.yml). Describe the problem first, the solution second.
- **Improve docs** — typos, unclear steps, missing context. Always welcome.
- **Fix or build** — grab a `good first issue`, or open one to align on approach before large changes.

For anything beyond a small fix, **open an issue first**.

---

## Project layout

| Path | What it is |
|---|---|
| `src/` | Rust daemon — ETL pipeline, coding-agent ingest, classification, worklog drafting |
| `ui/` | Next.js dashboard |
| `packages/meridian-mcp/` | TypeScript MCP server |
| `tray/` | Tauri menu-bar app |
| `services/` | Python services — MLX model server and worklog synthesiser |
| `tests/` | Rust integration tests |

See **[CLAUDE.md](CLAUDE.md)** for the full architecture and per-task recipes.

---

## Development setup

**Requirements:** macOS on Apple Silicon (M1+), Rust 1.93.1 (pinned via `rust-toolchain.toml`), Node 20+, Python 3.11.

### First-time setup

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
cp .env.example .env
bash install-dev.sh          # builds deps, registers screenpipe + a11y-helper launchd agents
cargo install cargo-watch    # Rust file watcher (one-time)
bash scripts/setup-hooks.sh  # install git hooks — do this before your first commit
```

`install-dev.sh` installs everything but does **not** register the Rust daemon or MLX server as launchd agents — those run in watch mode instead.

### Starting the dev environment

```bash
bash dev-start.sh
```

This opens 4 Terminal windows, one per service:

| Window | Command | Triggers on |
|---|---|---|
| Rust daemon | `cargo watch -x 'run --bin meridian'` | any `.rs` save |
| MLX server | `uvicorn --reload --reload-dir services/agents/` | any `.py` save in `services/agents/` |
| Next.js UI | `npm run dev` | any `.ts`/`.tsx` save |
| Tauri tray | `npm run tauri dev` | any Rust or JS/CSS save |

screenpipe and a11y-helper run via launchd and restart automatically — you don't need to touch them.

### Stopping the dev environment

- **Ctrl-C** in each Terminal window to stop the watch processes
- `meridian stop` to stop the launchd agents (screenpipe, a11y-helper)

### Installed package vs dev mode

These are two distinct setups — do not mix them:

| | Installed package (`curl … | bash`) | Dev mode (`install-dev.sh`) |
|---|---|---|
| Rust daemon | launchd agent, release binary | `cargo watch` in terminal |
| MLX server | launchd agent | `uvicorn --reload` in terminal |
| Next.js UI | launchd agent, production build | `npm run dev` in terminal |
| Tauri tray | launchd agent, production build | `npm run tauri dev` in terminal |
| screenpipe | launchd agent | launchd agent |
| a11y-helper | launchd agent | launchd agent |

---

## Before you open a pull request

Run the same checks CI runs — all must pass:

```bash
cargo fmt                          # format
cargo clippy -- -D warnings        # lint — warnings are errors
cargo test                         # Rust unit + integration tests
cargo build --release              # verify release build

cd ui && npm ci && npm run build   # dashboard builds
```

The git hooks enforce most of this automatically:

- **pre-commit** — `cargo fmt --check` + `cargo clippy -- -D warnings`
- **commit-msg** — Conventional Commits format
- **pre-push** — full suite: fmt + clippy + `cargo test` + UI build + UI tests

**Never bypass the hooks** (`--no-verify`). Fix the cause, don't skip it.

### Touching ETL, DB schema, or migrations?

Read **[TESTING.md](TESTING.md)** first. The integration tests in `tests/integration_etl.rs` encode invariants (session boundaries, gap detection, cursor advancement). Add a test for any new behaviour.

---

## Commit and branch conventions

- **One branch per change.** Format: `type/short-description` — e.g. `feat/linear-sync`, `fix/timeline-gap`.
- **Conventional Commits:** `type(scope): summary` — e.g. `fix(etl): detect sleep gaps that span ETL run boundaries`.
  - Common types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`.
- **Never push to `main` directly.** Open a PR from your branch.
- **Do not mention Claude as co-author in commit messages.**

Releases are automated by release-please from the commit history, so accurate types matter.

### File-header convention

Every `.rs`, `.ts`, and `.tsx` file must start with:

```
//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
```

SQL migrations use the `--` comment form. Python files in `services/agents/` use a `"""…"""` module docstring. The hooks expect these.

---

## Opening the pull request

1. Push your branch and open a PR against `main`.
2. Fill out the PR template — what changed, why, and how you tested it.
3. Make sure CI is green.
4. A maintainer will review. We may ask for changes.

Maintainers never merge their own PRs. Leave the final merge to a reviewer.

---

## Coding conventions

- **Rust** — `anyhow::Result` with `.context("…")` on every DB call; `tracing` with structured fields; avoid `unwrap()` outside tests.
- **TypeScript** — no `any` unless justified with a comment; keep UI API routes thin (query, transform, return JSON).
- **SQL** — add a new numbered migration; never edit an existing one.
- **Keep files under ~500 lines** — split when they grow past that.

Full rationale in [CLAUDE.md](CLAUDE.md).

---

## Questions?

Open a [discussion or issue](https://github.com/Meridiona/meridian/issues) or email **akarsh@meridiona.com**.
