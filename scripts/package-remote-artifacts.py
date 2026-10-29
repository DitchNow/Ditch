#!/usr/bin/env python3
"""Assemble verified remote resources before the existing signing step."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys

TARGETS = {
    'aarch64-apple-darwin', 'x86_64-apple-darwin',
    'aarch64-unknown-linux-gnu', 'x86_64-unknown-linux-gnu',
}


def digest(path):
    with path.open('rb') as stream:
        result = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            result.update(block)
        return result.hexdigest()


def bundle(source, destination, native, target, metadata):
    if target not in TARGETS or not target.endswith('-apple-darwin'):
        raise ValueError('unsupported native Mac target')
    entries = []
    supplied = {}
    manifest = source / 'remote-artifacts.json'
    if manifest.is_file():
        supplied = json.loads(manifest.read_text())
        for key, expected in metadata.items():
            actual = supplied.get(key, 'production' if key == 'deployment_environment' else None)
            if actual != expected:
                raise ValueError(f'remote artifacts have mismatched {key}; rebuild the selected targets')
        seen = set()
        for entry in supplied['artifacts']:
            remote_target = entry['target']
            name = entry['artifact']
            if remote_target not in TARGETS or name != f'ditchd-{remote_target}' or remote_target in seen:
                raise ValueError('invalid or duplicate remote artifact target/filename')
            seen.add(remote_target)
            binary = source / name
            if digest(binary) != entry['sha256']:
                raise ValueError(f'remote artifact checksum mismatch: {name}')
            entries.append((remote_target, binary))
    elif str(source) != '/dev/null':
        raise ValueError(f'missing remote-artifacts.json in {source}; rebuild the selected targets')

    # Xcode already built this executable with the desktop environment/identity.
    if not any(item[0] == target for item in entries):
        entries.append((target, native))
    # Validate all inputs before replacing the dedicated generated resource folder.
    for _, binary in entries:
        if not binary.is_file():
            raise ValueError(f'missing remote binary: {binary}')
    if source.resolve() == destination.resolve():
        raise ValueError('artifact source and bundle destination must differ')
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    # Retain release provenance (for example community_revision) when supplied.
    output = dict(supplied)
    output.update(metadata, artifacts=[])
    for remote_target, binary in entries:
        name = f'ditchd-{remote_target}'
        copied = destination / name
        shutil.copyfile(binary, copied)
        copied.chmod(0o600)
        output['artifacts'].append(dict(target=remote_target, artifact=name, sha256=digest(copied)))
    (destination / 'remote-artifacts.json').write_text(json.dumps(output, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('source', 'destination', 'native'):
        parser.add_argument('--' + key, required=True, type=Path)
    for key in ('target', 'environment', 'edition', 'version', 'build'):
        parser.add_argument('--' + key, required=True)
    parser.add_argument('--protocol', required=True, type=int)
    args = parser.parse_args()
    bundle(args.source, args.destination, args.native, args.target, dict(
        deployment_environment=args.environment, edition=args.edition,
        version=args.version, build_identifier=args.build,
        remote_runtime_protocol_version=args.protocol,
    ))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError) as error:
        sys.exit(f'error: {error}')
