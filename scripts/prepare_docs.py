#!/usr/bin/env python3
"""Stage the public documentation without maintaining a second Markdown tree."""
import json
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]


def main():
    destination = ROOT / 'target/docs-src'
    if destination.exists():
        shutil.rmtree(destination)
    manifest = json.loads((ROOT / 'distribution.json').read_text())['files']
    for name in sorted(manifest):
        if name not in ('README.md', 'LICENSE') and not name.startswith(
            ('docs/guide/',)
        ):
            continue
        source = ROOT / name
        if source.is_symlink():
            raise ValueError('Documentation source must not be a symlink: ' + name)
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    print(destination)


if __name__ == '__main__':
    main()
