"""The established lifecycle contract driven through actual stdio MCP calls."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import unittest
import test_control as fixture
import test_native as native
from mcp_client import Client

PARSER = None


def setUpModule():
    global PARSER
    native.setUpModule()
    sys.path.insert(0, str(native.ORIGINAL.parent))
    spec = importlib.util.spec_from_file_location('mcp_reference_parser', native.ORIGINAL)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    PARSER = module.parser()


def tearDownModule():
    native.tearDownModule()


class MCPControlTests(native.NativeControlTests):
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
            self.client = Client(native.NATIVE, env=env)
            self.client_signature = signature
        parsed = vars(PARSER.parse_args(args))
        name = parsed.pop('command')
        if name.startswith('_'):
            return super().cli(*args, ok=ok, env=env)
        parsed.pop('func')
        for key in ('state_root', 'bindings_root', 'config', 'herdr_session'):
            value = parsed.pop(key)
            if value is not None:
                # Explicit global selection is tested by the original direct CLI fixture.
                return super().cli(*args, ok=ok, env=env)
        if name in ('projects', 'tasks'):
            parsed.pop('kind')
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
