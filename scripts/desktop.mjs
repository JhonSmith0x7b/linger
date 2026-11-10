import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
// Use the isolated development toolchain if present; standard installs use PATH.
const env = { ...process.env };
if (existsSync(resolve('.cache/rustup/toolchains'))) {
  env.RUSTUP_HOME = resolve('.cache/rustup');
  env.CARGO_HOME = resolve('.cache/cargo');
}
const binary = resolve('node_modules', '.bin', process.platform === 'win32' ? 'tauri.cmd' : 'tauri');
const args = process.argv.slice(2);
// A stable development signer lets macOS recognize local app updates for TCC/Keychain.
// Do not choose a release identity, change keychain trust, or sign raw `tauri dev` binaries.
const localDebugBundle = process.platform === 'darwin'
  && args[0] === 'build' && args.includes('--debug') && !args.includes('--no-bundle');
if (localDebugBundle && !env.APPLE_SIGNING_IDENTITY) {
  const identities = spawnSync('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning'], {
    encoding: 'utf8', timeout: 10000,
  });
  const development = [...(identities.stdout || '').matchAll(/^[ \t]*\d+\) [A-Fa-f0-9]+ "(Apple Development:[^"]+)"/gm)]
      .map(match => match[1]);
  if (identities.status === 0 && development.length === 1) {
    env.APPLE_SIGNING_IDENTITY = development[0];
    console.info('Using the local Apple Development identity for this debug app bundle.');
  } else {
    console.warn('No unique Apple Development identity selected. Set APPLE_SIGNING_IDENTITY for stable macOS permissions across rebuilds.');
  }
}
const child = spawn(binary, args.length ? args : ['dev'], { stdio: 'inherit', env, shell: process.platform === 'win32' });
child.on('exit', code => process.exit(code ?? 1));
child.on('error', error => { console.error(error.message); process.exit(1); });
