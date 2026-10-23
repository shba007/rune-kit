"""Read-only integration check against a real rune-git-native artifact.

python scripts/git_gateway_conformance.py --rune-kit target/debug/rune-kit.exe --plugin /path/rune-git-native.exe
"""
import argparse
import hashlib
import json
from pathlib import Path
import queue
import subprocess
import threading


class Gateway:
    def __init__(self, rune, plugin, read_only=False, extra_args=()):
        args = [str(rune), 'run', str(plugin), *extra_args]
        if read_only:
            args.append('--read-only')
        self.process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, encoding='utf-8')
        self.output = queue.Queue()
        self.errors = []
        def receive():
            for line in self.process.stdout:
                self.output.put(line)
        def errors():
            for line in self.process.stderr:
                self.errors.append(line.rstrip())
        threading.Thread(target=receive, daemon=True).start()
        threading.Thread(target=errors, daemon=True).start()
        self.next_id = 0

    def request(self, method, params=None):
        self.next_id += 1
        self.send({'jsonrpc': '2.0', 'id': self.next_id, 'method': method, 'params': params or {}})
        response = json.loads(self.output.get(timeout=40))
        assert response.get('id') == self.next_id, response
        return response

    def send(self, value):
        self.process.stdin.write(json.dumps(value) + '\n')
        self.process.stdin.flush()

    def initialize(self):
        reply = self.request('initialize', {'protocolVersion': '2025-03-26', 'capabilities': {},
                    'clientInfo': {'name': 'rune-kit-cycle1', 'version': '1'}})
        assert reply['result']['protocolVersion'] == '2025-03-26', reply
        self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        return reply['result']['serverInfo']

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()


def run(rune, plugin):
    result = {'rune_version': subprocess.check_output([str(rune), '--version'], text=True).strip(),
              'plugin': str(plugin), 'plugin_sha256': hashlib.sha256(plugin.read_bytes()).hexdigest()}
    with subprocess.Popen([str(plugin)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True) as direct:
        requests = [
            {'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-03-26','capabilities':{},'clientInfo':{'name':'cycle1','version':'1'}}},
            {'jsonrpc':'2.0','id':2,'method':'info'},
            {'jsonrpc':'2.0','id':3,'method':'tools/list'},
        ]
        data, errors = direct.communicate(''.join(json.dumps(r)+'\n' for r in requests), timeout=40)
        replies = [json.loads(line) for line in data.splitlines()]
        result['plugin_initialize'] = replies[0]
        result['plugin_info'] = replies[1]['result']
        producer = replies[2]['result']
        producer_tools = producer['tools'] if isinstance(producer, dict) else producer
    normal = Gateway(rune, plugin)
    try:
        result['server'] = normal.initialize()
        reply = normal.request('tools/list')
        assert 'error' not in reply, reply
        tools = reply['result']['tools']
        assert tools, reply
        validator = next(t for t in tools if t['name'].endswith('__validate_commit_message'))
        assert validator['annotations']['readOnlyHint'] is True, validator
        assert all('annotations' in t for t in tools)
        write_tool = next(t for t in tools if t['annotations'].get('readOnlyHint') is not True
                          and any(effect in t['inputSchema'].get('x-rune-rules', {}).get('effects', [])
                                  for effect in ['write', 'delete']))
        for produced in producer_tools:
            forwarded = next(t for t in tools if t['name'].endswith('__'+produced['name']))
            assert forwarded['inputSchema'] == produced['inputSchema'], produced['name']
        result['schemas_preserved'] = len(producer_tools)
        result['normal_count'] = len(tools)
        result['validator'] = validator
    finally:
        normal.close()
    readonly = Gateway(rune, plugin, read_only=True)
    try:
        readonly.initialize()
        reply = readonly.request('tools/list')
        assert 'error' not in reply, reply
        tools = reply['result']['tools']
        assert tools and all(t['annotations'].get('readOnlyHint') is True for t in tools)
        selected = next(t for t in tools if t['name'] == validator['name'])
        schema = selected['inputSchema']
        assert schema['properties']['message']['type'] == 'string', schema
        assert set(schema.get('required', [])) <= {'message'}, schema
        arguments = {'message': 'fix: verify gateway annotation compatibility'}
        call = readonly.request('tools/call', {'name': selected['name'], 'arguments': arguments})
        assert 'error' not in call and not call['result'].get('isError', False), call
        assert json.loads(call['result']['content'][0]['text'])['valid'] is True, call
        result['validator_arguments'] = arguments
        result['validator_call'] = call
        refusal = readonly.request('tools/call', {'name': write_tool['name'], 'arguments': {}})
        assert refusal['error']['code'] == -32600, refusal
        result['forced_write_refusal'] = refusal
        result['readonly_count'] = len(tools)
    finally:
        readonly.close()
    result['stderr'] = normal.errors + readonly.errors
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--rune-kit', '--rune', dest='rune', type=Path, required=True)
    parser.add_argument('--plugin', type=Path, required=True)
    parser.add_argument('--output', type=Path)
    options = parser.parse_args()
    result = run(options.rune.resolve(), options.plugin.resolve())
    text = json.dumps(result, indent=2)
    if options.output:
        options.output.write_text(text+'\n', encoding='utf-8')
    print(text)
