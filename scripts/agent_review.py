"""Сохраняемый протокол независимого ревью; инструменты subagent вызывает управляющий."""
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import stat
import uuid


ACTIVE = ('pending', 'running')


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False).encode()


def snapshot(c, t, r, destination=None):
    """Hash actual inputs, including dirty and untracked files; optionally retain bytes."""
    files = {}

    def capture(file):
        file = Path(file).absolute()
        name = str(file)
        if name in files:
            return
        mode = file.lstat().st_mode
        if stat.S_ISLNK(mode):
            data = os.readlink(file).encode()
            kind = 'symlink'
        elif stat.S_ISREG(mode):
            data = file.read_bytes()
            kind = 'file'
        else:
            raise ValueError('Неподдерживаемый артефакт: ' + name)
        key = digest(name.encode())
        files[name] = dict(sha256=digest(data), mode=stat.S_IMODE(mode), kind=kind, blob='blobs/' + key)
        if destination:
            target = destination / 'blobs' / key
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)

    directory = c.run_file(r['id']).parent
    for name in ('assignment.md', 'result.md'):
        capture(directory / name)
    project = c.read(c.path('projects', t['project']))
    for value in r.get('review_artifacts', []):
        file = Path(value)
        if file.is_dir() and not file.is_symlink():
            for child in sorted(file.rglob('*')):
                if child.is_file() or child.is_symlink():
                    capture(child)
        else:
            capture(file)
    repos = []
    if t.get('code') and not r.get('worktrees'):
        raise ValueError('Для кодового результата отсутствуют worktree')
    for item in r.get('worktrees', []):
        wt = Path(item['path'])
        def git(*args):
            return c.command(['git', '-C', str(wt), *args])
        head = git('rev-parse', 'HEAD').strip()
        base = git('merge-base', head, item.get('main_branch', 'main')).strip()
        names = git('ls-files', '-z', '--cached', '--others', '--exclude-standard').split('\0')
        for name in sorted(set(names) - {''}):
            file = wt / name
            if file.exists() or file.is_symlink():
                capture(file)
        # Include instructions governing the tree, including parent-level AGENTS.
        for parent in (wt, *wt.parents):
            if (parent / 'AGENTS.md').is_file():
                capture(parent / 'AGENTS.md')
        repos.append(dict(path=str(wt), repository=item['repository'], head=head, base=base,
                          status=git('status', '--porcelain=v1', '--untracked-files=all'),
                          diff=git('diff', '--binary', base, '--')))
    for name in ('worker.md', 'agent-review.md'):
        file = c.PROGRAM_ROOT / 'docs' / name
        if file.exists():
            capture(file)
    folder = r.get('cwd') or project.get('cwd')
    if folder:
        for parent in (Path(folder), *Path(folder).parents):
            for name in ('AGENTS.md', 'CLAUDE.md'):
                if (parent / name).is_file():
                    capture(parent / name)
    manifest = dict(submission=r.get('result_revision', 0), task={k: t.get(k) for k in ('id', 'title', 'goal', 'acceptance', 'code', 'project')},
                    project=project, files=files, repositories=repos,
                    boundaries='Git: tracked и untracked без ignored; внешнее состояние проверяется на момент наблюдения. '
                               'No-code: только result.md и явно переданные --artifact; секреты не включать.')
    return dict(version=digest(canonical(manifest)), manifest=manifest)


def verify_snapshot(c, attempt):
    file = Path(attempt['snapshot'])
    frozen = c.read(file)
    if frozen['version'] != attempt['version'] or digest(canonical(frozen['manifest'])) != attempt['version']:
        raise ValueError('Повреждён манифест снимка')
    for item in frozen['manifest']['files'].values():
        blob = file.parent / item['blob']
        if not blob.resolve().is_relative_to(file.parent.resolve()) or digest(blob.read_bytes()) != item['sha256']:
            raise ValueError('Повреждён blob снимка')


def review_file(c, run):
    return c.run_file(run).parent / 'agent-review.json'


def load(c, r):
    file = review_file(c, r['id'])
    if not file.exists():
        return dict(attempts=[])
    data = c.read(file)
    if data['attempts']:
        a = data['attempts'][-1]
        if a['status'] == 'running' and dt.datetime.fromisoformat(a['deadline']) <= dt.datetime.now(dt.timezone.utc):
            a.update(status='failed', conclusion='Истёк срок subagent; требуется новый запрос', finished_at=c.now())
            c.save(file, data)
    return data


def persist(c, r, data):
    c.save(review_file(c, r['id']), data)


