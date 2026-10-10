#!/usr/bin/env python3
"""Предварительная проверка; herdr session читает только по явному параметру."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
sys.dont_write_bytecode = True
import runtime_config
import platform_support
import agent_adapters


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--state-root'); p.add_argument('--bindings-root'); p.add_argument('--config')
    p.add_argument('--herdr-session')
    a = p.parse_args()
    root, bindings, settings, machine, config = runtime_config.load(a.state_root, a.config, a.bindings_root)
    checks = {'platform': sys.platform, 'python': sys.version.split()[0],
              'state_root': str(root), 'bindings_root': str(bindings),
              'program_root': str(runtime_config.PROGRAM_ROOT), 'config': str(config) if config else None,
              'adapter': settings['adapter'], 'capabilities': agent_adapters.select(settings['adapter']).CAPABILITIES}
    for key, directory in [('state_writable', root), ('bindings_writable', bindings)]:
        directory.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryFile(dir=directory) as f:
            f.write(b'check'); f.flush()
        checks[key] = True
    checks['process_identity'] = platform_support.process_identity(os.getpid()) is not None
    checks['machine_identity'] = bool(platform_support.machine_id())
    for name in ['git', machine.get('herdr_executable', 'herdr'), agent_adapters.select(settings['adapter']).AGENT_KIND]:
        exe = shutil.which(name)
        checks[name] = {'found': bool(exe)}
        if exe:
            result = subprocess.run([exe, '--version'], capture_output=True, text=True, timeout=10)
            checks[name]['version'] = result.stdout.strip().splitlines()[:1]
    if a.herdr_session:
        result = subprocess.run([machine.get('herdr_executable', 'herdr'), '--session', a.herdr_session,
                                 'status', 'server'], capture_output=True, text=True, timeout=10)
        checks['herdr_server'] = {'session': a.herdr_session, 'reachable': result.returncode == 0}
    print(json.dumps(checks, ensure_ascii=False))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as exc:
        raise SystemExit(str(exc))
