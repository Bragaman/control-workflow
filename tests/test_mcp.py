"""The established lifecycle contract driven through actual stdio MCP calls."""
import argparse
import json
from pathlib import Path
import test_control as fixture
from mcp_client import Client


def parser():
    result = argparse.ArgumentParser()
    for name in ('state-root', 'bindings-root', 'config', 'herdr-session', 'program-root'):
        result.add_argument('--' + name)
    commands = result.add_subparsers(dest='command', required=True)
    for spec in json.loads((fixture.ROOT / 'src/commands.json').read_text()):
        sub = commands.add_parser(spec['name'])
        for option in spec['options']:
            flags = option['flags']
            kwargs = dict(default=option['default'])
            if flags:
                kwargs.update(dest=option['name'], required=option['required'])
            if option['boolean']:
                kwargs['action'] = argparse.BooleanOptionalAction if len(flags) > 1 else 'store_true'
            else:
                if option['integer']: kwargs['type'] = int
                if option['multiple']:
                    kwargs.update(action='append') if flags else kwargs.update(nargs='+')
                if option['choices']: kwargs['choices'] = option['choices']
            sub.add_argument(*(flags[:1] or [option['name']]), **kwargs)
    return result


PARSER = parser()


class MCPControlTests(fixture.ControlTests):
    def setUp(self):
        if getattr(self, 'client', None):
            self.close_client()
        self.client = None
        self.client_signature = None
        self.addCleanup(self.close_client)
        super().setUp()

    def close_client(self):
        if self.client:
            self.client.close()
            self.client = None

    def cli(self, *args, ok=True, env=None):
        env = env or self.env
        config = env.get('CONTROL_PLANE_CONFIG')
        signature = (tuple(sorted(env.items())), Path(config).read_bytes() if config else None)
        if signature != self.client_signature:
            self.close_client()
            self.client = Client(fixture.BINARY, env=env)
            self.client_signature = signature
        parsed = vars(PARSER.parse_args(args))
        name = parsed.pop('command')
        if name.startswith('_'):
            return super().cli(*args, ok=ok, env=env)
        for key in ('state_root', 'bindings_root', 'config', 'herdr_session', 'program_root'):
            value = parsed.pop(key)
            if value is not None:
                # Explicit global selection is tested by the original direct CLI fixture.
                return super().cli(*args, ok=ok, env=env)
        arguments = {k: v for k, v in parsed.items() if v is not None}
        result, failed = self.client.call(name, arguments)
        self.assertEqual(failed, not ok, result)
        return result

    def test_mcp_protocol_and_schema_errors(self):
        names = {t['name'] for t in self.client.request('tools/list')['result']['tools']}
        self.assertTrue({'open', 'complete', 'review-request', 'memory-list'} <= names)
        self.assertFalse(any(n.startswith('_') for n in names))
        self.assertNotIn('install', names)
        self.assertEqual(self.client.request('ping')['result'], {})
        self.assertEqual(self.client.request('unknown')['error']['code'], -32601)
        self.assertEqual(self.client.request('tools/call', {'name': '_launch'})['error']['code'], -32602)
        value, failed = self.client.call('task-create', {'id': '../escape'})
        self.assertTrue(failed)
        self.assertIn('error', value)
        self.assertTrue(self.client.call('memory-list', {'project': 'lab', 'limit': 0})[1])
        self.assertTrue(self.client.call('projects', {'unexpected': True})[1])
        self.assertTrue(self.client.call('task-set', {'id': 'x', 'code': 'true'})[1])
        self.assertEqual(self.cli('projects')[0]['id'], 'lab')
