---
name: control-review
description: Получить прогресс, вопросы и результат задачи или вернуть её на доработку; приёмку направить в control-complete.
---

Прочитай [команду и выбор экземпляра](../../docs/control-entry.md).
Выполни show tasks TASK и run RUN. Читай только нужные progress.md,
questions.json и result.md. Покажи в чате выводы, изменения, проверки,
ограничения и вопросы; ссылка на отчёт дополняет ответ.

Для blocked используй [worker.md](../../docs/worker.md); для launch_failed —
[recovery.md](../../docs/recovery.md). Просмотр не означает приёмку.
По поручению вернуть на доработку: task-set TASK --status ready --goal '...'.
Запускай продолжение только если пользователь его попросил.
Просьбу принять или завершить выполни через
[control-complete](../control-complete/SKILL.md). До приёмки сохраняй файлы и worktree.
