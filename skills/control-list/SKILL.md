---
name: control-list
description: Показать задачи, проекты и ожидающие ответа вопросы контрольной панели.
---

Если контракт экземпляра ещё не прочитан, читай
[control-entry.md](../../docs/control-entry.md).
Выполни `tasks`; при необходимости `tasks --project ID` или
`tasks --status blocked` (для ревью — `--status review`). Для завершённых и
отменённых используй `tasks --all`; для проектов — `projects`.
Покажи ID, название и статус. Для списка достаточно краткого вывода команды.
Если спрашивают, что ждёт ответа, прочитай questions.json только blocked;
для launch_failed — error из state.json. Просмотр списка не запускает задачи.
