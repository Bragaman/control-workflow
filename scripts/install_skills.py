#!/usr/bin/env python3
"""Подключить, проверить или снять только ссылки восьми скиллов этой установки."""
import argparse
from pathlib import Path

NAMES = ('control-list', 'control-project', 'control-task', 'control-start',
         'control-review', 'control-complete', 'control-memory', 'control-sync')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', required=True)
    parser.add_argument('--action', choices=('install', 'check', 'remove'), default='install')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    destination = Path(args.destination).expanduser().resolve()
    pairs = [(root / 'skills' / name, destination / name) for name in NAMES]
    for source, target in pairs:
        if not (source / 'SKILL.md').is_file():
            parser.error('Нет исходника: ' + str(source))
        present = target.exists() or target.is_symlink()
        owned = target.is_symlink() and target.resolve() == source.resolve()
        if present and not owned:
            parser.error('Чужой файл или ссылка: ' + str(target))
        if args.action == 'check' and not owned:
            parser.error('Не подключён: ' + str(target))
    if args.action == 'install':
        destination.mkdir(parents=True, exist_ok=True)
    for source, target in pairs:
        if args.action == 'install' and not target.is_symlink():
            target.symlink_to(source, target_is_directory=True)
        elif args.action == 'remove' and target.is_symlink():
            target.unlink()
        print(str(target))


if __name__ == '__main__':
    main()
