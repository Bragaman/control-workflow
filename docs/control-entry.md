# Команда и экземпляр панели

Из любого каталога запускай `python3 /absolute/path/control_plane/scripts/control.py`.
Путь программы бери из assignment.md или выбранной пользователем установки.
Перед подкомандой указывай `--state-root /absolute/path/state`, если данные
находятся отдельно. Выбор установки не означает выбор её встроенного state.
При неизвестном экземпляре уточни его, прежде чем изменять данные.

`--config /path/config.json` выбирает настройки пользователя;
`--bindings-root /path/bindings` — привязки этой машины. CLI имеет приоритет над
CONTROL_PLANE_STATE, CONTROL_PLANE_CONFIG и CONTROL_PLANE_BINDINGS.
CONTROL_PLANE_ROOT сохраняется как прежнее имя state; документы от него не зависят.
Полные выбранные пути показывает команда `environment`.

Вне herdr перед подкомандой укажи `--herdr-session NAME`; имя проверяй через
`herdr session list`. При запрете сокета повтори ту же команду с разрешением
вне песочницы. Не подставляй HERDR_ENV и не выбирай чужой текущий workspace.

Управляющий запускает только выбранные задачи и возвращает результат open,
не ожидая работы исполнителя. Исполнитель читает [worker.md](worker.md),
показывает результат в чате и сохраняет артефакты. План — по
[planning.md](planning.md), ошибки запуска — по [recovery.md](recovery.md),
приёмка и Git — по [git-policy.md](git-policy.md).
