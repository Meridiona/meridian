# Getting help

## Something isn't working

Start with the built-in diagnostics - they resolve most issues without anyone else
being involved:

```bash
meridian doctor    # checks config, services, and permissions
meridian status    # what's actually running
meridian logs -f   # watch the pipeline live
```

If `meridian doctor` reports a problem, its output usually names the fix.

## Still stuck

| I want to... | Go here |
|---|---|
| Report a bug | [Open a bug report](https://github.com/Meridiona/meridian/issues/new?template=bug_report.yml) |
| Request a feature | [Open a feature request](https://github.com/Meridiona/meridian/issues/new?template=feature_request.yml) |
| Ask a question, or share how you use it | [Discussions](https://github.com/Meridiona/meridian/discussions) |
| Report a security vulnerability | **Do not open an issue** - see [SECURITY.md](../SECURITY.md) |
| Ask about privacy or data handling | [docs/privacy.md](../docs/privacy.md), or email akarsh@meridiona.com |

## What makes a bug report easy to fix

- Your OS and version, and the Meridian version (Settings → Account, or `meridian --version`).
- What you expected, and what happened instead.
- Whether it started after an update.
- Relevant output from `meridian doctor` and `meridian logs`.

If the problem is hard to describe, **Settings → Account → Export Diagnostics** produces
a `.tar.gz` of local logs you can attach. Look through it before sending - it contains
your own log output. Quote your **Support ID** (also in Settings → Account) if you have
error reporting enabled, so we can match your report to what your machine reported.

## Documentation

- [README](../README.md) - what Meridian is and how to install it
- [SETUP.md](../SETUP.md) - permissions, tracker setup, configuration, troubleshooting
- [ARCHITECTURE.md](../ARCHITECTURE.md) - how the system fits together
- [CONTRIBUTING.md](../CONTRIBUTING.md) - getting a dev environment running

## Response expectations

Meridian is maintained by a small team. Issues are read, and security reports are
acknowledged within 48 hours. General issues and discussions may take longer - a
reproducible report with logs attached gets resolved fastest.
