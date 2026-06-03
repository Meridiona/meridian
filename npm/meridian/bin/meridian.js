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

// Is the global npm prefix writable without root? `npm i -g` installs into
// <prefix>/lib/node_modules; on a /usr/local prefix that needs sudo, on a
// Homebrew/user prefix it doesn't.
function npmGlobalWritable() {
  const r = spawnSync('npm', ['config', 'get', 'prefix'], { encoding: 'utf8' });
  const prefix = (r.stdout || '').trim();
  if (!prefix) return true; // unknown — let npm decide
  try {
    fs.accessSync(path.join(prefix, 'lib', 'node_modules'), fs.constants.W_OK);
    return true;
  } catch {
    return false;
  }
}

// Update the global package, elevating ONLY this step when the prefix needs
// root. The rest of `update` (setup: per-user launchd agents, venv) must NOT run
// as root, so we never sudo the whole command — just this one install.
function npmInstallLatest() {
  // Install both packages explicitly — npm won't always upgrade the optional
  // arch package when only the main package is requested.
  const args = ['install', '-g', '@meridiona/meridian@latest', '@meridiona/meridian-darwin-arm64@latest'];
  if (npmGlobalWritable()) {
    return spawnSync('npm', args, { stdio: 'inherit' });
  }
  console.error('meridian update: the global npm prefix needs root — elevating just the');
  console.error('  package install (you may be prompted for your password)…');
  return spawnSync('sudo', ['npm', ...args], { stdio: 'inherit' });
}

const cmd = process.argv[2];
const rest = process.argv.slice(3);

// Every Meridian command is per-user: launchd agents live under gui/$UID and
// state under ~/.meridian. Running as root (e.g. `sudo meridian update`) makes
// launchd bootstrap fail ("Domain does not support specified action") and leaves
// root-owned files behind. Refuse up front — the one step that genuinely needs
// root (`npm install -g` during `update`) is elevated on its own below.
if (typeof process.getuid === 'function' && process.getuid() === 0) {
  console.error('meridian: do not run as root / with sudo.');
  console.error('  Meridian runs per-user (launchd agents under gui/$UID, files in ~/.meridian);');
  console.error('  as root, launchd fails and ~/.meridian fills with root-owned files.');
  console.error(`  Run it as your normal user:  meridian ${cmd || '<command>'}`);
  console.error('  (`meridian update` elevates just the npm install step if your prefix needs root.)');
  process.exit(1);
}

if (cmd === 'setup' || cmd === 'install') {
  const bundle = resolveBundle();
  run('bash', [path.join(bundle, 'scripts', 'meridian-npm-setup.sh'), bundle, ...rest]);
} else if (cmd === 'update' || cmd === '_update-continue') {
  if (cmd === 'update') {
    // Step 1: update the thin launcher only. The current process runs the OLD
    // launcher in memory — fixes to also install darwin-arm64 may only be in the
    // NEW launcher on disk. Re-exec the newly installed launcher so step 2 always
    // uses the latest code, even when invoked by an old launcher.
    console.log('meridian update: downloading latest release…');
    console.log('  The package includes a pre-built Python venv (~160 MB) — expect ~1-3 min.');
    const _start = Date.now();
    const up = spawnSync('npm', ['install', '-g', '@meridiona/meridian@latest'], { stdio: 'inherit' });
    if (up.status) process.exit(up.status);
    console.log(`  Launcher updated in ${Math.round((Date.now() - _start) / 1000)}s`);
    // Re-exec the freshly installed launcher to continue with the correct code.
    const npmRoot = (spawnSync('npm', ['root', '-g'], { encoding: 'utf8' }).stdout || '').trim();
    const newLauncher = path.join(npmRoot, '@meridiona', 'meridian', 'bin', 'meridian.js');
    if (fs.existsSync(newLauncher)) {
      const r = spawnSync(process.execPath, [newLauncher, '_update-continue'], { stdio: 'inherit' });
      process.exit(r.status ?? 1);
    }
    // Fallback: new launcher not found, continue in this process.
  }
  // Step 2 (runs in new launcher): install bundle + run setup.
  const _bundleStart = Date.now();
  const bup = spawnSync('npm', ['install', '-g', '@meridiona/meridian-darwin-arm64@latest'],
    { stdio: 'inherit', env: { ...process.env } });
  if (bup.status) process.exit(bup.status);
  console.log(`  Bundle downloaded in ${Math.round((Date.now() - _bundleStart) / 1000)}s`);
  const bundle = resolveBundle();
  run('bash', [path.join(bundle, 'scripts', 'meridian-npm-setup.sh'), bundle, '--skip-permissions']);
} else {
  // Prefer the CLI installed at ~/.meridian/app (post-setup); fall back to the
  // bundle's copy (e.g. running a command before `meridian setup`).
  const appCli = path.join(os.homedir(), '.meridian', 'app', 'scripts', 'meridian-cli.sh');
  const cli = fs.existsSync(appCli) ? appCli : path.join(resolveBundle(), 'scripts', 'meridian-cli.sh');
  run('bash', [cli, ...process.argv.slice(2)]);
}
