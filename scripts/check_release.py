#!/usr/bin/env python3
"""Validate release tag, Cargo metadata and the native executable."""
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path


def main():
    if sys.argv[1] == '--package':
        root = Path(sys.argv[2])
        manifest = json.loads((root / 'distribution-rust.json').read_text())
        if manifest['schema_version'] != 1 or manifest['implementation'] != 'rust':
            raise ValueError('Invalid package manifest')
        files = manifest['files']
        actual = {str(p.relative_to(root)) for p in root.rglob('*') if p.is_file()}
        if any(p.is_symlink() for p in root.rglob('*')) or actual != set(files) | {'distribution-rust.json'}:
            raise ValueError('Unexpected package contents')
        for name, expected in files.items():
            path = Path(name)
            if path.is_absolute() or '..' in path.parts or hashlib.sha256((root / path).read_bytes()).hexdigest() != expected:
                raise ValueError(f'Invalid package file: {name}')
        binary = subprocess.check_output([str(root / 'bin/control-workflow'), '--version'], text=True).strip()
        if binary != f"control-workflow {manifest['version']}":
            raise ValueError('Package binary and manifest versions differ')
        print('Package manifest verified')
        return
    tag = sys.argv[1]
    if not re.fullmatch(r'v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-rc\.(0|[1-9]\d*))?', tag):
        raise ValueError(f'Invalid release tag: {tag}')
    version = tag[1:]
    cargo = tomllib.loads(Path('Cargo.toml').read_text())['package']['version']
    lock = tomllib.loads(Path('Cargo.lock').read_text())
    versions = [p['version'] for p in lock['package'] if p['name'] == 'control-workflow']
    binary = subprocess.check_output(['target/release/control-workflow', '--version'], text=True).strip()
    if cargo != version or versions != [version] or binary != f'control-workflow {version}':
        raise ValueError(f'Version mismatch: tag={version}, Cargo={cargo}, lock={versions}, binary={binary}')
    print(f'Validated {tag}')


if __name__ == '__main__':
    main()
