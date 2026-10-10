# Пути, настройки и данные

По умолчанию state расположен вне программы: Linux — XDG_STATE_HOME/control-workflow
(иначе ~/.local/state/control-workflow); macOS — ~/Library/Application Support/control-workflow/state. Для отдельного экземпляра укажи --state-root; установка
может быть доступна только для чтения. Каталог state создаётся при первой записи.
Справка и environment доступны без herdr. Исполнители запускаются через herdr. Степень проверки адаптеров указана в compatibility.md.

| Слой | Файлы | Выбор |
| --- | --- | --- |
| Программа | scripts/, config/defaults.json, docs/, skills/ | Физический путь control.py; ресурсы доступны для чтения |
| Пользователь | JSON с profiles, adapter, git_mode, agent_start_timeout_ms, state_root, bindings_root | --config, затем CONTROL_PLANE_CONFIG; без выбора — defaults.json |
| Машина | BINDINGS_ROOT/runtime.json, projects/ID.json и tasks/ID.json | --bindings-root, CONTROL_PLANE_BINDINGS, bindings_root из config, иначе STATE_ROOT/bindings |
| Состояние | projects/, tasks/, runs/, memory/, notifications/, views/, status/, .control.lock | --state-root, CONTROL_PLANE_STATE, прежний CONTROL_PLANE_ROOT, state_root из config, иначе внешний каталог ОС |

Пути в JSON должны быть абсолютными. Относительные пути параметров CLI и
переменных окружения разрешаются от текущего каталога. Для воспроизводимости
используй абсолютные пути. CHILD получает выбранные state/config/bindings
через окружение; запуск из собственного таба сохраняет их при complete.

Пользовательский JSON объединяется с defaults.json; можно заменить отдельный
профиль без копирования всей таблицы. Например:

```json
{
  "git_mode": "branch",
  "profiles": {
    "implementation": {
      "normal": {"model": "my-model", "reasoning_effort": "medium"}
    }
  }
}
```

Существование модели у провайдера не проверяется: конфигурацию выбирает
пользователь. Проверяются наличие имени, допустимый effort, Git-режим,
положительный таймаут и adapter=herdr-codex или herdr-claude.
Неизвестный адаптер отклоняется до создания терминала.

runtime.json принимает herdr_executable, herdr_session, cwd и worktrees_root.
Для проекта рабочий каталог меняется через project-set --cwd /path.
Если worktree нет, cwd проекта имеет приоритет над cwd машины; иначе используется
state. Репозитории, workspace, namespace и layout_pending проектов, а также
переопределения репозиториев задач записываются в bindings. CLI show объединяет
описание и локальную привязку. Исторические run сохраняют пути как свидетельство
своего запуска; это не переносимые настройки нового экземпляра.

Старые смешанные project/task JSON продолжают читаться. При следующей записи
локальные поля сохраняются сначала в bindings, затем убираются из описания;
сбой между записями оставляет обе копии, и bindings имеют приоритет.
Для отката на старый код сначала выполни materialize-bindings: команда
дублирует локальные поля в прежние JSON под блокировкой, сохраняя bindings. Текущие
запуски, память и worktree не мигрируют и не удаляются. Переход нескольких файлов
не образует общей транзакции: после аварии сверяй run, task и binding.
Каждому state нужен собственный bindings_root; совместное использование одного
каталога bindings двумя state нарушает изоляцию.

Сводки сверок находятся в STATE_ROOT/status.

Отчёты finish и memory-add --file пересчитывают обычные Markdown-ссылки и
определения ссылок относительно нового файла; абсолютные пути, URL, anchors
и блоки кода сохраняются. Соседние артефакты остаются в исходном каталоге:
сохранение текста не копирует всю папку. Для вложенных скобок в пути используй
URL-кодирование, для пробелов — угловые скобки или %20. HTML и нестандартные
Markdown-конструкции не преобразуются.

Настройки config и runtime bindings имеют schema_version=1; неизвестная версия
отклоняется до внешних действий. Default config Linux — XDG_CONFIG_HOME/control-workflow/config.json
(иначе ~/.config/control-workflow/config.json); macOS — ~/Library/Application Support/control-workflow/config.json.
Если этот файл есть, он читается автоматически; явный --config имеет приоритет.
Отсутствующие bindings schema_version читаются как версия 1 для совместимости.
