"""Minimal real stdio MCP client shared by contract tests and measurements."""
import json
import selectors
import subprocess


class Client:
    def __init__(self, binary, args=(), env=None):
        self.process = subprocess.Popen([str(binary), *map(str, args), 'mcp'], env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, bufsize=1)
        self.serial = 0
        self.request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                   'clientInfo': {'name': 'workflow-contract-client', 'version': '1'}})
        self.notify('notifications/initialized', {})

    def notify(self, method, params):
        self.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': method, 'params': params}) + '\n')
        self.process.stdin.flush()

    def request(self, method, params=None):
        self.serial += 1
        self.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': self.serial,
                                            'method': method, 'params': params or {}}) + '\n')
        self.process.stdin.flush()
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            if not selector.select(15):
                raise TimeoutError('MCP response timeout')
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError('MCP exited: ' + self.process.stderr.read())
        def reject_constant(token):
            raise AssertionError('Non-finite literal on JSON-RPC wire: ' + token)
        value = json.loads(line, parse_constant=reject_constant)
        assert value['id'] == self.serial and value['jsonrpc'] == '2.0', value
        return value

    def call(self, name, arguments):
        response = self.request('tools/call', {'name': name, 'arguments': arguments})
        if 'error' in response:
            raise RuntimeError(response)
        result = response['result']
        return result.get('structuredContent', {}).get('result', json.loads(result['content'][0]['text'])), result.get('isError', False)

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        if not self.process.stdin.closed:
            self.process.stdin.close()
        self.process.stdout.close()
        self.process.stderr.close()
