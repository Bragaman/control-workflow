# Установка

Панель, herdr и CLI агента работают на одной машине. Нужны Linux x86_64/ARM64
или macOS ARM64, авторизованный Codex или Claude Code. Для задач с кодом нужен Git.
Готовая поставка работает без Python и Rust.

## 1. Подготовьте зависимости

Установите [herdr](https://herdr.dev/docs/install/) и CLI выбранного агента:
[Codex](https://learn.chatgpt.com/docs/codex/cli) или
[Claude Code](https://code.claude.com/docs/en/overview).
Пройдите вход у провайдера и проверьте интерактивный запуск агента.
Команды herdr и codex либо claude должны быть доступны в PATH.

## 2. Установите панель

Со страницы [Releases](https://github.com/Bragaman/control-workflow/releases) скачайте
архив для своей платформы и файл SHA256SUMS. В каталоге загрузки проверьте суммы:

Linux:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

macOS — укажите имя скачанного архива:

```sh
CW_ARCHIVE="control-workflow-v0.1.0-macos-aarch64.tar.gz"
grep "  $CW_ARCHIVE$" SHA256SUMS | shasum -a 256 -c -
```

Дождитесь `OK` для своего архива. В примере macOS замените версию в имени архива на скачанную. Сумма обнаруживает повреждения. Она не подтверждает авторство архива,
поэтому используйте официальный репозиторий. Распакуйте архив, перейдите в его каталог и выполните:

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
"$CW_PROGRAM/bin/control-workflow" --config "$CW_CONFIG" doctor
"$CW_PROGRAM/bin/control-workflow" --config "$CW_CONFIG" environment
"$CW_PROGRAM/bin/control-workflow" skills --destination "$CW_MANAGER/.agents/skills"
```

В doctor значения `herdr.found` и `codex.found` должны быть true.

Для задач с кодом также нужно `git.found=true`. Doctor не проверяет вход у провайдера или доступность модели. В environment видны
выбранные пути: сохраните их для управляющего. Последняя команда подключает
поставляемые навыки к каталогу управляющего.

Пример выше использует Codex. Для Claude Code выберите адаптер herdr-claude,
подготовьте модели Claude в конфигурации и зарегистрируйте навыки в .claude/skills.
Адаптер Claude экспериментальный.

## 4. Откройте сессию

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
