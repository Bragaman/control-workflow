---
name: control-review
description: Показать прогресс, вопросы или результат задачи панели; вернуть на доработку; отправить результат на независимое агентское ревью через subagent.
---

Если контракт экземпляра ещё не прочитан, читай
[control-entry.md](../../docs/control-entry.md).
Выполни show tasks TASK и run RUN. Читай только нужные progress.md,
questions.json и result.md. Покажи в чате выводы, изменения, проверки,
ограничения и вопросы; ссылка на отчёт дополняет ответ.

Для blocked используй [questions.md](../../docs/questions.md); для launch_failed —
[recovery.md](../../docs/recovery.md). Просмотр не означает приёмку.
По поручению вернуть на доработку: task-set TASK --status ready --goal '...'.
Запускай продолжение только если пользователь его попросил.
Если пользователь поручил принять, завершить или отменить задачу, прочитай
[control-complete](../control-complete/SKILL.md) и выполни его инструкции.
До приёмки сохраняй файлы и worktree.

Для поручения об агентском ревью и pending auto-задачи прочитай
[agent-review.md](../../docs/agent-review.md) и выполни его протокол.