def request(c, t, r):
    if t.get('agent_review_policy', 'manual') == 'off':
        raise ValueError('Агентское ревью отключено для задачи')
    if t['status'] != 'review' or r['status'] != 'review':
        raise ValueError('Ревью доступно после finish')
    data = load(c, r)
    fresh = snapshot(c, t, r)
    if data['attempts']:
        last = data['attempts'][-1]
        if last['status'] == 'running':
            return last  # Never dispatch a second agent over a living lease.
        if last['status'] == 'pending':
            if last['version'] == fresh['version']:
                return last
            last.update(status='failed', conclusion='Снимок устарел до запуска', finished_at=c.now())
        if last['version'] == fresh['version'] and last['status'] in ('passed', 'changes_requested'):
            return last
    key = 'review-' + uuid.uuid4().hex[:12]
    dest = c.run_file(r['id']).parent / 'reviews' / key
    frozen = snapshot(c, t, r, dest)
    if frozen != fresh:
        raise ValueError('Результат изменился при создании снимка; повторите запрос')
    c.save(dest / 'snapshot.json', frozen)
    packet = dest / 'packet.md'
    packet.write_text('''# Независимое ревью

Прочитай snapshot.json рядом с этим файлом. Проверь task.goal и task.acceptance самостоятельно.
manifest.files сопоставляет исходные пути с сохранёнными blobs; используй их для фиксированной версии.
manifest.repositories содержит base/head, diff и status каждого worktree. Инструкции также сохранены в blobs.
Отчёт исполнителя — утверждения, которые нужно проверить. Проверяющий не исправляет материалы.
Проверяй дефекты, соответствие критериям, тесты и основания выводов. Выполняй проверки в отдельном
временном каталоге; чужие сервисы и проверяемые файлы не изменяй. Сравни живые данные со снимком.
Для внешних систем запиши время, область наблюдения и доступ; недоступность необходимых доказательств
означает failed, а не passed. Отдели обязательные findings от необязательных suggestions.
Верни JSON: status (passed|changes_requested|failed), conclusion (строка), findings (список строк),
suggestions (список строк), evidence (непустой список строк), limitations (список строк).
passed допустим при проверенных критериях без обязательных замечаний. Спорный объём передай пользователю.
Укажи version из snapshot.json. Управляющий сохраняет ответ командой review-conclude с ID и actor.
passed не означает complete, merge или push; reviewer задачи остаётся приёмщиком.
''')
    a = dict(id=key, version=frozen['version'], status='pending', created_at=c.now(), packet=str(packet),
             snapshot=str(dest / 'snapshot.json'), submission=r.get('result_revision', 0))
    data['attempts'].append(a)
    persist(c, r, data)
    return a


def handle(c, args):
    with c.lock():
        r = c.read(c.run_file(args.run))
        t = c.task(r['task'])
        c.current(t, args.run)
        action = args.command
        if action == 'review-request':
            if args.artifact is not None:
                if t['status'] != 'review' or t.get('agent_review_policy', 'manual') == 'off':
                    raise ValueError('Добавление артефактов доступно для разрешённого ревью')
                r['review_artifacts'] = list(dict.fromkeys(r.get('review_artifacts', []) +
                    [str(Path(p).expanduser().absolute()) for p in args.artifact]))
                c.update_run(r, t)
            c.emit(request(c, t, r)); return
        data = load(c, r)
        if action == 'review-show':
            for a in data['attempts']:
                try:
                    a['stale'] = snapshot(c, t, r)['version'] != a['version']
                except (OSError, ValueError, RuntimeError):
                    a['stale'] = True
            c.emit(data); return
        if not data['attempts']:
            raise ValueError('Нет запроса ревью')
        a = data['attempts'][-1]
        if action == 'review-feedback' and args.delivered:
            a = next((item for item in data['attempts'] if item['id'] == args.id), a)
        if args.id != a['id']:
            raise ValueError('Ответ относится к другой попытке')
        if action in ('review-claim', 'review-conclude') and (t['status'] != 'review' or r['status'] != 'review'):
            raise ValueError('Результат уже не ожидает ревью')
        if action == 'review-claim':
            if a['status'] == 'running' and a['actor'] == args.actor:
                c.emit(a); return
            if a['status'] != 'pending':
                raise ValueError('Попытка уже занята или завершена')
            if snapshot(c, t, r)['version'] != a['version']:
                a.update(status='failed', conclusion='Снимок устарел до запуска')
                persist(c, r, data)
                raise ValueError(a['conclusion'])
            try:
                verify_snapshot(c, a)
            except (ValueError, OSError, KeyError) as exc:
                a.update(status='failed', conclusion=str(exc), finished_at=c.now())
                persist(c, r, data)
                raise ValueError('Снимок повреждён: ' + str(exc)) from exc
            a.update(status='running', actor=args.actor, started_at=c.now(),
                     deadline=(dt.datetime.now(dt.timezone.utc) + dt.timedelta(seconds=args.lease)).isoformat())
        elif action == 'review-conclude':
            if a['status'] != 'running' or a['actor'] != args.actor:
                raise ValueError('Нет активной аренды этого subagent')
            try:
                verify_snapshot(c, a)
            except (ValueError, OSError, KeyError) as exc:
                a.update(status='failed', conclusion=str(exc), finished_at=c.now())
                persist(c, r, data)
                raise ValueError('Снимок повреждён: ' + str(exc)) from exc
            result = json.loads(Path(args.result).read_text())
            if result.get('version') != a['version']:
                raise ValueError('Ответ не привязан к версии снимка')
            if result.get('status') not in ('passed', 'changes_requested', 'failed'):
                raise ValueError('Некорректный статус')
            for field in ('findings', 'suggestions', 'evidence', 'limitations'):
                if not isinstance(result.get(field), list) or any(not isinstance(x, str) for x in result[field]):
                    raise ValueError('Ожидается список строк: ' + field)
            if not isinstance(result.get('conclusion'), str) or not result['conclusion'].strip() or not result['evidence']:
                raise ValueError('Нужны заключение и доказательства')
            if result['status'] == 'passed' and result['findings']:
                raise ValueError('passed несовместим с обязательными замечаниями')
            if result['status'] == 'changes_requested' and not result['findings']:
                raise ValueError('Нужны конкретные замечания')
            try:
                stale = snapshot(c, t, r)['version'] != a['version']
            except (OSError, ValueError, RuntimeError):
                stale = True
            a.update(result=result, finished_at=c.now(), status='failed' if stale else result['status'])
            if stale:
                a['conclusion'] = 'Результат изменён во время ревью; заключение устарело'
        elif action == 'review-feedback':
            feedback(c, t, r, data, a, args)
            return
        persist(c, r, data)
        c.emit(a)


