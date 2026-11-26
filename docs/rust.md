# Rust CLI и MCP

CLI и MCP используют общее Rust-ядро. [Установка и обновление](install.md)
описывают постоянную поставку, config и регистрацию скиллов.

## Разработка и проверки

Установи Rust из rust-toolchain.toml и linker платформы. Для проверок также
нужны Python 3.10+ с venv и Git. Из checkout:

```sh
python3 -m venv target/check-venv
. target/check-venv/bin/activate
python3 -m pip install -r requirements-dev.txt
sh scripts/check.sh
```

Скрипт `scripts/check.sh` запускает Rust fmt/clippy, Ruff, тесты Rust,
release-сборку, аудит поставки и контрактные тесты Python. CI вызывает тот же скрипт.
Тесты вызывают CLI/MCP с временными state/config/bindings, Git remote и fake herdr;
реальные агенты и табы не запускаются. Проверки реальных адаптеров выполняются
отдельно по [матрице приёмки](compatibility.md).

В Linux CI запускает скрипт в отдельном PID namespace с собственным `/proc`
от имени пользователя runner. Тесты очистки проверяют, что при недоступных
ссылках процесса панель сохраняет данные. Если посторонний процесс мешает
проверкам на Linux, запусти скрипт в таком же окружении. Для запуска команды
без sudo нужны разрешённые пользовательские пространства имён.

```sh
unshare --user --map-current-user --pid --fork --mount-proc --kill-child \
  setpriv --no-new-privs sh scripts/check.sh
```

После изменения файлов поставки обнови distribution.json:

```sh
python3 scripts/release_audit.py --write-manifest
python3 scripts/release_audit.py
```

Аудит сверяет положительный список, SHA-256, локальные Markdown-ссылки и все
достижимые Git-деревья по встроенным шаблонам секретов и личных данных.
Он не гарантирует обнаружения произвольного секрета; метаданные Git проверяй
отдельно перед публикацией.

## Сайт документации

Сайт собирается MkDocs Material из README и пользовательских страниц docs/guide.
Единственные поддерживаемые исходники остаются на прежних местах. Скрипт берёт
README, лицензию и docs/guide из distribution.json и создаёт временное дерево target/docs-src;
генерируемые файлы не коммитятся. Служебные docs/skills не публикуются и не входят
в поиск сайта. Пользовательские страницы и SVG входят в комплект CLI вместе с README.

```sh
python3 -m venv target/docs-venv
. target/docs-venv/bin/activate
python -m pip install -r requirements-docs.txt
python scripts/prepare_docs.py
python -m mkdocs build --strict
python -m mkdocs serve
```

После изменения исходников повтори prepare_docs.py. Статический результат находится
в target/site. Зависимости сайта закреплены отдельно от runtime и проверок Rust.
Workflow documentation проверяет сборку в PR и публикует main через GitHub Actions
на [GitHub Pages](https://bragaman.github.io/control-workflow/). В настройках Pages
выбран источник GitHub Actions; ручной workflow_dispatch повторяет публикацию main.
Навигация задаётся в mkdocs.yml, поиск работает локально в браузере.

## Самостоятельная поставка и MCP

Бинарник встраивает docs/skills/config/examples. Создай пакет в новом каталоге
вне исходников, установки и данных:

```sh
cargo build --locked --release
target/release/control-workflow package --prefix /path/to/native-package
printf '{}\n' > /path/to/test-preferences.json
/path/to/native-package/bin/control-workflow --state-root /path/to/test-state --bindings-root /path/to/test-bindings --config /path/to/test-preferences.json environment
```

Пустой config использует defaults. Выбор путей описан в
[runtime-layout.md](runtime-layout.md), постоянная установка, откат и подключение
скиллов — в [install.md](install.md).
Для испытаний herdr создай отдельную именованную сессию командой
`herdr --session sandbox` и проверь имя через `herdr session list`.

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

MCP закрепляет config/runtime при запуске процесса: после их изменения
перезапусти сервер. HTTP-транспорт, автоматическая миграция данных и изменение
подключений клиента не реализованы.

Протокол stdio и структура tools описаны в [спецификации транспорта](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports) и [спецификации tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools).

## Совместимость JSON

Файловое JSON-ядро читает константы NaN, Infinity и -Infinity, которые сохраняет штатный Python json. Их canonical-представление и CLI совместимы с Python. Вход JSON-RPC остаётся строгим JSON. В структурированном MCP-ответе такие числа заменяются строковыми тегами, а nonFiniteNumbers сопоставляет JSON Pointer с тегом; это сохраняет различие чисел и обычных строк. Поле content.text содержит JSON-представление, совместимое с Python. Например, числовой Infinity возвращается так:

```json
{
  "result": {"value": "Infinity"},
  "nonFiniteNumbers": {"/value": "Infinity"}
}
```

Карта появляется только при наличии таких чисел; обычная строка "Infinity" в неё не входит. Пути относятся к result, используют экранирование JSON Pointer (~0 для ~, ~1 для /) и десятичные индексы массивов.

## Версии и выпуск

До 1.0 используется SemVer с договорённостью: patch повышается для совместимых исправлений, minor — для возможностей и несовместимых изменений. Последние явно отмечаются как breaking в release notes; стабильность API до 1.0 не обещается. Предварительный выпуск имеет вид v0.2.0-rc.1, помечается prerelease и не становится latest. Версия Cargo содержит суффикс rc без v; тег добавляет v. Версии меняй осознанно.

Перед выпуском обнови версии в Cargo.toml и Cargo.lock и подготовь краткие release notes, явно указав breaking changes. После приёмки и отправки кода в main на машине сопровождающего проверь чистое дерево, коммит и отсутствие тега, затем создай аннотированный тег и отправь его:

```sh
git status --short
git log -1 --oneline
git ls-remote --tags origin 'refs/tags/v0.1.0*'
git tag -a v0.1.0 -m 'Release v0.1.0'
git push origin refs/tags/v0.1.0
```

Native CI проверяет тег, Cargo.toml, Cargo.lock и бинарник, выполняет `scripts/check.sh`, проверяет распакованные пакеты и публикует три архива и SHA256SUMS только после всех jobs. Первый выпуск получает краткое описание; последующие получают автоматически сформированные notes, которые сопровождающий должен проверить и дополнить breaking changes. Стабильный выпуск становится latest. PR не получает прав публикации. При ошибке публикации может остаться черновик: проверь его и причину ошибки, удали только заведомо неполный черновик вручную перед повтором. Повтор не меняет существующий выпуск или его файлы; опубликованный выпуск не заменяй новыми бинарниками под тем же тегом. После push проверь результат Actions, статус latest/prerelease, состав файлов, скачай архивы и проверь SHA256SUMS. Фактический выпуск не происходит до отправки тега.
