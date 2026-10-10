#!/usr/bin/env python3
"""Установка/обновление программы по manifest без переноса состояния."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def entries(root):
    manifest = json.loads((root / 'distribution.json').read_text())
    if manifest['schema_version'] != 1:
        raise ValueError('Неизвестная схема manifest')
    for name, checksum in manifest['files'].items():
        path = Path(name)
        if path.is_absolute() or '..' in path.parts or name == 'distribution.json':
            raise ValueError('Недопустимый путь manifest')
        source = root / path
        if any(part.is_symlink() for part in [source, *source.parents[:len(path.parts)-1]]):
            raise ValueError('Symlink в дистрибутиве: ' + name)
        if digest(source) != checksum:
            raise ValueError('Хеш manifest не совпал: ' + name)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', required=True, help='Отдельный каталог только для программы')
    args = parser.parse_args()
    source = Path(__file__).resolve().parents[1]
    target = Path(args.prefix).expanduser().absolute()
    if target.is_symlink():
        parser.error('prefix не должен быть symlink')
    target = target.resolve()
    if target == source or target.is_relative_to(source) or source.is_relative_to(target):
        parser.error('prefix должен быть вне исходников')
    manifest = entries(source)
    if target.exists():
        if not target.is_dir() or not (target / 'distribution.json').is_file():
            parser.error('Каталог не принадлежит установщику')
        previous = entries(target)
        allowed = set(previous['files']) | {'distribution.json'}
        for path in target.rglob('*'):
            if path.is_symlink() or (path.is_file() and str(path.relative_to(target)) not in allowed):
                parser.error('Непредусмотренный файл в установке: ' + str(path.relative_to(target)))
    target.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix='.' + target.name + '-stage-', dir=target.parent))
    backup = None
    try:
        for name in [*manifest['files'], 'distribution.json']:
            dest = stage / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / name, dest)
        if target.exists():
            backup = Path(tempfile.mkdtemp(prefix='.' + target.name + '-backup-', dir=target.parent))
            backup.rmdir()
            os.replace(target, backup)
        try:
            os.replace(stage, target)
        except OSError:
            if backup is not None:
                os.replace(backup, target)
                backup = None
            raise
        print(json.dumps({'program_root': str(target), 'files': len(manifest['files']),
                          'backup': str(backup) if backup else None, 'state_copied': False}))
    finally:
        if stage.exists():
            shutil.rmtree(stage)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError) as exc:
        raise SystemExit(str(exc))
