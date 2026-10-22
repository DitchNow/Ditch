#!/usr/bin/env python3
"""Configuration isolation checks; never starts Ditch or reads user app data."""
import base64
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

COMMUNITY = Path(__file__).resolve().parents[1]
GENERATOR = COMMUNITY / 'scripts/configure-macos-environments.py'


class Isolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.configs = self.root / 'apps/macos/macos/Runner/Configs'
        self.configs.mkdir(parents=True)
        self.envdir = self.root / 'apps/macos/config'
        self.envdir.mkdir(parents=True)
        for env, host in [('production', 'relay.ditchnow.nl'),
                          ('staging', 'ditch-remote-relay-staging.matin-1a7.workers.dev')]:
            (self.envdir / f'.env.{env}').write_text(
                f'DITCH_DEPLOYMENT_ENVIRONMENT={env}\nDITCH_RELAY_ORIGIN=https://{host}\nDITCH_UPDATE_ALLOWED_HOSTS={host}\n')

    def generate(self, *args):
        return subprocess.run([sys.executable, str(GENERATOR), '--workspace', str(self.root), *args],
                              capture_output=True, text=True)

    def test_each_scheme_keeps_its_own_identity_and_unsigned_trust(self):
        (self.envdir / '.env.release.production').write_text('DITCH_SPARKLE_PUBLIC_ED_KEY=production-public-key\n')
        self.assertEqual(self.generate().returncode, 0)
        prod = (self.configs / 'Environment-production.xcconfig').read_text()
        stage = (self.configs / 'Environment-staging.xcconfig').read_text()
        self.assertIn('PRODUCT_BUNDLE_IDENTIFIER = ai.theditch.app\n', prod)
        self.assertIn('PRODUCT_BUNDLE_IDENTIFIER = ai.theditch.app.staging\n', stage)
        self.assertIn('PRODUCT_NAME = Ditch Staging\n', stage)
        self.assertIn('DITCH_SPARKLE_PUBLIC_ED_KEY = \n', stage)
        self.assertNotIn('production-public-key', stage)
        for environment, config in [('production', prod), ('staging', stage)]:
            self.assertIn(f'FLAVOR = {environment}\n', config)
            encoded = next(line.split(' = ', 1)[1] for line in config.splitlines() if line.startswith('DART_DEFINES = '))
            self.assertIn('=', encoded)  # Flutter 3.38's legacy assemble parser.
            decoded = dict(base64.b64decode(value).decode().split('=', 1) for value in encoded.split(','))
            self.assertEqual(decoded['DITCH_DEPLOYMENT_ENVIRONMENT'], environment)
            self.assertEqual(decoded['FLUTTER_APP_FLAVOR'], environment)
        self.assertEqual(self.generate('--check').returncode, 0)

    def test_stale_configuration_is_rejected(self):
        self.assertEqual(self.generate().returncode, 0)
        (self.configs / 'Environment-staging.xcconfig').write_text('PRODUCT_BUNDLE_IDENTIFIER = ai.theditch.app\n')
        self.assertNotEqual(self.generate('--check').returncode, 0)

    def test_only_staging_debug_uses_ad_hoc_signing(self):
        self.assertEqual(self.generate().returncode, 0)
        for environment in ['production', 'staging']:
            for mode in ['Debug', 'Release', 'Profile']:
                config = (self.configs / f'{mode}-{environment}.xcconfig').read_text()
                self.assertEqual('CODE_SIGN_IDENTITY = -' in config,
                                 environment == 'staging' and mode == 'Debug')
        roots = [COMMUNITY, COMMUNITY.parent]
        for root in roots:
            project = root / 'apps/macos/macos/Runner.xcodeproj/project.pbxproj'
            if not project.exists():
                continue
            objects = json.loads(subprocess.check_output(['plutil', '-convert', 'json', '-o', '-', str(project)]))['objects']
            for obj in objects.values():
                if obj['isa'] == 'XCBuildConfiguration' and obj['name'] == 'Debug-staging':
                    for key in obj['buildSettings']:
                        self.assertFalse(key.startswith(('CODE_SIGN_IDENTITY', 'DEVELOPMENT_TEAM', 'CODE_SIGN_STYLE')),
                                         'Target must inherit staging ad-hoc signing from its xcconfig')

    def test_cross_environment_relay_and_private_settings_are_rejected(self):
        path = self.envdir / '.env.staging'
        original = path.read_text()
        path.write_text(original.replace('ditch-remote-relay-staging.matin-1a7.workers.dev', 'relay.ditchnow.nl'))
        self.assertNotEqual(self.generate().returncode, 0)
        path.write_text(original + 'DITCH_OFFICIAL_BUILD_CREDENTIAL=never-export-this\n')
        result = self.generate()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('never-export-this', result.stdout + result.stderr)

    def test_all_process_and_storage_names_are_disjoint(self):
        identities = json.loads((COMMUNITY / 'config/deployment-identities.json').read_text())
        for key in identities['production']:
            self.assertNotEqual(identities['production'][key], identities['staging'][key], key)
        self.assertEqual(identities['production']['app_id'], 'ai.theditch.app')
        self.assertEqual(identities['production']['support_directory'], 'The Ditch')

    def test_xcode_targets_and_legacy_scheme_select_explicit_environments(self):
        roots = [COMMUNITY]
        if (COMMUNITY.parent / 'COMMUNITY_REVISION').exists():
            roots.append(COMMUNITY.parent)
        for root in roots:
            xcode = root / 'apps/macos/macos/Runner.xcodeproj'
            objects = json.loads(subprocess.check_output(['plutil', '-convert', 'json', '-o', '-', str(xcode / 'project.pbxproj')]))['objects']
            for obj in objects.values():
                if obj['isa'] == 'XCConfigurationList':
                    names = {objects[ref]['name'] for ref in obj['buildConfigurations']}
                    for env in ['production', 'staging']:
                        self.assertTrue({f'{mode}-{env}' for mode in ['Debug', 'Release', 'Profile']} <= names)
            for scheme, env in [('Runner', 'production'), ('production', 'production'), ('staging', 'staging')]:
                tree = ET.parse(xcode / f'xcshareddata/xcschemes/{scheme}.xcscheme')
                for element in tree.iter():
                    if 'buildConfiguration' in element.attrib:
                        self.assertTrue(element.attrib['buildConfiguration'].endswith('-' + env))
                self.assertIn('configure-macos-environments.py', tree.find('.//ActionContent').attrib['scriptText'])


if __name__ == '__main__':
    unittest.main()
