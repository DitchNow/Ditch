#!/usr/bin/env python3
"""Isolated Codex/Ollama check. --config-only makes no inference request.

Without --config-only, fixes a disposable fixture, checks conversation-only
recall after a child restart, compacts, and checks recall again. Optional token:
DITCH_OLLAMA_SMOKE_TOKEN environment variable.
This harness declines elevated approvals; interactive approval acceptance is
a separate Ditch UI check. Never reads or changes the user's Codex home.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time
import uuid


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--codex', default='codex')
    p.add_argument('--endpoint', required=True)
    p.add_argument('--model', required=True)
    p.add_argument('--output', required=True, type=Path)
    p.add_argument('--config-only', action='store_true')
    p.add_argument('--context-window', type=int, default=65536,
                   help='Must match the model serving configuration (default: 65536)')
    a = p.parse_args()
    root = a.output.resolve() / ('ollama-smoke-' + uuid.uuid4().hex)
    project, home = root / 'project', root / 'codex-home'
    project.mkdir(parents=True)
    home.mkdir(mode=0o700)
    (project / 'fixture.py').write_text('def add(a, b):\n    return a - b\n')
    (project / 'test_fixture.py').write_text('from fixture import add\nassert add(2, 3) == 5\n')
    key = 'ditch_ollama_' + uuid.uuid4().hex
    endpoint = a.endpoint.rstrip('/').removesuffix('/v1')
    if not 8192 <= a.context_window <= 262144:
        p.error('--context-window must be between 8192 and 262144')
    config = {'model_provider': key, 'model': a.model, 'web_search': 'disabled',
              'model_context_window': a.context_window,
              'model_auto_compact_token_limit': a.context_window * 3 // 4,
              'model_auto_compact_token_limit_scope': 'total',
              'features.apps': False, 'skills.max_context_tokens': 1024,
              'tool_output_token_limit': 4096}
    marker = 'CEDAR-' + uuid.uuid4().hex
    for k, v in dict(name='Ditch smoke', base_url=endpoint+'/v1', wire_api='responses', requires_openai_auth=False, supports_websockets=False).items():
        config[f'model_providers.{key}.{k}'] = v
    token = os.environ.get('DITCH_OLLAMA_SMOKE_TOKEN')
    if token: config[f'model_providers.{key}.env_key'] = 'DITCH_OLLAMA_SMOKE_TOKEN'
    env = dict(os.environ, CODEX_HOME=str(home))
    for name in ('OPENAI_API_KEY', 'CODEX_API_KEY'): env.pop(name, None)
    command = [a.codex]
    for k, v in config.items(): command.extend(['-c', k+'='+json.dumps(v)])
    command.append('app-server')
    version = subprocess.check_output([a.codex, '--version'], text=True).strip()

    def run(thread_id=None, compact=False):
        log = root / 'app-server.stderr'
        with log.open('a') as stderr:
            child = subprocess.Popen(command, cwd=project, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, text=True)
        messages = queue.Queue()
        def read():
            for line in child.stdout:
                try: messages.put(json.loads(line))
                except ValueError: pass
            messages.put(None)
        threading.Thread(target=read, daemon=True).start()
        def send(v): child.stdin.write(json.dumps(v)+'\n'); child.stdin.flush()
        def receive(deadline):
            remaining = deadline-time.monotonic()
            if remaining <= 0: raise TimeoutError('Codex timed out')
            v = messages.get(timeout=remaining)
            if v is None: raise RuntimeError('Codex exited; inspect isolated stderr log')
            if v.get('error'): raise RuntimeError('Codex RPC failed; inspect isolated stderr log')
            return v
        def call(id_, method, params):
            send(dict(id=id_, method=method, params=params))
            deadline = time.monotonic()+30
            while True:
                v = receive(deadline)
                if v.get('id') == id_: return v['result']
        try:
            call(1, 'initialize', {'clientInfo': {'name': 'ditch_ollama_smoke', 'version': '1.0'}, 'capabilities': {'experimentalApi': True}})
            send({'method': 'initialized', 'params': {}})
            params = dict(cwd=str(project), modelProvider=key, model=a.model, approvalPolicy='on-request', sandbox='workspace-write')
            if thread_id: params['threadId'] = thread_id
            if a.config_only: params['ephemeral'] = True
            result = call(2, 'thread/resume' if thread_id else 'thread/start', params)
            if result['thread'].get('modelProvider') != key: raise RuntimeError('Provider mismatch')
            native_id = result['thread']['id']
            if a.config_only: return native_id
            if compact:
                call(4, 'thread/compact/start', {'threadId': native_id})
                deadline = time.monotonic()+180
                while True:
                    v = receive(deadline)
                    if v.get('method') == 'turn/completed':
                        if v['params']['turn']['status'] != 'completed':
                            raise RuntimeError('Compaction failed')
                        break
            prompt = (
                f'Remember this conversation marker: {marker}. Never write it to a file or use tools to store it. '
                'Read fixture.py and test_fixture.py. Fix add and run python3 test_fixture.py. Work only in this directory.'
                if not thread_id else
                'State the exact conversation marker I asked you to remember, using only conversation history. '
                'Then read fixture.py and run python3 test_fixture.py to verify the previous fix.'
            )
            call(3, 'turn/start', {'threadId': native_id, 'input': [{'type': 'text', 'text': prompt}]})
            deadline = time.monotonic()+180
            messages_text = []
            while True:
                v = receive(deadline)
                method = v.get('method', '')
                if method == 'item/completed':
                    item = v.get('params', {}).get('item', {})
                    if item.get('type') == 'agentMessage':
                        messages_text.append(item.get('text', ''))
                if 'id' in v and method:
                    if method.endswith('/requestApproval'): send({'id': v['id'], 'result': {'decision': 'decline'}})
                    else: send({'id': v['id'], 'error': {'code': -32601, 'message': 'Unsupported smoke request'}})
                if method == 'turn/completed':
                    if v['params']['turn']['status'] != 'completed': raise RuntimeError('Turn failed or interrupted')
                    if thread_id and marker not in '\n'.join(messages_text):
                        raise RuntimeError('Conversation marker was lost after resume/compaction')
                    if any(marker in path.read_text(errors='replace') for path in project.rglob('*') if path.is_file()):
                        raise RuntimeError('Model stored the marker in a file; recall check is invalid')
                    subprocess.run(['python3', 'test_fixture.py'], cwd=project, check=True)
                    return native_id
        finally:
            child.terminate()
            try: child.wait(timeout=5)
            except subprocess.TimeoutExpired: child.kill(); child.wait()
            if token: log.write_text(log.read_text().replace(token, '[redacted]'))
    native_id = run()
    if not a.config_only:
        run(native_id)
        run(native_id, compact=True)
    report = dict(codex_version=version, config_only=a.config_only, context_window=a.context_window,
                  provider_binding_verified=True, coding_and_resume_verified=not a.config_only,
                  recall_after_restart_and_compaction_verified=not a.config_only,
                  approval_grant_verified=False, thread_id=native_id)
    (root/'result.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))
    print('Evidence:', root)


if __name__ == '__main__': main()
