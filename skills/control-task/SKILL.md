---
name: control-task
description: Создать или изменить задачу контрольной панели, критерии, зависимости и приёмщика.
---

Если контракт экземпляра ещё не прочитан, читай
[control-entry.md](../../docs/control-entry.md).
Для создания нужны проект, цель и проверяемые критерии. Выведи их из запроса;
уточни пробелы, влияющие на цель или критерии одним сообщением. До передачи работы сохрани план
по [planning.md](../../docs/planning.md).

Тип, сложность и именованный профиль выбери по [task-profile.md](../../docs/task-profile.md).
Используй абсолютную команду выбранного экземпляра с его state/config/bindings;
ниже `control-workflow` обозначает эту команду со всеми глобальными параметрами:

```sh
control-workflow task-create TASK --project PROJECT --title 'Название' --goal 'Цель' --acceptance 'Критерии' --code --task-kind implementation --complexity normal --profile PROFILE --agent-review-policy auto --compact
```

Для работы без изменений отслеживаемых файлов замени `--code` на `--no-code`;
тип, сложность и профиль в примере замени выбранными значениями. Создание не
запускает исполнителя: для выбранной пользователем задачи используй control-start.

--code нужен для изменений отслеживаемых файлов репозитория; --no-code —
для чтения и применения существующих настроек на хост. Повторяемый --repo
переопределяет репозитории проекта; --depends задаёт зависимости.
Для --code Git-режим выбери по [git-policy.md](../../docs/git-policy.md).
Приёмщик по умолчанию user; делегирование требует явного поручения.
Для новой задачи агентское ревью по умолчанию auto; manual и off задаются
явно через --agent-review-policy как исключения. Auto не меняет приёмщика.

Редактирование: task-set ID --goal '...' --acceptance '...' --status ready --compact;
тип/сложность — --task-kind и --complexity; профиль — --profile NAME.
--profile auto возвращает правила политики. Явный профиль сохраняется в задаче;
причину выбора кратко запиши в плане. Для running/blocked используй
progress; для вопросов — [questions.md](../../docs/questions.md),
для interrupt — [recovery.md](../../docs/recovery.md).

--compact сокращает ответ; полная запись доступна через `show tasks ID`.
Без флага task-create/task-set сохраняют прежний полный JSON.
