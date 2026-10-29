#!/usr/bin/env python3
"""Remote packaging tests with disposable files and fake compilers; no deployment."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

COMMUNITY = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('packaging', COMMUNITY / 'scripts/package-remote-artifacts.py')
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)


class Artifacts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'source'
        self.source.mkdir()
        self.destination = self.root / 'bundle'
        self.native = self.root / 'native'
        self.native.write_bytes(b'native runtime')
        self.metadata = dict(deployment_environment='staging', edition='community',
                             version='1.2.3', build_identifier='source-abc', remote_runtime_protocol_version=4)

    def manifest(self, **overrides):
        binary = self.source / 'ditchd-x86_64-unknown-linux-gnu'
        binary.write_bytes(b'linux runtime')
        data = dict(self.metadata, artifacts=[dict(target='x86_64-unknown-linux-gnu',
                    artifact=binary.name, sha256=packaging.digest(binary))])
        data.update(overrides)
        (self.source / 'remote-artifacts.json').write_text(json.dumps(data))

    def bundle(self, source=None):
        packaging.bundle(source or self.source, self.destination, self.native,
                         'aarch64-apple-darwin', self.metadata)
        return json.loads((self.destination / 'remote-artifacts.json').read_text())

    def test_native_fallback_has_valid_manifest_and_removes_stale_resources(self):
        self.manifest()
        self.bundle()
        data = self.bundle(Path('/dev/null'))
        self.assertEqual(len(data['artifacts']), 1)
        self.assertFalse((self.destination / 'ditchd-x86_64-unknown-linux-gnu').exists())
        entry = data['artifacts'][0]
        self.assertEqual(entry['sha256'], packaging.digest(self.destination / entry['artifact']))

    def test_only_selected_manifest_entries_are_copied(self):
        self.manifest(community_revision='fixture-revision')
        (self.source / 'ditchd-unselected').write_bytes(b'stale')
        data = self.bundle()
        self.assertEqual(len(data['artifacts']), 2)
        self.assertEqual(data['community_revision'], 'fixture-revision')
        self.assertFalse((self.destination / 'ditchd-unselected').exists())

    def test_invalid_metadata_and_checksums_preserve_previous_bundle(self):
        self.bundle(Path('/dev/null'))
        original = (self.destination / 'remote-artifacts.json').read_bytes()
        for overrides in (dict(deployment_environment='production'), dict(edition='commercial'),
                          dict(build_identifier='old'), dict(remote_runtime_protocol_version=999)):
            self.manifest(**overrides)
            with self.assertRaises(ValueError):
                self.bundle()
        self.manifest()
        (self.source / 'ditchd-x86_64-unknown-linux-gnu').write_bytes(b'changed')
        with self.assertRaises(ValueError):
            self.bundle()
        self.assertEqual(original, (self.destination / 'remote-artifacts.json').read_bytes())

    def test_missing_manifest_and_unsafe_filenames_are_rejected(self):
        with self.assertRaises(ValueError):
            self.bundle()
        self.manifest(artifacts=[dict(target='x86_64-unknown-linux-gnu', artifact='../native', sha256='')])
        with self.assertRaises(ValueError):
            self.bundle()

    def test_legacy_manifest_is_only_accepted_in_production(self):
        self.manifest()
        path = self.source / 'remote-artifacts.json'
        data = json.loads(path.read_text())
        del data['deployment_environment']
        path.write_text(json.dumps(data))
        with self.assertRaises(ValueError):
            self.bundle()
        self.metadata['deployment_environment'] = 'production'
        self.bundle()

    def fixture(self, source):
        root = self.root / source.name
        (root / 'scripts').mkdir(parents=True)
        for name in ('build-identifier', 'build-remote-artifacts', 'macos-app'):
            shutil.copy2(source / 'scripts' / name, root / 'scripts' / name)
        for path, text in {'Cargo.toml':'version = "1.2.3"\n', 'Cargo.lock':'lock',
                           'COMMUNITY_REVISION':'fixture-pin',
                           'crates/ditch_protocol/src/lib.rs':'pub const REMOTE_RUNTIME_PROTOCOL_VERSION: u16 = 4;\n',
                           'config/deployment-identities.json':'{}',
                           'community/config/deployment-identities.json':'{}',
                           'community/Cargo.toml':'shared', 'community/Cargo.lock':'shared',
                           'community/crates/runtime.rs':'shared runtime',
                           'apps/macos/config/.env.staging':'DITCH_DEPLOYMENT_ENVIRONMENT=staging\nDITCH_RELAY_ORIGIN=https://ditch-remote-relay-staging.matin-1a7.workers.dev\nDITCH_UPDATE_ALLOWED_HOSTS=ditch-remote-relay-staging.matin-1a7.workers.dev\n'}.items():
            dest = root / path
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(text)
        fakebin = root / 'fakebin'
        fakebin.mkdir()
        (fakebin / 'lib').mkdir()
        (fakebin / 'lib/libstd-fixture.rlib').touch()
        executables = {
            'uname': '#!/bin/sh\nprintf "Darwin\\n"\n',
            'rustc': '#!/bin/sh\nprintf "%s/lib\\n" "$(dirname "$0")"\n',
            'git': '#!/bin/sh\nprintf "fixture-revision\\n"\n',
            'cargo': f'#!{sys.executable}\nimport os, pathlib, sys\ntarget=sys.argv[sys.argv.index("--target")+1]\np=pathlib.Path(os.environ["CARGO_TARGET_DIR"])/target/"release/ditchd"\np.parent.mkdir(parents=True,exist_ok=True)\np.write_text(os.environ["DITCH_BUILD_IDENTIFIER"])\n',
            'flutter': '#!/bin/sh\nprintf "%s\\n" "${DITCH_BUILD_IDENTIFIER:-}" "${DITCH_REMOTE_ARTIFACT_DIR:-}" >> "$FIXTURE_FLUTTER_LOG"\n',
            'docker': f'#!{sys.executable}\nimport os, pathlib, sys\na=sys.argv\ne=dict(a[i+1].split("=",1) for i,v in enumerate(a) if v=="-e")\nr=pathlib.Path(os.environ["FIXTURE_FLUTTER_LOG"]).parent\np=r/e["CARGO_TARGET_DIR"].removeprefix("/workspace/")/"release/ditchd"\np.parent.mkdir(parents=True,exist_ok=True)\np.write_text(e["DITCH_BUILD_IDENTIFIER"])\n(r/"docker.log").write_text(" ".join(a))\n',
        }
        for name, content in executables.items():
            path = fakebin / name
            path.write_text(content)
            path.chmod(0o755)
        # No Xcode generation is needed to test wrapper-to-builder propagation.
        for prefix in ('', 'community/'):
            path = root / (prefix + 'scripts/configure-macos-environments.py')
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('')
        env = {k:v for k,v in os.environ.items() if not k.startswith('DITCH_')}
        env.update(PATH=str(fakebin)+os.pathsep+env['PATH'], DITCH_FLUTTER=str(fakebin/'flutter'),
                   DITCH_DEPLOYMENT_ENVIRONMENT='staging', DITCH_REMOTE_TARGETS='aarch64-apple-darwin',
                   FIXTURE_FLUTTER_LOG=str(root/'flutter.log'))
        return root, env

    def test_native_run_and_show_do_not_build_extra_targets(self):
        root, env = self.fixture(COMMUNITY)
        env.pop('DITCH_REMOTE_TARGETS')
        for action in ('show', 'run'):
            result = subprocess.run([str(root/'scripts/macos-app'), 'staging', action], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((root/'dist/remote').exists())

    def test_linux_cross_build_uses_docker_even_with_installed_standard_library(self):
        root, env = self.fixture(COMMUNITY)
        env['DITCH_REMOTE_TARGETS'] = 'x86_64-unknown-linux-gnu'
        result = subprocess.run([str(root/'scripts/macos-app'), 'staging', 'run'], env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        data = json.loads((root/'dist/remote/staging/1.2.3/remote-artifacts.json').read_text())
        self.assertEqual(data['artifacts'][0]['target'], 'x86_64-unknown-linux-gnu')
        self.assertIn('linux/amd64', (root/'docker.log').read_text())
        self.assertIn('DITCH_DEPLOYMENT_ENVIRONMENT=staging', (root/'docker.log').read_text())

    def test_wrappers_build_selected_targets_with_desktop_identity(self):
        sources = [COMMUNITY]
        if (COMMUNITY.parent / 'COMMUNITY_REVISION').is_file():
            sources.append(COMMUNITY.parent)
        for source in sources:
            with self.subTest(edition=source.name):
                root, env = self.fixture(source)
                result = subprocess.run([str(root/'scripts/macos-app'), 'staging', 'run'], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                data = json.loads((root/'dist/remote/staging/1.2.3/remote-artifacts.json').read_text())
                identity = subprocess.check_output([str(root/'scripts/build-identifier')], env=env, text=True).strip()
                self.assertEqual(data['build_identifier'], identity)
                self.assertEqual([a['target'] for a in data['artifacts']], ['aarch64-apple-darwin'])
                self.assertIn(identity, (root/'flutter.log').read_text())
                self.assertFalse((root/'dist/remote/production').exists())
                # A shared source edit must invalidate the Commercial fingerprint.
                (root/'community/crates/runtime.rs').write_text('changed')
                if source != COMMUNITY:
                    updated = subprocess.check_output([str(root/'scripts/build-identifier')], env=env, text=True).strip()
                    self.assertNotEqual(identity, updated)
                env['DITCH_REMOTE_TARGETS'] = 'invalid-target'
                result = subprocess.run([str(root/'scripts/build-remote-artifacts')], env=env, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('unsupported remote target', result.stderr)


if __name__ == '__main__':
    unittest.main()
