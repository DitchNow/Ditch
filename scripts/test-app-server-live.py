#!/usr/bin/env python3
"""Opt-in live adapter smoke test, with disposable project and Codex home.

Requires existing Codex authentication. Copies auth only into the temporary
home on the same machine and removes it after testing. Never uses a Ditch DB.
"""
import argparse
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--ssh', help='Existing SSH config alias; requires Python 3 and Codex')
parser.add_argument('--codex', default=shutil.which('codex'))
parser.add_argument('--manifest-path', default=str(Path(__file__).resolve().parents[1] / 'Cargo.toml'))
args = parser.parse_args()
root = Path(tempfile.mkdtemp(prefix='ditch-session-smoke-', dir='/tmp'))
env = dict(os.environ, DITCH_TEST_PROJECT_ROOT=str(root))
ssh = ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', args.ssh] if args.ssh else None
remote_created = False
try:
    if ssh:
        # Keep the /tmp spelling so cwd exists at the same path on macOS and Linux.
        setup = '''import os, pathlib, shutil
root=pathlib.Path(ROOT)
root.mkdir(mode=0o700)
try:
    home=root/'codex-home'; home.mkdir(mode=0o700)
    source=pathlib.Path(os.environ.get('CODEX_HOME', str(pathlib.Path.home()/'.codex')))/'auth.json'
    shutil.copyfile(source,home/'auth.json'); os.chmod(home/'auth.json',0o600)
except BaseException:
    shutil.rmtree(root); raise
'''.replace('ROOT', repr(str(root)))
        subprocess.run(ssh + ['sh -lc ' + shlex.quote('python3 -')], input=setup, text=True, check=True)
        remote_created = True
        command = 'export CODEX_HOME=' + shlex.quote(str(root / 'codex-home')) + '; exec codex app-server'
        wrapper = root / 'ssh-codex'
        wrapper.write_text('#!/bin/sh\nexec ' + shlex.join(ssh + ['sh -lc ' + shlex.quote(command)]) + '\n')
        wrapper.chmod(0o700)
        env['DITCH_TEST_CODEX_BINARY'] = str(wrapper)
    else:
        if not args.codex:
            raise RuntimeError('Codex executable not found; pass --codex')
        home = root / 'codex-home'
        home.mkdir(mode=0o700)
        source = Path(os.environ.get('CODEX_HOME', str(Path.home() / '.codex'))) / 'auth.json'
        shutil.copyfile(source, home / 'auth.json')
        (home / 'auth.json').chmod(0o600)
        env.update(DITCH_TEST_CODEX_BINARY=args.codex, DITCH_TEST_CODEX_HOME=str(home))
    result = subprocess.run(['cargo', 'test', '--manifest-path', args.manifest_path,
                             '-p', 'ditchd', '--lib', 'live_app_server_session_smoke',
                             '--', '--ignored', '--nocapture'], env=env)
    raise SystemExit(result.returncode)
finally:
    if remote_created:
        cleanup = 'import pathlib,shutil; p=pathlib.Path(' + repr(str(root)) + '); shutil.rmtree(p,ignore_errors=True)'
        subprocess.run(ssh + ['python3 -'], input=cleanup, text=True, check=True)
    shutil.rmtree(root)
