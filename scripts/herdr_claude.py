"""Experimental interactive Claude Code adapter; retains permission prompts."""
from pathlib import Path
import uuid

ADAPTER = 'herdr-claude'
AGENT_KIND = 'claude'
ACCESS_POLICY = 'default'
CAPABILITIES = {'interactive_start': True, 'assignment_confirmation': True,
                'resume_by_id': False, 'session_id_capture': False,
                'turn_events': False, 'platforms_tested': [], 'experimental': True}


def start_args(run, task, cwd, assignment, state_root, worker_doc, timeout_ms):
    # A requested UUID is recorded, but is not confirmation of a live CLI session.
    session_id = run.setdefault('requested_agent_session_id', str(uuid.uuid4()))
    args = ['agent', 'start', 'cp-' + run['id'][4:], '--kind', AGENT_KIND,
            '--pane', run['pane'], '--timeout', str(timeout_ms), '--',
            '--session-id', session_id, '--permission-mode', ACCESS_POLICY,
            '--effort', run['reasoning_effort']]
    if run['model'] != 'default':
        args.extend(['--model', run['model']])
    dirs = [str(state_root), *[wt['path'] for wt in run['worktrees'][1:]]]
    for directory in dict.fromkeys(dirs):
        if Path(directory).resolve() != Path(cwd).resolve():
            args.extend(['--add-dir', directory])
    args.append(task['id'] + ': Прочитай задание ' + str(assignment) +
                ' и выполни его. Покажи результат в чате и сохрани полезные артефакты по ' + str(worker_doc) + '.')
    return args
