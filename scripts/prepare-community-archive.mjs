#!/usr/bin/env node
// Local maintainer workflow: configure Community, then archive manually in Xcode.
// Relay registration, notarization and publication remain separate operations.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const origins = { production: 'https://relay.ditchnow.nl', staging: 'https://ditch-remote-relay-staging.matin-1a7.workers.dev' };
const targets = ['x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu', 'aarch64-apple-darwin', 'x86_64-apple-darwin'];
const trustKeys = ['DITCH_SPARKLE_PUBLIC_ED_KEY', 'DITCH_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64',
  'DITCH_COMMUNITY_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64', 'DITCH_APPLE_TEAM_ID', 'DITCH_COMMUNITY_BUILD_SEQUENCE'];
const usage = 'usage: ./community-production.sh VERSION BUILD (or ./community-staging.sh VERSION BUILD)';
const hash = text => createHash('sha256').update(text).digest('hex');

export function parseArgs(args) {
  const [environment, version, value] = args;
  if (args.length !== 3 || !Object.hasOwn(origins, environment)
      || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version ?? '')
      || !/^[1-9]\d*$/.test(value ?? '') || !Number.isSafeInteger(Number(value))) throw Error(usage);
  return { environment, version, build: Number(value) };
}
function readEnv(file, allowed) {
  const text = fs.readFileSync(file, 'utf8');
  const values = {};
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const match = /^([A-Z0-9_]+)=(.*)$/.exec(line);
    if (!match || Object.hasOwn(values, match[1]) || (allowed && !allowed.includes(match[1]))) throw Error(`Invalid or unsupported setting in ${file}.`);
    values[match[1]] = match[2].trim().replace(/^(['"])(.*)\1$/, '$2');
  }
  if (allowed && allowed.some(key => !values[key])) throw Error(`Incomplete public configuration: ${file}`);
  return { text, values };
}
function privateText(file) {
  const info = fs.lstatSync(file);
  if (!info.isFile() || info.isSymbolicLink() || (info.mode & 0o777) !== 0o600 || info.uid !== process.getuid()) throw Error(`Expected an owned mode-0600 private file: ${file}`);
  return fs.readFileSync(file, 'utf8').trim();
}
function execute(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', ...options });
  if (result.error || result.status !== 0) throw Error(`${path.basename(command)} failed (${result.status ?? 'could not start'}).`);
  return (result.stdout ?? '').trim();
}
function launch(command, args, options) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { ...options, detached: true });
    child.once('error', reject);
    child.once('spawn', () => { child.unref(); resolve(); });
  });
}

