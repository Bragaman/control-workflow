# Rust CLI и MCP

CLI и MCP используют общее Rust-ядро. [Установка и обновление](install.md)
описывают постоянную поставку, config и регистрацию скиллов.

Сборка Linux/macOS: установите Rust 1.91.0, Git и штатный linker платформы (на macOS Xcode Command Line Tools), затем выполните из checkout:

```sh
cargo build --locked --release
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

Создайте самостоятельную поставку в новом каталоге вне исходников и состояния:

```sh
target/release/control-workflow package --prefix /path/to/native-package
printf '{}\n' > /path/to/test-preferences.json
/path/to/native-package/bin/control-workflow --state-root /path/to/test-state --bindings-root /path/to/test-bindings --config /path/to/test-preferences.json environment
/path/to/native-package/bin/control-workflow --state-root /path/to/test-state --bindings-root /path/to/test-bindings --config /path/to/test-preferences.json doctor
```

Бинарник содержит ресурсы docs/skills/defaults и создаёт manifest с SHA-256. Готовая поставка не требует Python или Rust toolchain. `install --prefix PATH` создаёт новую установку либо обновляет только принадлежащую Rust-установщику: проверяет manifest, запрещает неизвестные файлы и symlink, сохраняет прежний каталог в backup. Возврат выполняется заменой программного каталога на этот backup при остановленных клиентах. Состояние не переносится. Каталог с файлами другой программы нельзя указывать как prefix.

Приоритет путей: аргументы CLI, CONTROL_PLANE_STATE/CONTROL_PLANE_CONFIG/CONTROL_PLANE_BINDINGS (прежнее CONTROL_PLANE_ROOT), пользовательский config, платформенные значения по умолчанию. На Linux используются XDG_STATE_HOME/XDG_CONFIG_HOME, на macOS — `~/Library/Application Support/control-workflow`. `--program-root` или CONTROL_WORKFLOW_PROGRAM задаёт каталог ресурсов; установленный бинарник определяет его как родителя bin. В config и runtime используются прежние поля, включая adapter, profiles, cwd, worktrees_root и herdr_executable.

Файл test-preferences.json с пустым JSON-объектом использует встроенные defaults; собственные профили можно задать по examples/config-codex.json или examples/config-claude.json в checkout. Для команд herdr создайте отдельную тестовую сессию (команда herdr session attach sandbox в отдельном терминале) и проверьте имя через `herdr session list --json`. Параметры MCP ниже используют эту сессию; не указывайте рабочую сессию для испытаний.

Запуск MCP stdio:

```sh
/path/to/native-package/bin/control-workflow --state-root /path/to/test-state --bindings-root /path/to/test-bindings --config /path/to/test-preferences.json --herdr-session sandbox mcp
```

Пример конфигурации клиента, поддерживающего stdio:

```json
{
  "mcpServers": {
    "control-workflow": {
      "command": "/path/to/native-package/bin/control-workflow",
      "args": ["--state-root", "/path/to/test-state", "--bindings-root", "/path/to/test-bindings", "--config", "/path/to/test-preferences.json", "--herdr-session", "sandbox", "mcp"]
    }
  }
}
```

Один процесс обслуживает последовательные JSON-RPC запросы: initialize, notifications/initialized, tools/list, tools/call, ping. Транспорт — JSON по строкам, stdout содержит только протокол; предел сообщения 16 MiB. Поддерживаются версии протокола 2024-11-05, 2025-03-26 и 2025-06-18. Каждый tool имеет явную схему; основные команды CLI представлены одноимёнными tools. Успешный результат возвращается в content и structuredContent.result; ошибка инструмента — в content с isError. Экземпляр закреплён параметрами процесса. Данные перечитываются на каждом вызове. Одновременные процессы CLI/MCP согласуют записи через flock одного state/.control.lock.

Запускайте только задачи, выбранные пользователем. `finish` передаёт в review; accept и complete требуют поручения назначенного reviewer. Подсказки MCP не заменяют проверку полномочий управляющим. После обрыва не повторяйте мутацию вслепую: прочитайте run, сохранённые intents, completion-request и git-completion. Внешние команды вызываются непосредственно, без shell. Git требует Git CLI; адаптеры — herdr и codex/claude, учётные данные предоставляются штатными CLI провайдеров. Claude сохраняет permission-mode default.

Сборки CI используют отдельные Linux и macOS runners, выполняют native tests и создают пакет с checksum. Успешная кросс-компиляция не подтверждает работу herdr/агентов на целевой ОС. Проверки реальных адаптеров требуют отдельной тестовой herdr-сессии и изолированных state/config/bindings; используйте их только при наличии такого окружения. HTTP-транспорт, автоматическая миграция и изменение подключений клиента не входят в Rust-программу.

Подключение скиллов отдельной установки выполняйте только по поручению пользователя:

```sh
/path/to/native-package/bin/control-workflow skills --destination /path/to/client/skills
/path/to/native-package/bin/control-workflow skills --destination /path/to/client/skills --action check
/path/to/native-package/bin/control-workflow skills --destination /path/to/client/skills --action remove
```

Перед изменениями проверяются все восемь ссылок. Чужие файлы и ссылки приводят к отказу; команда не заменяет их.

Повторяемые проверки контрактов, кроме cargo test:

```sh
CONTROL_WORKFLOW_BIN="$PWD/target/release/control-workflow" PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -v
```

Python 3.10+ нужен только разработчику для black-box тестов и аудита исходников.
Тесты вызывают бинарник напрямую, используя временные каталоги, локальные Git remote
и fake herdr. test_control.py проверяет жизненный цикл и Git, test_runtime.py —
изоляцию и артефакты, test_public.py — установку и Claude, test_mcp.py — те же
контракты через stdio. test_interop.py проверяет несколько Rust-процессов, блокировки,
конкурентные CLI/MCP open, восстановление парных записей и сохранённый формат JSON.
Стандартный json используется для проверки канонического хеша снимков; реализация
Python-панели для тестов не нужна. MCP-клиент из tests/mcp_client.py выполняет initialize
и tools/call через stdin/stdout процесса. Изменения state видны следующему вызову;
config/runtime закрепляются при запуске процесса, для их изменения перезапустите MCP.

Протокол stdio и структура tools описаны в [спецификации транспорта](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports) и [спецификации tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools).


Файловое JSON-ядро читает константы NaN, Infinity и -Infinity, которые сохраняет штатный Python json. Их canonical-представление и CLI совместимы с Python. Вход JSON-RPC остаётся строгим JSON. В структурированном MCP-ответе такие числа заменяются строковыми тегами, а nonFiniteNumbers сопоставляет JSON Pointer с тегом; это сохраняет различие чисел и обычных строк. Поле content.text содержит JSON-представление, совместимое с Python. Например, числовой Infinity возвращается так:

```json
{
  "result": {"value": "Infinity"},
  "nonFiniteNumbers": {"/value": "Infinity"}
}
```

Карта появляется только при наличии таких чисел; обычная строка "Infinity" в неё не входит. Пути относятся к result, используют экранирование JSON Pointer (~0 для ~, ~1 для /) и десятичные индексы массивов.