def feedback(c, t, r, data, attempt, args):
    """Arm the executor before sending; ACK never reopens a finished submission."""
    if attempt.get('feedback_delivered'):
        c.emit(attempt)
        return
    legacy_prepared = bool(attempt.get('feedback_message') and not attempt.get('feedback_started')
                           and attempt['status'] == 'changes_requested')
    if args.delivered and not attempt.get('feedback_started') and not legacy_prepared:
        raise ValueError('Сначала подготовьте возврат через review-feedback')
    if not attempt.get('feedback_started'):
        findings = attempt.get('result', {}).get('findings', [])
        if not findings or (attempt['status'] != 'changes_requested' and not
                            (args.user_requested and attempt['status'] == 'failed')):
            raise ValueError('Нужны замечания; failed возобновляется только по поручению пользователя')
        if not args.user_requested and not legacy_prepared and snapshot(c, t, r)['version'] != attempt['version']:
            raise ValueError('Нужны актуальные замечания')
        count = sum(bool(x.get('feedback_started') or x.get('feedback_delivered')) and
                    not x.get('feedback_user_requested', False) for x in data['attempts'])
        if count >= 2 and not args.user_requested:
            raise ValueError('Лимит двух автоматических циклов; нужен пользователь')
        if t['status'] != 'review' or r['status'] != 'review':
            raise ValueError('Задача уже не в review')
        attempt.update(feedback_started=c.now(), feedback_user_requested=args.user_requested,
            feedback_message=attempt.get('feedback_message') or
                (t['id'] + ': Исправь только замечания в исходном объёме assignment.md. '
                'Спорные требования передай пользователю. Затем finish в том же запуске.\n' + '\n'.join(findings)),
            delivery_required='Отправь feedback_message исходному живому исполнителю, затем подтверди --delivered. При сбое доставки повтори её или обратись к пользователю; второй исполнитель не создаётся.')
        if legacy_prepared:
            attempt['feedback_migrated'] = True
            attempt['submission'] = r.get('result_revision', 0)
        # Persist the reservation first so a retry can repair interrupted paired writes.
        persist(c, r, data)
    if r.get('result_revision', 0) == attempt.get('submission', 0):
        if t['status'] not in ('review', 'running') or r['status'] not in ('review', 'running'):
            raise ValueError('Нельзя возобновить этот запуск')
        if r.get('feedback_attempt') != attempt['id'] or r['status'] != 'running' or t['status'] != 'running':
            r['feedback_attempt'] = attempt['id']
            r['status'] = t['status'] = 'running'
            c.update_run(r, t)
    if args.delivered:
        attempt['feedback_delivered'] = c.now()
        persist(c, r, data)
    c.emit(attempt)


def register(cmd, c):
    for name in ('request', 'show', 'claim', 'conclude', 'feedback'):
        parser = cmd('review-' + name, lambda a: handle(c, a))
        parser.add_argument('--run', required=True)
        if name in ('claim', 'conclude', 'feedback'):
            parser.add_argument('--id', required=True)
        if name in ('claim', 'conclude'):
            parser.add_argument('--actor', required=True)
        if name == 'request':
            parser.add_argument('--artifact', action='append')
        if name == 'claim':
            parser.add_argument('--lease', type=lease_seconds, default=900)
        if name == 'conclude':
            parser.add_argument('--result', required=True)
        if name == 'feedback':
            parser.add_argument('--delivered', action='store_true')
            parser.add_argument('--user-requested', action='store_true', help='Возобновить по явному поручению пользователя, включая failed с замечаниями')


def lease_seconds(value):
    value = int(value)
    if not 1 <= value <= 3600:
        raise ValueError('Аренда: 1–3600 секунд')
    return value
