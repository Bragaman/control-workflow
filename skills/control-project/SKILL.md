---
name: control-project
description: Создать или настроить проект контрольной панели.
---

Если контракт экземпляра ещё не прочитан, читай
[control-entry.md](../../docs/control-entry.md).
Сначала projects: проверь дубликаты. Из запроса выведи название, короткий ID
на латинице и описание; создай project-create ID --name '...' --description '...'.
Уточни только неясную цель; репозитории и workspace нужны при выборе работы.
Для изменения — project-set ID --description '...', повторяемый --repo /path,
--workspace ID или --clear-workspace. Команда сохраняет локальные пути и
workspace отдельно от описания проекта; чужой namespace требует перепривязки.
