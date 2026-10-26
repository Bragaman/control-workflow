---
name: control-task
description: Создать или изменить задачу контрольной панели, критерии, зависимости и приёмщика.
---

Если контракт экземпляра ещё не прочитан, читай
[control-entry.md](../../docs/control-entry.md).
Для создания нужны проект, цель и проверяемые критерии. Выведи их из запроса;
уточни существенные пробелы одним сообщением. До передачи работы сохрани план
по [planning.md](../../docs/planning.md).

`task-create ID --project ID --title '...' --goal '...' --acceptance '...' --compact`.
Тип и сложность выбери по [task-profile.md](../../docs/task-profile.md).

--code нужен для изменений отслеживаемых файлов репозитория; --no-code —
для чтения и применения существующих настроек на хост. Повторяемый --repo
переопределяет репозитории проекта; --depends задаёт зависимости.
Для --code Git-режим выбери по [git-policy.md](../../docs/git-policy.md).
Приёмщик по умолчанию user; делегирование требует явного поручения.

Редактирование: task-set ID --goal '...' --acceptance '...' --status ready --compact;
тип/сложность — --task-kind и --complexity. Для running/blocked используй
progress; для вопросов — [questions.md](../../docs/questions.md),
для interrupt — [recovery.md](../../docs/recovery.md).

--compact сокращает ответ; полная запись доступна через `show tasks ID`.
Без флага task-create/task-set сохраняют прежний полный JSON.
