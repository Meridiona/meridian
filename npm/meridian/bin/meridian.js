#!/usr/bin/env node
// meridian — normalises screenpipe activity into structured app sessions
//
// Thin launcher for the `@meridiona/meridian` npm package. The prebuilt app
// (daemon binary + dashboard + Python services + scripts) lives in the per-arch
// optional dependency. This shim:
//   * `meridian setup`  → copies the bundle to ~/.meridian/app + installs the
//                         daemons (prereqs, Python venv/MLX, launchd agents).
//   * `meridian update` → reinstalls the latest npm package, then re-runs setup.
//   * everything else   → delegates to the installed CLI (start/stop/logs/…).
'use strict';

const path = require('path');
const fs = require('fs');
const os = require('os');
const { spawnSync } = require('child_process');

if (process.platform !== 'darwin' || process.arch !== 'arm64') {
  console.error('Meridian runs on macOS Apple Silicon (arm64) only.');
  process.exit(1);
}

function resolveBundle() {
  try {
    return path.dirname(require.resolve('@meridiona/meridian-darwin-arm64/package.json'));
  } catch {
    console.error('Meridian: the prebuilt package @meridiona/meridian-darwin-arm64 is not installed.');
    console.error('Reinstall with:  npm install -g @meridiona/meridian');
    process.exit(1);
  }
}

function run(file, args) {
  const r = spawnSync(file, args, { stdio: 'inherit', env: process.env });
  process.exit(r.status == null ? 1 : r.status);
}

const cmd = process.argv[2];
const rest = process.argv.slice(3);

if (cmd === 'setup' || cmd === 'install') {
  const bundle = resolveBundle();
  run('bash', [path.join(bundle, 'scripts', 'meridian-npm-setup.sh'), bundle, ...rest]);
} else if (cmd === 'update') {
  const up = spawnSync('npm', ['install', '-g', '@meridiona/meridian@latest'], { stdio: 'inherit' });
  if (up.status) process.exit(up.status);
  const bundle = resolveBundle();
  run('bash', [path.join(bundle, 'scripts', 'meridian-npm-setup.sh'), bundle, '--skip-permissions']);
} else {
  // Prefer the CLI installed at ~/.meridian/app (post-setup); fall back to the
  // bundle's copy (e.g. running a command before `meridian setup`).
  const appCli = path.join(os.homedir(), '.meridian', 'app', 'scripts', 'meridian-cli.sh');
  const cli = fs.existsSync(appCli) ? appCli : path.join(resolveBundle(), 'scripts', 'meridian-cli.sh');
  run('bash', [cli, ...process.argv.slice(2)]);
}
