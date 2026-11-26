# Установка

Панель и CLI агента работают на одной машине. Нужны Linux x86_64/ARM64
или macOS ARM64, авторизованный Codex или Claude Code. Для задач с кодом нужен Git.
Готовая поставка работает без Python и Rust.

## 1. Подготовьте зависимости

Установите CLI выбранного агента: [Codex](https://learn.chatgpt.com/docs/codex/cli) или
[Claude Code](https://code.claude.com/docs/en/overview).
Пройдите вход у провайдера и проверьте интерактивный запуск агента.

Для штатного TUI установите [herdr](https://herdr.dev/docs/install/).
Для работы с одним исполнителем Codex через Desktop, TUI и API отдельно
установите herdr-core на машине исполнителя. Команда `hc --version` проверяет,
что оболочка нашла его в PATH. [Официальный установщик для Linux и macOS](https://github.com/herdrdev/herdr-core#install).

```sh
curl -fsSL https://releases.herdr.dev/core/install.sh | sh
hc --version
```

## 2. Установите панель

Со страницы [Releases](https://github.com/Bragaman/control-workflow/releases) скачайте
архив для своей платформы и файл SHA256SUMS. В каталоге загрузки проверьте суммы:

Linux:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

На macOS замените значение `CW_ARCHIVE` именем скачанного архива.

```sh
CW_ARCHIVE="control-workflow-vX.Y.Z-macos-aarch64.tar.gz"
grep "  $CW_ARCHIVE$" SHA256SUMS | shasum -a 256 -c -
```

Дождитесь `OK` для своего архива. Скачивайте архив и суммы из официального
репозитория. После распаковки выполните команду установки из каталога архива.

```sh
export CW_PROGRAM="$HOME/.local/share/control-workflow/program"
./bin/control-workflow install --prefix "$CW_PROGRAM"
```

Выберите новый каталог установки вне исходников и данных панели. Пакеты macOS
не подписаны и не notarized: Gatekeeper может потребовать разрешить запуск.

Для сборки из исходников установите Rust 1.91.0 и linker платформы, затем из checkout:

```sh
./scripts/install.sh --prefix "$HOME/.local/share/control-workflow/program"
```

## 3. Подготовьте управляющего

В той же оболочке создайте конфигурацию и каталог, из которого будете запускать агента:

```sh
export CW_MANAGER="$HOME/control-workflow-manager"
export CW_CONFIG="$HOME/.config/control-workflow/config.json"
mkdir -p "$CW_MANAGER" "$(dirname "$CW_CONFIG")"
cp "$CW_PROGRAM/examples/config-codex.json" "$CW_CONFIG"
```

Пример конфигурации запускает Codex в штатном TUI. Если вы установили herdr-core
для новых запусков, выберите `hc-codex` в [настройках](settings.md) перед проверкой.

```sh
"$CW_PROGRAM/bin/control-workflow" --config "$CW_CONFIG" doctor
"$CW_PROGRAM/bin/control-workflow" --config "$CW_CONFIG" environment
"$CW_PROGRAM/bin/control-workflow" skills --destination "$CW_MANAGER/.agents/skills"
```

Для Codex проверьте в выводе doctor поле `codex.found` и поле выбранного
адаптера (`herdr.found` или `hc.found`). Оба значения должны быть true.

Для задач с кодом также нужно `git.found=true`. Doctor не проверяет вход у провайдера или доступность модели. В environment видны
выбранные пути: сохраните их для управляющего. Последняя команда подключает
поставляемые навыки к каталогу управляющего.

Пример выше использует Codex. Для Claude Code выберите адаптер herdr-claude,
подготовьте модели Claude в конфигурации и зарегистрируйте навыки в .claude/skills.
Адаптер Claude экспериментальный.

## 4. Откройте сессию

Если выбрали herdr-core, запустите управляющего командой `codex` из
`"$CW_MANAGER"`. При выборе экземпляра передайте пути из environment.
Подключение к исполнителю описано в [настройках](settings.md).

Для штатного TUI откройте сессию:

```sh
cd "$CW_MANAGER"
herdr --session control-workflow
```

В открытой оболочке запустите codex. Отправьте сообщение, заменив пути значениями
из environment:

```text
Управляй экземпляром control-workflow:
программа /path/to/program/bin/control-workflow,
--state-root /path/to/state,
--bindings-root /path/to/bindings,
--config /path/to/config.json,
--herdr-session control-workflow.
Покажи задачи. Новые задачи пока не запускай.
```

Теперь можно [создать первую задачу](first-task.md).
