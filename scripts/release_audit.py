#!/usr/bin/env python3
"""Положительный список, хеши, локальные ссылки и признаки личных данных."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote, urlsplit

SCRIPTS = ('control', 'completion_git', 'install_skills', 'artifact_links', 'herdr_codex',
           'runtime_config', 'platform_support', 'agent_adapters', 'herdr_claude', 'install',
           'doctor', 'release_audit')
DOCS = ('worker', 'planning', 'control-entry', 'git-policy', 'recovery', 'runtime-layout',
        'install', 'architecture', 'compatibility')
SKILLS = ('list', 'project', 'task', 'start', 'review', 'complete', 'memory', 'sync')
ALLOWLIST = ({'README.md', 'AGENTS.md', 'CLAUDE.md', 'LICENSE.pending', '.gitignore',
              'config/defaults.json', 'examples/config-codex.json', 'examples/config-claude.json',
              'examples/project.json', 'examples/task.md', '.github/workflows/check.yml',
              'skills/control-sync/references/checks.md'} |
             {'scripts/' + name + '.py' for name in SCRIPTS} |
             {'docs/' + name + '.md' for name in DOCS} |
             {'skills/control-' + name + '/SKILL.md' for name in SKILLS} |
             {'tests/test_' + name + '.py' for name in ('control', 'runtime', 'public')})
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
    for file in root.rglob('*'):
        rel = str(file.relative_to(root))
        if rel == '.git' or rel.startswith('.git/'):
            continue
        if file.is_symlink():
            raise ValueError('Symlink: ' + rel)
        if file.is_file():
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
            if set(names) - ALLOWLIST - {'distribution.json'}:
                raise ValueError('Посторонние пути в истории ' + commit)
            for name in names:
                inspect(name, subprocess.check_output(['git', '-C', str(root), 'show', commit + ':' + name]))
    print(json.dumps({'files': len(files), 'skills': len(SKILLS), 'symlinks': 0,
                      'history_commits_checked': len(commits), 'privacy_pattern_matches': 0,
                      'local_links': 'passed', 'license': 'pending'}, ensure_ascii=False))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as exc:
        raise SystemExit(str(exc))
