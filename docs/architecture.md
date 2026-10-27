# Устройство

CLI (src/args.rs) и MCP stdio (src/mcp.rs) вызывают общее ядро src/core.rs.
Команды и схемы параметров описаны в src/commands.json. Файловое хранилище
src/store.rs использует flock одного state/.control.lock и атомарную замену файлов.

src/config.rs выбирает program/config/bindings/state. src/lifecycle.rs сохраняет
intent перед запуском, проверяет namespace и ownership. Неизвестный исход старта
требует сверки; автоматическая повторная отправка assignment запрещена.
Результат herdr done означает готовность агента к вводу; приёмка панели выполняется отдельно.

src/external.rs вызывает Git, herdr и CLI агентов без shell, проверяет identity
процессов на Linux/macOS. Linux использует boot_id/start_ticks; macOS —
IOPlatformUUID, kern.boottime и libproc с микросекундным временем старта
и проверкой zombie. ABI: [proc_bsdinfo XNU](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/sys/proc_info.h).
Подставные ответы в тестах не заменяют проверку macOS. Адаптер сохраняется в запуске и используется при
восстановлении, даже если настройки следующего запуска изменились. Модель default
оставляет выбор модели CLI агента.

Codex сохраняет workspace-write. Claude запускается с permission-mode=default;
его собственные запросы доступа остаются активными. Это не эквивалент sandbox Codex.
См. документацию Claude о [флагах](https://code.claude.com/docs/en/cli-reference)
и [правах доступа](https://code.claude.com/docs/en/permissions). Дополнительные каталоги
передаются через --add-dir. Запрошенный UUID сохраняется отдельно и не доказывает
identity активной сессии. Native resume и события отдельных ходов не реализованы.

src/git.rs сохраняет checkpoint после Git-действий. src/review.rs создаёт
фиксированные снимки и проверяет версию и blobs. src/core.rs пересчитывает
Markdown-ссылки отчётов. Сохранённый формат, включая канонические хеши и
NaN/Infinity в файлах, совместим с существующими записями. MCP передаёт эти числа
через строковые теги и nonFiniteNumbers, сохраняя строгий JSON-RPC.

src/resources.rs встраивает документы, скиллы и примеры; src/shipping.rs создаёт
самостоятельную поставку с бинарником и SHA-256 manifest. Ресурсы совпадают
с исходными файлами. Подробнее: [расположение данных](runtime-layout.md),
[контракт исполнителя](worker.md), [Git](git-policy.md), [восстановление](recovery.md).
