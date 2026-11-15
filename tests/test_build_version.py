"""Exercise build metadata against a real temporary Git repository."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BuildVersionTest(unittest.TestCase):
    def test_git_release_dirty_and_fallback(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            script = root / 'build-script'
            subprocess.run(['rustc', '--edition=2024', str(ROOT / 'build.rs'), '-o', str(script)], check=True)
            source = root / 'source'
            source.mkdir()
            env = {**os.environ, 'CARGO_PKG_VERSION': '0.1.0', 'GIT_CEILING_DIRECTORIES': str(root)}
            env.pop('CONTROL_WORKFLOW_RELEASE_TAG', None)

            def run(*args):
                return subprocess.run(args, cwd=source, env=env, text=True, capture_output=True, check=True).stdout

            self.assertIn('commit unknown', run(str(script)))
            # A source archive inside an unrelated checkout must not use its HEAD.
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-C', str(root), '-c', 'user.name=Test', '-c',
                            'user.email=test@example.invalid', 'commit', '--allow-empty', '-qm', 'parent'], check=True)
            env['GIT_CEILING_DIRECTORIES'] = str(root.parent)
            self.assertIn('commit unknown', run(str(script)))
            run('git', 'init', '-q')
            run('git', 'config', 'user.name', 'Test')
            run('git', 'config', 'user.email', 'test@example.invalid')
            (source / 'file').write_text('first')
            run('git', 'add', 'file')
            run('git', 'commit', '-qm', 'first')
            first = run('git', 'rev-parse', 'HEAD').strip()
            self.assertIn(f'commit {first})', run(str(script)))
            (source / 'file').write_text('changed')
            self.assertIn(f'commit {first}-dirty)', run(str(script)))
            run('git', 'add', 'file')
            run('git', 'commit', '-qm', 'second')
            second = run('git', 'rev-parse', 'HEAD').strip()
            self.assertNotEqual(first, second)
            self.assertIn(f'commit {second})', run(str(script)))
            run('git', 'tag', 'v0.1.0')
            env['CONTROL_WORKFLOW_RELEASE_TAG'] = 'v0.1.0'
            self.assertIn('CONTROL_WORKFLOW_BUILD_VERSION=0.1.0\n', run(str(script)))
            (source / 'new').write_text('dirty')
            self.assertNotEqual(subprocess.run([str(script)], cwd=source, env=env, capture_output=True).returncode, 0)
