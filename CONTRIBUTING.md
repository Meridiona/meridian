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
| `meridian-core/` | Shared Rust crate — DB readers used by both the daemon and the tray |
| `ui/` | Next.js dashboard (static export, embedded in the Tauri binary — no Node server) |
| `packages/meridian-mcp/` | TypeScript MCP server |
| `tray/` | Tauri menu-bar app — in-process capture, dashboard webview, MLX supervision |
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
bash install-dev.sh          # builds deps, registers OpenObserve agent
cargo install cargo-watch    # Rust file watcher (one-time)
bash scripts/setup-hooks.sh  # install git hooks — do this before your first commit
```

`install-dev.sh` builds all deps but does **not** register the daemon, MLX server, or tray as launchd agents — those run in watch mode instead. Capture runs in-process inside the Tauri tray; no screenpipe or a11y-helper agent is needed.

> **Upgrading from an older dev setup?** If you have screenpipe or a11y-helper registered from before v1.64.0, remove them:
>
> ```bash
> launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.screenpipe.plist
> launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.a11y-helper.plist
> ```
>

### Starting the dev environment

```bash
bash dev-start.sh
```

This opens **3 Terminal windows**:

| Window | Command | Triggers on |
|---|---|---|
| Rust daemon | `cargo watch -x 'run --bin meridian'` | any `.rs` save |
| MLX server | `uvicorn --reload --reload-dir services/agents/` | any `.py` save in `services/agents/` |
| Tauri tray | `npm run tauri dev` | any Rust or frontend save |

`npm run tauri dev` automatically starts the Next.js dev server on port 3939 (via `beforeDevCommand` in `tray/src-tauri/tauri.conf.json`) and opens the dashboard in a native Tauri webview — no separate `npm run dev` terminal is needed. The `beforeDevCommand` also copies `tray/src/` into `ui/public/popover/` so the popover window resolves correctly in dev mode.

### Stopping the dev environment

**Ctrl-C** in each Terminal window. `dev-start.sh` also kills any previous run at start, so re-running it is safe.

### Installed app vs dev mode

| | Installed app (`.dmg` / `bootstrap.sh`) | Dev mode (`install-dev.sh`) |
|---|---|---|
| Rust daemon | launchd agent, release binary | `cargo watch` in terminal |
| MLX server | launchd agent, provisioned runtime | `uvicorn --reload` in terminal |
| Dashboard | static export embedded in tray binary | Next.js dev server (auto-started by `tauri dev`) |
| Tauri tray | installed `.app` | `npm run tauri dev` in terminal |
| Capture | in-process inside tray | in-process inside tray |

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
- **TypeScript** — no `any` unless justified with a comment. The dashboard reaches Rust only via `ui/lib/bridge.ts` (`load`/`mutate` → `invoke`; `subscribe` → Tauri events). No `/api` fetch or `EventSource` — those routes are gone.
- **SQL** — add a new numbered migration; never edit an existing one.
- **Keep files under ~500 lines** — split when they grow past that.

Full rationale in [CLAUDE.md](CLAUDE.md).

---

## Questions?

Open a [discussion or issue](https://github.com/Meridiona/meridian/issues) or email **akarsh@meridiona.com**.
