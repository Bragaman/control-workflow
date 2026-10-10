"""Current interactive adapter; lifecycle and ownership checks stay in the core."""
import json
from pathlib import Path

ADAPTER = 'herdr-codex'
AGENT_KIND = 'codex'
ACCESS_POLICY = 'workspace-write'
CAPABILITIES = {'interactive_start': True, 'assignment_confirmation': True,
                'resume_by_id': False, 'session_id_capture': False,
                'turn_events': False, 'platforms_tested': ['linux']}


def transport(command, args, selected, timeout, executable='herdr'):
    prefix = [executable, '--session', selected] if selected else [executable]
    return command([*prefix, *args], timeout=timeout)


def namespace(command, selected, inherited, machine_id, uid, executable='herdr'):
    prefix = [executable, '--session', selected] if selected else [executable]
    sessions = json.loads(command([*prefix, 'session', 'list', '--json']))['sessions']
    matches = [item for item in sessions if (item['name'] == selected if selected else
               inherited and Path(item['socket_path']).resolve() == Path(inherited).resolve())]
    if len(matches) != 1:
        raise ValueError('Не удалось однозначно определить namespace herdr; укажите --herdr-session NAME')
    item = matches[0]
    return dict(session=item['name'], socket=str(Path(item['socket_path']).resolve()),
                machine=machine_id, uid=uid)


def start_args(run, task, cwd, assignment, state_root, worker_doc, timeout_ms):
    args = ['agent', 'start', 'cp-' + run['id'][4:], '--kind', AGENT_KIND,
            '--pane', run['pane'], '--timeout', str(timeout_ms), '--',
            '--sandbox', ACCESS_POLICY, '--cd', str(cwd),
            '--config', 'model_reasoning_effort=' + run['reasoning_effort']]
    if run['model'] != 'default':
        args.extend(['--model', run['model']])
    if Path(cwd).resolve() != state_root:
        args.extend(['--add-dir', str(state_root)])
    for wt in run['worktrees'][1:]:
        args.extend(['--add-dir', wt['path']])
    args.append(task['id'] + ': Прочитай задание ' + str(assignment) +
                ' и выполни его. Покажи результат в чате и сохрани полезные артефакты по ' + str(worker_doc) + '.')
    return args
