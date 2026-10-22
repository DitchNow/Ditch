#!/usr/bin/env python3
"""Generate independent Xcode configurations from each environment's public .env.

No credentials are read. Build phases also use --check to fail closed on stale
configuration instead of silently using the other environment's last build.
"""
import argparse
import base64
import json
import shlex
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--workspace', type=Path, default=Path(__file__).resolve().parents[1])
parser.add_argument('--check', action='store_true')
parser.add_argument('--build-environment', choices=['staging', 'production'])
args = parser.parse_args()
root = args.workspace.resolve()
shared = Path(__file__).resolve().parents[1]
identities = json.loads((shared / 'config/deployment-identities.json').read_text())
configs = root / 'apps/macos/macos/Runner/Configs'

def read_env(path):
    allowed = {'DITCH_DEPLOYMENT_ENVIRONMENT', 'DITCH_RELAY_ORIGIN', 'DITCH_UPDATE_ALLOWED_HOSTS',
               'DITCH_SPARKLE_PUBLIC_ED_KEY', 'DITCH_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64',
               'DITCH_COMMUNITY_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64', 'DITCH_APPLE_TEAM_ID',
               'DITCH_COMMUNITY_BUILD_SEQUENCE'}
    values = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        key, value = line.split('=', 1)
        if key not in allowed or any(c in value for c in ['\n', '\r', '$', '"']):
            raise ValueError(f'Invalid public build configuration: {path}')
        values[key] = value
    return values

def write(path, value):
    if args.check:
        if not path.exists() or path.read_text() != value:
            raise SystemExit(f'Stale environment configuration: {path}. Run configure-macos-environments.py.')
    elif not path.exists() or path.read_text() != value:
        path.write_text(value)

swift = '// Generated from config/deployment-identities.json. Do not edit.\nimport Foundation\n\nenum DitchIdentity {\n'
for environment in ['staging', 'production']:
    identity = identities[environment]
    swift += '#if DITCH_STAGING\n' if environment == 'staging' else '#else\n'
    swift += f'  static let environment = "{environment}"\n'
    for key, value in identity.items():
        swift += f'  static let {key} = {json.dumps(value)}\n'
swift += '#endif\n  static let supportRelativePath = "Library/Application Support/" + support_directory\n}\n'
write(root / 'apps/macos/macos/Runner/DeploymentIdentity.swift', swift)

for environment, identity in identities.items():
    write(configs / f'Identity-{environment}.sh', '\n'.join(
        f'{key}={shlex.quote(value)}' for key, value in {
            'DITCH_APP_BUNDLE_ID': identity['app_id'],
            'DITCH_RUNTIME_BUNDLE_ID': identity['runtime_id'],
            'DITCH_HELPER_NAME': identity['helper_name'],
            'DITCH_APP_NAME': identity['app_name'],
            'DITCH_SWIFT_IDENTITY_FLAGS': '-D DITCH_STAGING' if environment == 'staging' else '',
        }.items()) + '\n')
    values = read_env(root / f'apps/macos/config/.env.{environment}')
    if values['DITCH_DEPLOYMENT_ENVIRONMENT'] != environment:
        raise ValueError('Environment file/identity mismatch')
    origin = values['DITCH_RELAY_ORIGIN']
    if environment == 'production' and origin != 'https://relay.ditchnow.nl':
        raise ValueError('Production Relay mismatch')
    if environment == 'staging' and origin != 'https://ditch-remote-relay-staging.matin-1a7.workers.dev':
        raise ValueError('Staging Relay mismatch')
    # An unsigned staging configuration must never inherit production trust keys
    # through AppInfo.xcconfig or Xcode's project-level build settings.
    values.update({key: '' for key in ['DITCH_SPARKLE_PUBLIC_ED_KEY',
                  'DITCH_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64',
                  'DITCH_COMMUNITY_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64', 'DITCH_APPLE_TEAM_ID']})
    values['DITCH_COMMUNITY_BUILD_SEQUENCE'] = '0'
    trust = root / f'apps/macos/config/.env.release.{environment}'
    if trust.exists():
        values.update(read_env(trust))
    if args.build_environment == environment:
        import os
        for key, value in values.items():
            if os.environ.get(key, '') != value:
                raise SystemExit(f'Xcode has stale {key} for {environment}. Regenerate the configuration and rebuild.')
    lines = ['// Generated from this environment public .env files. Do not edit.']
    # Avoid xcconfig interpreting HTTPS's double slash as a comment.
    for key, value in values.items():
        lines.append(f'{key} = {value.replace("//", "/$()/")}')
    # Supply the flavor explicitly for direct Xcode builds, too. Including its
    # Dart define also avoids Flutter 3.38 assemble rejecting an entirely
    # unpadded base64 define list while parsing its legacy define arguments.
    dart_values = {k: values[k] for k in ['DITCH_DEPLOYMENT_ENVIRONMENT', 'DITCH_RELAY_ORIGIN']}
    dart_values['FLUTTER_APP_FLAVOR'] = environment
    lines += [f'PRODUCT_NAME = {identity["app_name"]}',
              f'FLAVOR = {environment}',
              f'PRODUCT_BUNDLE_IDENTIFIER = {identity["app_id"]}',
              f'DITCH_RUNTIME_BUNDLE_ID = {identity["runtime_id"]}',
              f'DITCH_HELPER_NAME = {identity["helper_name"]}',
              f'DITCH_URL_SCHEME = {identity["url_scheme"]}',
              f'DITCH_URL_NAME = {identity["url_name"]}',
              'SWIFT_ACTIVE_COMPILATION_CONDITIONS = $(inherited)' + (' DITCH_STAGING' if environment == 'staging' else ''),
              'DART_DEFINES = ' + ','.join(base64.b64encode(f'{k}={v}'.encode()).decode() for k, v in dart_values.items())]
    write(configs / f'Environment-{environment}.xcconfig', '\n'.join(lines) + '\n')
    for mode in ['Debug', 'Release', 'Profile']:
        signing = ('CODE_SIGN_IDENTITY = -\nCODE_SIGN_IDENTITY[sdk=macosx*] = -\n'
                   'CODE_SIGN_STYLE = Manual\nDEVELOPMENT_TEAM =\n'
                   'DEVELOPMENT_TEAM[sdk=macosx*] =\nPROVISIONING_PROFILE_SPECIFIER =\n'
                   if environment == 'staging' and mode == 'Debug' else '')
        write(configs / f'{mode}-{environment}.xcconfig',
              f'#include "{ "Debug" if mode == "Debug" else "Release" }.xcconfig"\n#include "AppInfo.xcconfig"\n#include "Environment-{environment}.xcconfig"\n' + signing)
