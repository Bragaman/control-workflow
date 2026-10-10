"""Read-only program resources, user preferences and per-machine bindings."""
import copy
import json
import os
import sys
import agent_adapters
from pathlib import Path

PROGRAM_ROOT = Path(__file__).resolve().parents[1]


def merged(base, override):
    if not isinstance(override, dict):
        raise ValueError('Конфигурация должна быть JSON-объектом')
    result = copy.deepcopy(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(result.get(key), dict):
            result[key] = merged(result[key], value)
        else:
            result[key] = value
    return result


def location(value):
    return Path(value).expanduser().resolve()


def default_paths():
    if sys.platform == 'darwin':
        base = Path.home() / 'Library/Application Support/control-workflow'
        return base / 'state', base / 'config.json'
    if sys.platform != 'linux':
        raise ValueError('Поддерживаются Linux и macOS')
    return (Path(os.environ.get('XDG_STATE_HOME', Path.home() / '.local/state')) / 'control-workflow',
            Path(os.environ.get('XDG_CONFIG_HOME', Path.home() / '.config')) / 'control-workflow/config.json')


def load(state=None, config=None, bindings=None):
    default_state, default_config = default_paths()
    config_path = config or os.environ.get('CONTROL_PLANE_CONFIG')
    if not config_path and default_config.is_file():
        config_path = default_config
    settings = json.loads((PROGRAM_ROOT / 'config/defaults.json').read_text())
    if config_path:
        config_path = location(config_path)
        settings = merged(settings, json.loads(config_path.read_text()))
    for key in ('state_root', 'bindings_root'):
        if settings.get(key) and not Path(settings[key]).expanduser().is_absolute():
            raise ValueError(key + ' в config должен быть абсолютным путём')
    root = location(state or os.environ.get('CONTROL_PLANE_STATE') or
                    os.environ.get('CONTROL_PLANE_ROOT') or settings.get('state_root') or default_state)
    binding_root = location(bindings or os.environ.get('CONTROL_PLANE_BINDINGS') or
                            settings.get('bindings_root') or root / 'bindings')
    runtime_file = binding_root / 'runtime.json'
    machine = json.loads(runtime_file.read_text()) if runtime_file.exists() else {}
    if not isinstance(machine, dict):
        raise ValueError('runtime.json должен быть JSON-объектом')
    for key in ('cwd', 'worktrees_root'):
        if machine.get(key) and not Path(machine[key]).expanduser().is_absolute():
            raise ValueError(key + ' в runtime должен быть абсолютным путём')
    agent_adapters.select(settings['adapter'])
    if settings.get('schema_version') != 1:
        raise ValueError('Неподдержанная schema_version config; требуется 1')
    if machine.get('schema_version', 1) != 1:
        raise ValueError('Неподдержанная schema_version bindings; требуется 1')
    if settings['git_mode'] not in ('main', 'branch'):
        raise ValueError('Неизвестный git_mode')
    timeout = settings['agent_start_timeout_ms']
    if isinstance(timeout, bool) or not isinstance(timeout, int) or timeout <= 0:
        raise ValueError('agent_start_timeout_ms должен быть положительным целым числом')
    profiles = settings['profiles']
    if not isinstance(profiles, dict):
        raise ValueError('profiles должен быть JSON-объектом')
    for kind in ('implementation', 'research', 'reasoning', 'planning'):
        if not isinstance(profiles.get(kind), dict):
            raise ValueError('Не найдены профили назначения ' + kind)
        for complexity in ('normal', 'complex'):
            if not isinstance(profiles[kind].get(complexity), dict):
                raise ValueError('Не найден профиль ' + kind + '/' + complexity)
            profile = settings['profiles'][kind][complexity]
            if not isinstance(profile.get('model'), str) or not profile['model'].strip():
                raise ValueError('Профиль должен содержать model')
            if profile.get('reasoning_effort') not in (('low', 'medium', 'high', 'xhigh', 'max') if settings['adapter'] == 'herdr-claude' else ('low', 'medium', 'high', 'xhigh', 'max', 'ultra')):
                raise ValueError('Неизвестный reasoning_effort')
    return root, binding_root, settings, machine, config_path


def child_environment(root, bindings, config):
    env = dict(os.environ, CONTROL_PLANE_STATE=str(root), CONTROL_PLANE_BINDINGS=str(bindings),
               PYTHONDONTWRITEBYTECODE='1')
    env.pop('CONTROL_PLANE_ROOT', None)
    if config:
        env['CONTROL_PLANE_CONFIG'] = str(config)
    else:
        env.pop('CONTROL_PLANE_CONFIG', None)
    return env
