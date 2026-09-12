#!/usr/bin/env python3
"""Exercise the archive signing step with real Mach-O files and ad-hoc signing."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import platform
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'apps/macos/macos/sign_status_host.sh'


def run(*args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kwargs)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


@unittest.skipUnless(platform.system() == 'Darwin', 'requires macOS codesign')
class RuntimeSigningTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.compiler_dir = tempfile.TemporaryDirectory(prefix='ditch-signing-compiler-')
        cls.addClassCleanup(cls.compiler_dir.cleanup)
        cls.binaries = {}
        source = Path(cls.compiler_dir.name) / 'runtime.c'
        source.write_text('int main(void) { return 0; }\n')
        for arch, target in [('arm64', 'aarch64-apple-darwin'), ('x86_64', 'x86_64-apple-darwin')]:
            binary = source.with_name(target)
            run('xcrun', 'clang', '-arch', arch, str(source), '-o', str(binary))
            cls.binaries[target] = binary

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='ditch-signing-test-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        main = self.root / 'Ditch.app/Contents/MacOS'
        self.helper = self.root / 'Ditch.app/Contents/Library/LoginItems/The Ditch Runtime.app'
        helper_macos = self.helper / 'Contents/MacOS'
        self.resources = self.helper / 'Contents/Resources/RemoteRuntimes'
        self.manifest = self.resources / 'remote-artifacts.json'
        main.mkdir(parents=True)
        helper_macos.mkdir(parents=True)
        host = 'aarch64-apple-darwin' if platform.machine() == 'arm64' else 'x86_64-apple-darwin'
        for directory in [main, helper_macos]:
            for name in ['ditch_cli', f'ditchd-remote-{host}']:
                shutil.copy2(self.binaries[host], directory / name)
        shutil.copy2(self.binaries[host], helper_macos / 'ditchd')
        (self.helper / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': 'dev.ditch.signing-test',
            'CFBundleExecutable': 'ditchd', 'CFBundlePackageType': 'APPL',
            'CFBundleVersion': '1', 'CFBundleShortVersionString': '1.0',
        }))
        self.env = dict(os.environ, MAIN_MACOS=str(main), HELPER_APP=str(self.helper),
                        HELPER_MACOS=str(helper_macos), HELPER_RESOURCES=str(self.resources),
                        HOST_REMOTE_TARGET=host, EXPANDED_CODE_SIGN_IDENTITY='-')

    def stage_matrix(self):
        self.resources.mkdir(parents=True)
        artifacts = []
        for target in [*self.binaries, 'aarch64-unknown-linux-gnu', 'x86_64-unknown-linux-gnu']:
            path = self.resources / f'ditchd-{target}'
            if target in self.binaries:
                shutil.copy2(self.binaries[target], path)
            else:
                path.write_bytes(b'\x7fELF fixture for ' + target.encode())
            path.chmod(0o600)  # Match the archive's resource permissions.
            artifacts.append(dict(target=target, artifact=path.name, sha256=digest(path)))
        manifest = dict(edition='community', version='0.1.0', build_identifier='fixture-125',
                        community_revision='a' * 40, remote_runtime_protocol_version=1,
                        artifacts=artifacts)
        self.manifest.write_text(json.dumps(manifest))
        return manifest

    def assert_hardened(self, path):
        run('/usr/bin/codesign', '--verify', '--strict', str(path))
        details = run('/usr/bin/codesign', '-dv', '--verbose=4', str(path)).stderr
        self.assertIn('runtime)', details)

    def test_matrix_signatures_and_exact_manifest_checksums(self):
        original = self.stage_matrix()
        linux_bytes = {p.name: p.read_bytes() for p in self.resources.glob('*-linux-gnu')}
        run('/bin/sh', str(SCRIPT), env=self.env)
        updated = json.loads(self.manifest.read_text())
        self.assertEqual({k: v for k, v in original.items() if k != 'artifacts'},
                         {k: v for k, v in updated.items() if k != 'artifacts'})
        for before, after in zip(original['artifacts'], updated['artifacts']):
            path = self.resources / after['artifact']
            self.assertEqual(after['sha256'], digest(path))
            self.assertEqual(before['target'], after['target'])
            self.assertEqual(before['artifact'], after['artifact'])
            if after['target'].endswith('apple-darwin'):
                self.assert_hardened(path)
                self.assertNotEqual(before['sha256'], after['sha256'])
            else:
                self.assertEqual(before, after)
                self.assertEqual(linux_bytes[path.name], path.read_bytes())
        self.assert_hardened(self.helper)
        run('/usr/bin/codesign', '--verify', '--deep', '--strict', str(self.helper))

    def test_source_build_without_cross_built_resources_or_identity(self):
        self.env.pop('EXPANDED_CODE_SIGN_IDENTITY')
        run('/bin/sh', str(SCRIPT), env=self.env)
        self.assert_hardened(self.helper)

    def test_stale_artifact_fails_before_signing(self):
        original = self.stage_matrix()
        path = self.resources / original['artifacts'][0]['artifact']
        path.write_bytes(path.read_bytes() + b'tampered')
        result = subprocess.run(['/bin/sh', str(SCRIPT)], env=self.env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('checksum mismatch', result.stderr)
        self.assertEqual(original, json.loads(self.manifest.read_text()))

    def test_missing_manifest_fails(self):
        self.resources.mkdir(parents=True)
        result = subprocess.run(['/bin/sh', str(SCRIPT)], env=self.env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)


if __name__ == '__main__':
    unittest.main()