export async function run(args, {
  workspace = root, home = os.homedir(), environment = process.env, exec = execute, launchXcode = launch,
  xcodeRunning = () => spawnSync('pgrep', ['-x', 'Xcode']).status === 0,
  freeBytes = dir => { const stat = fs.statfsSync(dir); return stat.bavail * stat.bsize; },
  xcode = '/Applications/Xcode.app/Contents/MacOS/Xcode', log = console.log,
} = {}) {
  const o = parseArgs(args);
  const community = path.join(workspace, 'community');
  if (xcodeRunning()) throw Error('Quit Xcode completely, then run this command again.');
  fs.accessSync(xcode, fs.constants.X_OK);
  // Match the local Commercial wrappers' current disk-space thresholds.
  const minimumBytes = (o.environment === 'production' ? 1437184 : 7437184) * 1024;
  if (freeBytes(workspace) < minimumBytes) throw Error(`At least ${(minimumBytes / 1024 ** 3).toFixed(2)} GiB of free disk space is required.`);
  const revision = exec('git', ['-C', community, 'rev-parse', 'HEAD']);
  if (!/^[a-f0-9]{40}$/.test(revision)) throw Error('Invalid Community source revision.');
  if (exec('git', ['-C', community, 'status', '--porcelain'])) throw Error('Commit the Community source changes before preparing an official archive. Local ignored .env files may remain.');

  const appEnv = readEnv(path.join(community, `apps/macos/config/.env.${o.environment}`),
    ['DITCH_DEPLOYMENT_ENVIRONMENT', 'DITCH_RELAY_ORIGIN', 'DITCH_UPDATE_ALLOWED_HOSTS']).values;
  if (appEnv.DITCH_DEPLOYMENT_ENVIRONMENT !== o.environment || appEnv.DITCH_RELAY_ORIGIN !== origins[o.environment]
      || !appEnv.DITCH_UPDATE_ALLOWED_HOSTS.split(',').includes(new URL(origins[o.environment]).host)) throw Error('Community environment does not match the requested Relay.');
  const publicFile = path.join(community, `apps/macos/config/.env.release.${o.environment}`);
  const trust = readEnv(publicFile, trustKeys);
  if (!/^DITCH_COMMUNITY_BUILD_SEQUENCE=.*$/m.test(trust.text)) throw Error('Community build sequence is missing.');
  const commercialTrust = readEnv(path.join(workspace, `apps/macos/config/.env.release.${o.environment}`), trustKeys).values;
  for (const key of trustKeys.filter(key => key !== 'DITCH_COMMUNITY_BUILD_SEQUENCE')) {
    if (trust.values[key] !== commercialTrust[key]) throw Error(`Community and Commercial release trust disagree: ${key}`);
  }
  // Read only the tool/certificate choices. Do not source this file or export Relay secrets.
  const secretsFile = path.join(workspace, `.env.release-secrets.${o.environment}`);
  privateText(secretsFile);
  const settings = readEnv(secretsFile).values;
  if (!settings.DITCH_FLUTTER || !settings.DITCH_CODESIGN_IDENTITY?.startsWith('Developer ID Application: ')) throw Error(`Configure DITCH_FLUTTER and DITCH_CODESIGN_IDENTITY in ${secretsFile}.`);
  const flutter = fs.statSync(settings.DITCH_FLUTTER).isDirectory()
    ? path.join(settings.DITCH_FLUTTER, 'bin/flutter') : settings.DITCH_FLUTTER;
  fs.accessSync(flutter, fs.constants.X_OK);

  // Match Relay's community-macos-release.mjs, including its retry receipt schema.
  const directory = path.join(home, 'Library/Application Support/DitchNow/ReleaseKeys', o.environment, 'community');
  const credentialFile = path.join(directory, `official-build-${o.version}-${o.build}.b64`);
  const requestFile = path.join(directory, `official-build-${o.version}-${o.build}.registration.json`);
  let credential = fs.existsSync(credentialFile) ? privateText(credentialFile) : null;
  let request = fs.existsSync(requestFile) ? JSON.parse(privateText(requestFile)) : null;
  if (credential && (!/^[A-Za-z0-9_-]{43,128}$/.test(credential) || Buffer.from(credential, 'base64url').toString('base64url') !== credential)) throw Error('Invalid saved Community credential.');
  const expected = { protocol_version: 1, deployment_environment: o.environment, edition: 'community', version: o.version,
    build: o.build, channel: o.environment === 'production' ? 'stable' : 'beta', community_revision: revision };
  if (request && (!credential || Object.entries(expected).some(([key, value]) => request[key] !== value)
      || request.credential_sha256 !== hash(credential))) throw Error('Saved Community registration belongs to different source/build/credential. Choose a new build number.');

  const buildEnv = { ...environment };
  for (const key of Object.keys(buildEnv)) if (/^(DITCH_|CLOUDFLARE_|STRIPE_|INTERNAL_ADMIN_SECRET$|APNS_PRIVATE_KEY$)/.test(key)) delete buildEnv[key];
  const identifier = exec(path.join(community, 'scripts/build-identifier'), [], { cwd: community, env: buildEnv });
  if (!/^[A-Za-z0-9._-]+$/.test(identifier)) throw Error('Invalid Community build identifier.');
  const workspaceVersion = /^version = "([^"]+)"/m.exec(fs.readFileSync(path.join(community, 'Cargo.toml'), 'utf8'))?.[1];
  if (!workspaceVersion) throw Error('Community workspace version is missing.');
  const artifacts = path.join(community, 'dist/remote', workspaceVersion);
  Object.assign(buildEnv, appEnv, { DITCH_EDITION: 'community', DITCH_APP_VERSION: o.version, DITCH_BUILD_NUMBER: String(o.build),
    DITCH_RELEASE_SEQUENCE: String(o.build), DITCH_COMMUNITY_REVISION: revision, DITCH_BUILD_IDENTIFIER: identifier,
    DITCH_FLUTTER: flutter, DITCH_CODESIGN_IDENTITY: settings.DITCH_CODESIGN_IDENTITY,
    DITCH_REMOTE_TARGETS: targets.join(' '), DITCH_REMOTE_ARTIFACT_OUTPUT: artifacts, DITCH_REMOTE_ARTIFACT_DIR: artifacts });

  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  if (!credential) {
    credential = randomBytes(48).toString('base64url');
    fs.writeFileSync(credentialFile, `${credential}\n`, { flag: 'wx', mode: 0o600 });
  }
  if (!request) {
    request = { ...expected, build_id: randomUUID(), credential_sha256: hash(credential), published_at: Date.now() };
    fs.writeFileSync(requestFile, `${JSON.stringify(request, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  }
  fs.writeFileSync(publicFile, trust.text.replace(/^DITCH_COMMUNITY_BUILD_SEQUENCE=.*$/m, `DITCH_COMMUNITY_BUILD_SEQUENCE=${o.build}`));
  log(`Preparing ${o.environment} Community ${o.version} (${o.build}) and all four remote runtimes.`);
  exec(path.join(community, 'scripts/build-remote-artifacts'), [], { cwd: community, env: buildEnv, stdio: 'inherit' });
  const manifest = JSON.parse(fs.readFileSync(path.join(artifacts, 'remote-artifacts.json'), 'utf8'));
  if (manifest.edition !== 'community' || manifest.community_revision !== revision || manifest.build_identifier !== identifier
      || JSON.stringify(manifest.artifacts?.map(item => item.target).sort()) !== JSON.stringify([...targets].sort())) throw Error('Community remote runtime manifest does not match this build.');
  buildEnv.DITCH_OFFICIAL_BUILD_CREDENTIAL_FILE = credentialFile;
  exec(path.join(community, 'scripts/macos-app'), [o.environment, 'build', '--release', '--config-only', '--build-name', o.version, '--build-number', String(o.build)],
    { cwd: community, env: buildEnv, stdio: 'inherit' });
  const logFile = path.join(os.tmpdir(), `ditch-xcode-community-${o.environment}-${o.version}-${o.build}.log`);
  const fd = fs.openSync(logFile, 'a', 0o600);
  try {
    await launchXcode(xcode, [path.join(community, 'apps/macos/macos/Runner.xcworkspace')],
      { cwd: community, env: buildEnv, stdio: ['ignore', fd, fd] });
  } finally { fs.closeSync(fd); }
  log(`Xcode opened for Community ${o.version} (${o.build}). Use Product > Archive, then Developer ID > Upload.\nAfter exporting, package the Community app and publish through Relay.\nCredential: ${credentialFile}\nRegistration receipt: ${requestFile}\nXcode log: ${logFile}`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  run(process.argv.slice(2)).catch(error => { console.error(`error: ${error.message}`); process.exitCode = 1; });
}
