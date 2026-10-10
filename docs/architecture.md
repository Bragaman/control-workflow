# Устройство

scripts/control.py содержит файловый workflow и защиту запуска: блокировку,
проверку namespace, ownership, сохранение intent перед внешним действием.
Неизвестный исход старта требует сверки, автоматическая повторная отправка
assignment запрещена. Результат herdr done означает готовность агента к вводу;
приёмка панели выполняется отдельно.

runtime_config.py выбирает program/config/bindings/state. agent_adapters.py
выбирает адаптер; в запуске он сохраняется и используется при восстановлении,
даже если настройки следующего запуска изменились. herdr_codex.py также
содержит общий транспорт к herdr; herdr_claude.py строит второй интерактивный
запуск. Модель default не передаёт --model, пользуясь настройкой CLI.

Codex сохраняет workspace-write. Claude запускается с permission-mode=default:
его собственные запросы доступа остаются активными. Это не эквивалент sandbox
Codex. Дополнительные каталоги передаются через --add-dir. Подробности
[флагов](https://code.claude.com/docs/en/cli-reference) и
[прав доступа](https://code.claude.com/docs/en/permissions) — в документации Claude.
Запрошенный UUID сохраняется отдельно; он не доказывает identity активной сессии.
Native resume и события отдельных ходов пока не реализованы.

platform_support.py сохраняет Linux boot_id/start_ticks; macOS использует
IOPlatformUUID, kern.boottime и libproc с микросекундным временем старта и
проверкой zombie. ABI опирается на публичный
[proc_bsdinfo XNU](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/sys/proc_info.h).
Тесты подставных системных ответов не заменяют проверку macOS.

completion_git.py сохраняет checkpoint после Git-действий. artifact_links.py
пересчитывает Markdown-ссылки отчётов. Общие инструкции и скиллы доступны из
ресурсов установки. Подробная семантика: git-policy.md, worker.md и recovery.md.
Установка из checkout намеренно сохраняет эту структуру; пакет PyPI не требуется.
