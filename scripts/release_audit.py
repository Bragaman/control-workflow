#!/usr/bin/env python3
"""Положительный список, хеши, локальные ссылки и признаки личных данных."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote, urlsplit

SCRIPTS = ('control', 'completion_git', 'install_skills', 'artifact_links', 'herdr_codex',
           'runtime_config', 'platform_support', 'agent_adapters', 'herdr_claude', 'install',
           'doctor', 'release_audit', 'agent_review')
DOCS = ('worker', 'planning', 'control-entry', 'git-policy', 'recovery', 'runtime-layout',
        'agent-review', 'install', 'architecture', 'compatibility', 'questions', 'task-profile')
SKILLS = ('list', 'project', 'task', 'start', 'review', 'complete', 'memory', 'sync')
ALLOWLIST = ({'README.md', 'AGENTS.md', 'CLAUDE.md', 'LICENSE', '.gitignore',
              'config/defaults.json', 'examples/config-codex.json', 'examples/config-claude.json',
              'examples/project.json', 'examples/task.md', '.github/workflows/check.yml',
              'skills/control-sync/references/checks.md'} |
             {'scripts/' + name + '.py' for name in SCRIPTS} |
             {'docs/' + name + '.md' for name in DOCS} |
             {'skills/control-' + name + '/SKILL.md' for name in SKILLS} |
             {'tests/test_' + name + '.py' for name in ('control', 'runtime', 'public')})
NATIVE_FILES = {'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
                '.github/workflows/rust.yml', 'docs/rust.md', 'tests/native.rs',
                'tests/test_native.py', 'tests/test_mcp.py', 'tests/test_interop.py', 'tests/mcp_client.py'} | {
    'src/' + name for name in ('args.rs', 'casefold.rs', 'commands.json', 'config.rs', 'core.rs',
                               'external.rs', 'git.rs', 'lib.rs', 'lifecycle.rs',
                               'main.rs', 'mcp.rs', 'resources.rs', 'review.rs', 'shipping.rs', 'store.rs')}
ALLOWLIST |= NATIVE_FILES
# The original license placeholder remains valid only in preserved historical trees.
HISTORY_ALLOWLIST = ALLOWLIST | {'LICENSE.pending'}
PRIVATE = re.compile(r'/home/(?:dm|[a-z][a-z0-9_-]*)/|run-[a-f0-9]{12}|'
                     r'-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----|'
                     r'\bAKIA[A-Z0-9]{16}\b|\bsk-[A-Za-z0-9_-]{24,}')


def inspect(name, data):
    if PRIVATE.search(data.decode('utf-8')):
        raise ValueError('Признак личных данных/секрета: ' + name)


def check_links(root, name):
    if not name.endswith('.md'):
        return
    text = (root / name).read_text()
    text = re.sub(r'```[^\n]*\n.*?```', '', text, flags=re.S)
    text = re.sub(r'`[^`\n]*`', '', text)
    for raw in re.findall(r'\]\((<[^>]+>|[^\s)]+)', text):
        target = unquote(urlsplit(raw.strip('<>')).path)
        if not target or urlsplit(raw.strip('<>')).scheme:
            continue
        destination = (root / name).parent / target
        if not destination.resolve().is_relative_to(root.resolve()) or not destination.exists():
            raise ValueError('Неверная локальная ссылка: ' + name + ' -> ' + raw)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--write-manifest', action='store_true')
    a = p.parse_args()
    root = Path(__file__).resolve().parents[1]
    actual = set()
    for directory, folders, names in os.walk(root, followlinks=False):
        relative = Path(directory).relative_to(root)
        folders[:] = [name for name in folders if name != '__pycache__'
                      and not (relative == Path('.') and name in ('.git', 'target'))]
        for name in [*folders, *names]:
            file = Path(directory) / name
            rel = str(file.relative_to(root))
            if file.is_symlink():
                raise ValueError('Symlink: ' + rel)
            if file.is_file() and rel != '.git':
                actual.add(rel)
    missing = ALLOWLIST - actual
    unexpected = actual - ALLOWLIST - {'distribution.json'}
    if missing or unexpected:
        raise ValueError('Отсутствуют: ' + ', '.join(sorted(missing)) + '; лишние: ' + ', '.join(sorted(unexpected)))
    files = {}
    for name in sorted(ALLOWLIST):
        data = (root / name).read_bytes()
        inspect(name, data)
        check_links(root, name)
        files[name] = hashlib.sha256(data).hexdigest()
    manifest = {'schema_version': 1, 'files': files}
    if a.write_manifest:
        (root / 'distribution.json').write_text(json.dumps(manifest, indent=2) + '\n')
    else:
        if json.loads((root / 'distribution.json').read_text()) != manifest:
            raise ValueError('Хеши distribution.json не совпадают')
    # Check every historical tree if Git is available; an installed copy has no Git metadata.
    git = subprocess.run(['git', '-C', str(root), 'rev-parse', '--show-toplevel'],
                         text=True, capture_output=True)
    commits = []
    if git.returncode == 0 and Path(git.stdout.strip()).resolve() == root:
        commits = subprocess.check_output(['git', '-C', str(root), 'rev-list', '--all'], text=True).splitlines()
        for commit in commits:
            names = subprocess.check_output(['git', '-C', str(root), 'ls-tree', '-r', '--name-only', commit], text=True).splitlines()
            if set(names) - HISTORY_ALLOWLIST - {'distribution.json'}:
                raise ValueError('Посторонние пути в истории ' + commit)
            for name in names:
                inspect(name, subprocess.check_output(['git', '-C', str(root), 'show', commit + ':' + name]))
    print(json.dumps({'files': len(files), 'skills': len(SKILLS), 'symlinks': 0,
                      'history_commits_checked': len(commits), 'privacy_pattern_matches': 0,
                      'local_links': 'passed', 'license': 'MIT'}, ensure_ascii=False))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as exc:
        raise SystemExit(str(exc))
