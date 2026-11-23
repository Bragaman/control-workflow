# Команда и экземпляр панели

Бери абсолютный путь bin/control-workflow и параметры state/config/bindings из assignment.md
или выбранной пользователем установки. Из любого cwd вызывай
`/absolute/path/program/bin/control-workflow`.
Установка и state — разные выборы; неизвестный экземпляр уточни
до записи. Выбранные пути показывает `environment`.

Перед подкомандой: --state-root /path/state, --config /path/config.json,
--bindings-root /path/bindings. Они приоритетнее CONTROL_PLANE_STATE,
CONTROL_PLANE_CONFIG, CONTROL_PLANE_BINDINGS; CONTROL_PLANE_ROOT — прежнее имя state.

Для hc-codex выбирай hc_session в runtime.json. Подключение из Desktop, TUI и API описано в [runtime-layout.md](runtime-layout.md#один-терминал-hc-в-трёх-клиентах).

Для команд TUI herdr вне его окружения укажи --herdr-session NAME, проверив имя
через `herdr session list`. При запрете сокета повтори ту же команду с разрешением
вне песочницы. Не подставляй HERDR_ENV и не выбирай чужой текущий workspace.

Исполнитель читает [worker.md](worker.md). При составлении плана читай
[planning.md](planning.md), при ошибке запуска — [recovery.md](recovery.md),
при выборе Git-режима или приёмке — [git-policy.md](git-policy.md).
