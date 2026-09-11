import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { parseArgs, run } from './prepare-community-archive.mjs';
const revision = 'a'.repeat(40);
const targets = ['x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu', 'aarch64-apple-darwin', 'x86_64-apple-darwin'];
function fixture(t) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'ditch-community-archive-test-'));
  t.after(() => fs.rmSync(workspace, { recursive: true, force: true }));
  const home = path.join(workspace, 'home'), community = path.join(workspace, 'community');
  function file(name, value, mode = 0o600) {
    const dest = path.join(workspace, name);
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.writeFileSync(dest, value, { mode });
    return dest;
  }
  const flutter = file('tools/flutter', '#!/bin/sh\nexit 0\n', 0o700);
  const xcode = file('tools/Xcode', '#!/bin/sh\nexit 0\n', 0o700);
  file('community/Cargo.toml', '[workspace.package]\nversion = "0.1.1"\n');
  for (const environment of ['staging', 'production']) {
    const host = environment === 'production' ? 'relay.ditchnow.nl' : 'ditch-remote-relay-staging.matin-1a7.workers.dev';
    file(`community/apps/macos/config/.env.${environment}`, `DITCH_DEPLOYMENT_ENVIRONMENT=${environment}\nDITCH_RELAY_ORIGIN=https://${host}\nDITCH_UPDATE_ALLOWED_HOSTS=${host}\n`);
    const trust = `DITCH_SPARKLE_PUBLIC_ED_KEY=${environment}-sparkle\nDITCH_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64=${environment}-commercial\nDITCH_COMMUNITY_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64=${environment}-community\nDITCH_APPLE_TEAM_ID=TESTTEAM12\nDITCH_COMMUNITY_BUILD_SEQUENCE=102\n`;
    file(`community/apps/macos/config/.env.release.${environment}`, trust);
    file(`apps/macos/config/.env.release.${environment}`, trust);
    file(`.env.release-secrets.${environment}`, `DITCH_FLUTTER=${flutter}\nDITCH_CODESIGN_IDENTITY=Developer ID Application: Test (TESTTEAM12)\nDITCH_RELEASE_REGISTER_TOKEN=do-not-export\n`);
  }
  const calls = [], launches = [], messages = [];
  let dirty = false, badManifest = false, currentRevision = revision;
  const options = { workspace, home, xcode, freeBytes: () => 100 * 1024 ** 3, xcodeRunning: () => false,
    environment: { PATH: process.env.PATH, DITCH_EDITION: 'commercial', DITCH_OFFICIAL_BUILD_CREDENTIAL: 'do-not-inherit',
      DITCH_REMOTE_ARTIFACT_DIR: '/wrong/commercial', CLOUDFLARE_API_TOKEN: 'cf-private', STRIPE_SECRET_KEY: 'stripe-private', INTERNAL_ADMIN_SECRET: 'admin-private' },
    log: message => messages.push(message),
    exec(command, args, opts = {}) {
      calls.push({ command, args, opts: { ...opts, ...(opts.env ? { env: { ...opts.env } } : {}) } });
      if (command === 'git') return args.includes('status') ? dirty ? ' M source.rs' : '' : currentRevision;
      if (command.endsWith('/build-identifier')) return '0.1.1-community-source';
      if (command.endsWith('/build-remote-artifacts')) {
        const dir = opts.env.DITCH_REMOTE_ARTIFACT_OUTPUT;
        fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(path.join(dir, 'remote-artifacts.json'), JSON.stringify({ edition: badManifest ? 'commercial' : 'community',
          community_revision: currentRevision, build_identifier: opts.env.DITCH_BUILD_IDENTIFIER, artifacts: targets.map(target => ({target})) }));
        return '';
      }
      if (command.endsWith('/macos-app')) return '';
      throw Error(`Unexpected test command: ${command}`);
    },
    async launchXcode(command, args, opts) { launches.push({ command, args, opts }); },
  };
  return { workspace, home, community, options, calls, launches, messages, file,
    setDirty: value => dirty = value, setBadManifest: value => badManifest = value, setRevision: value => currentRevision = value };
}
test('validates version and build arguments', () => {
  assert.deepEqual(parseArgs(['production','0.1.0','122']), {environment:'production',version:'0.1.0',build:122});
  for (const args of [[], ['test','0.1.0','122'], ['staging','bad','122'], ['production','0.1.0','0'], ['production','0.1.0','01'], ['production','0.1.0','9007199254740992']]) assert.throws(() => parseArgs(args));
});
for (const environment of ['staging','production']) test(`${environment} prepares Community-only archive inputs and compatible reusable Relay receipt`, async t => {
  const f = fixture(t);
  await run([environment,'0.1.0','122'], f.options);
  const build = f.calls.find(call => call.command.endsWith('/macos-app'));
  assert.deepEqual(build.args, [environment,'build','--release','--config-only','--build-name','0.1.0','--build-number','122']);
  assert.equal(build.opts.env.DITCH_EDITION, 'community');
  assert.equal(build.opts.env.DITCH_RELEASE_SEQUENCE, '122');
  assert.equal(build.opts.env.DITCH_APP_VERSION, '0.1.0');
  assert.equal(build.opts.env.DITCH_COMMUNITY_REVISION, revision);
  assert(build.opts.env.DITCH_REMOTE_ARTIFACT_DIR.startsWith(f.community));
  const remote = f.calls.find(call => call.command.endsWith('/build-remote-artifacts'));
  assert.equal(remote.opts.env.DITCH_REMOTE_TARGETS.split(' ').length, 4);
  assert.equal(remote.opts.env.DITCH_OFFICIAL_BUILD_CREDENTIAL_FILE, undefined);
  for (const key of ['DITCH_RELEASE_REGISTER_TOKEN','DITCH_OFFICIAL_BUILD_CREDENTIAL','CLOUDFLARE_API_TOKEN','STRIPE_SECRET_KEY','INTERNAL_ADMIN_SECRET']) assert.equal(build.opts.env[key], undefined);
  assert.equal(f.launches.length, 1);
  assert.deepEqual(f.launches[0].args, [path.join(f.community,'apps/macos/macos/Runner.xcworkspace')]);
  assert.deepEqual(f.launches[0].opts.env, build.opts.env);
  const credentialFile = build.opts.env.DITCH_OFFICIAL_BUILD_CREDENTIAL_FILE;
  assert.equal(credentialFile,path.join(f.home,'Library/Application Support/DitchNow/ReleaseKeys',environment,'community/official-build-0.1.0-122.b64'));
  const credential = fs.readFileSync(credentialFile,'utf8').trim();
  assert.match(credential,/^[A-Za-z0-9_-]{64}$/);
  assert.equal(fs.statSync(credentialFile).mode & 0o777,0o600);
  const receiptFile = credentialFile.replace('.b64','.registration.json');
  const receipt = JSON.parse(fs.readFileSync(receiptFile));
  assert.equal(receipt.edition,'community');assert.equal(receipt.build,122);assert.equal(receipt.community_revision,revision);
  assert.equal(receipt.credential_sha256,createHash('sha256').update(credential).digest('hex'));
  assert.equal(receipt.channel, environment==='production'?'stable':'beta');
  assert.equal(fs.statSync(receiptFile).mode & 0o777,0o600);
  assert.match(fs.readFileSync(path.join(f.community,`apps/macos/config/.env.release.${environment}`),'utf8'),/^DITCH_COMMUNITY_BUILD_SEQUENCE=122$/m);
  assert.match(fs.readFileSync(path.join(f.workspace,`apps/macos/config/.env.release.${environment}`),'utf8'),/^DITCH_COMMUNITY_BUILD_SEQUENCE=102$/m);
  assert(!f.messages.join('\n').includes(credential));
  await run([environment,'0.1.0','122'],f.options);
  assert.equal(fs.readFileSync(credentialFile,'utf8').trim(),credential);
  assert.deepEqual(JSON.parse(fs.readFileSync(receiptFile)),receipt);
  f.setRevision('b'.repeat(40));
  await assert.rejects(run([environment,'0.1.0','122'],f.options),/different source/);
  assert.equal(fs.readFileSync(credentialFile,'utf8').trim(),credential);
});
test('refuses dirty Community source before generating credentials', async t => {
  const f=fixture(t);f.setDirty(true);
  await assert.rejects(run(['production','0.1.0','122'],f.options),/Commit the Community/);
  assert(!fs.existsSync(f.home));assert.equal(f.launches.length,0);
});
test('requires Xcode to be closed and sufficient disk space', async t => {
  const f=fixture(t);
  await assert.rejects(run(['production','0.1.0','122'],{...f.options,xcodeRunning:()=>true}),/Quit Xcode/);
  await assert.rejects(run(['production','0.1.0','122'],{...f.options,freeBytes:()=>0}),/free disk space/);
  assert(!fs.existsSync(f.home));
});
test('rejects mismatched environment and upgrade trust before generating credentials', async t => {
  const f=fixture(t);
  const file=path.join(f.community,'apps/macos/config/.env.release.production');
  fs.writeFileSync(file,fs.readFileSync(file,'utf8').replace('production-commercial','wrong-commercial'));
  await assert.rejects(run(['production','0.1.0','122'],f.options),/release trust disagree/);
  assert(!fs.existsSync(f.home));
});
test('rejects Commercial remote artifacts before configuring or opening Xcode', async t => {
  const f=fixture(t);f.setBadManifest(true);
  await assert.rejects(run(['production','0.1.0','122'],f.options),/runtime manifest/);
  assert.equal(f.launches.length,0);
  assert(!f.calls.some(call=>call.command.endsWith('/macos-app')));
});
