# Установка и обновление

Нужны Linux x86_64/ARM64 или macOS ARM64 (Apple Silicon), herdr и авторизованный
CLI агента; для задач с кодом — Git. macOS Intel не поддерживается.
Для сборки из исходников нужны Rust 1.91.0 и linker платформы. Готовая поставка
работает без Python и Rust toolchain.
Программа, herdr и CLI агента работают на одной машине. Границы проверенной
поддержки перечислены в [матрице](compatibility.md).

## Зависимости

Установи [herdr](https://herdr.dev/docs/install/) и CLI выбранного агента:
[Codex](https://learn.chatgpt.com/docs/codex/cli) или
[Claude Code](https://code.claude.com/docs/en/overview). Пройди штатный вход
и проверь интерактивный запуск клиента. Обе команды должны быть доступны в PATH;
credentials остаются в хранилище провайдера вне state панели.

## Новый экземпляр с Codex

Выбирай prefix вне исходников, ресурсов, state, bindings и файла config.
Каталог другой программы и symlink в управляемой установке отклоняются.
Поставка содержит manifest с SHA-256 для проверки управляемых файлов.

Скачай архив для своей платформы и SHA256SUMS со страницы [Releases](https://github.com/Bragaman/control-workflow/releases). Названия содержат версию и платформу: linux-x86_64, linux-aarch64 или macos-aarch64. Например, для Linux x86_64 (на macOS замени имя архива на `macos-aarch64`):

```sh
export CW_VERSION=v0.1.0
export CW_ARCHIVE="control-workflow-$CW_VERSION-linux-x86_64.tar.gz"
curl -fLO "https://github.com/Bragaman/control-workflow/releases/download/$CW_VERSION/$CW_ARCHIVE"
curl -fLO "https://github.com/Bragaman/control-workflow/releases/download/$CW_VERSION/SHA256SUMS"
# Выполняй в каталоге с обоими файлами; работает и на macOS.
grep "  $CW_ARCHIVE$" SHA256SUMS | shasum -a 256 -c -
mkdir -p control-workflow-download
tar -xzf "$CW_ARCHIVE" -C control-workflow-download
export CW_PROGRAM="$HOME/.local/share/control-workflow/program"
./control-workflow-download/bin/control-workflow install --prefix "$CW_PROGRAM"
```

Проверяй успешный результат `OK` до распаковки; сумма обнаруживает повреждение, но сама по себе не подтверждает авторство. Пакеты macOS не подписаны и не notarized: Gatekeeper может заблокировать запуск; разрешай запуск только после проверки источника и по настройкам своей системы.

Для установки из исходников вместо загрузки выполни из checkout:

```sh
export CW_PROGRAM="$HOME/.local/share/control-workflow/program"
./scripts/install.sh --prefix "$CW_PROGRAM"
```

Затем в той же оболочке выбери новые каталоги экземпляра:

```sh
export CW_STATE="$HOME/.local/state/control-workflow-demo"
export CW_BINDINGS="$HOME/.local/state/control-workflow-demo-bindings"
export CW_CONFIG="$HOME/.config/control-workflow-demo/config.json"
export CW_MANAGER="$HOME/control-workflow-manager-demo"
mkdir -p "$(dirname "$CW_CONFIG")" "$CW_MANAGER"
cp "$CW_PROGRAM/examples/config-codex.json" "$CW_CONFIG"
"$CW_PROGRAM/bin/control-workflow" --state-root "$CW_STATE" --bindings-root "$CW_BINDINGS" --config "$CW_CONFIG" doctor
"$CW_PROGRAM/bin/control-workflow" --state-root "$CW_STATE" --bindings-root "$CW_BINDINGS" --config "$CW_CONFIG" environment
```

Проверь выбранные пути в environment. В doctor ожидаются herdr.found и codex.found со значением true;
для задач с кодом — также git.found=true. Doctor создаёт
state и bindings и проверяет запись временным файлом; без --herdr-session он
не обращается к серверу. Он не проверяет авторизацию или доступность модели.
Пример config-codex подключает встроенную политику моделей.
Выбор профиля и миграция старого config описаны в [model-policy.md](model-policy.md).
Пример Claude сохраняет прежний model=default; для него подготовь политику с моделями Claude.

По поручению пользователя подключи скиллы управляющего выбранной установки:

```sh
"$CW_PROGRAM/bin/control-workflow" skills --destination "$CW_MANAGER/.agents/skills"
"$CW_PROGRAM/bin/control-workflow" skills --destination "$CW_MANAGER/.agents/skills" --action check
```

[Codex обнаруживает .agents/skills и следует symlink](https://learn.chatgpt.com/docs/build-skills).
После обновления с добавленными скиллами повтори команду `skills --destination` для каждого каталога: она добавит недостающие ссылки.
Для Claude используй .claude/skills в каталоге управляющего и пример
config-claude.json: [инструкция Claude Code](https://code.claude.com/docs/en/skills).
Установщик не перезаписывает AGENTS.md и CLAUDE.md; чужие файлы или ссылки
блокируют операцию. При чтении ссылок из SKILL.md разрешай их относительно
физического исходника после разрешения symlink.

Создай именованную сессию herdr из каталога управляющего:

```sh
cd "$CW_MANAGER"
herdr --session control-workflow-demo
```

Это интерактивная команда: в herdr 0.9.3 она создаёт сессию, если её ещё нет,
или подключается к существующей. Синтаксис описан в
[CLI herdr](https://herdr.dev/docs/cli-reference/).
В открывшейся оболочке запусти codex из каталога управляющего и отправь запрос,
заменив все /absolute/path реальными абсолютными путями из environment:

```text
Управляй экземпляром control-workflow:
программа /absolute/path/program/bin/control-workflow,
--state-root /absolute/path/state,
--bindings-root /absolute/path/bindings,
--config /absolute/path/config.json,
--herdr-session control-workflow-demo.
Прочитай /absolute/path/program/docs/control-entry.md.
Используй $control-list и покажи задачи. Новые задачи пока не запускай.
```

Скилл не выбирает экземпляр. Управляющий должен использовать полный путь
программы и эти параметры перед каждой подкомандой. Проверить имя сессии можно
через herdr session list; явный --herdr-session нужен при вызове вне herdr.
Если скиллы не появились, перезапусти Codex. Абсолютный путь к SKILL.md позволяет
прочитать инструкцию вручную, но не регистрирует команду.

Далее выполни [синтетический цикл](../examples/task.md) в выбранном экземпляре.

## Обновление

Перед переключением остановите новые запуски, дождитесь завершения команд запуска
и приёмки и текущего хода каждого исполнителя. Уже запущенный процесс продолжает
работать со старым бинарником; документы и навыки доступны по прежнему пути и
заменятся при установке. State, config и bindings не меняются. Установщик
проверяет прежние управляемые файлы, отказывает при ручных изменениях или чужих
файлах и сохраняет прежнюю программу в backup. Параллельные вызовы нового
установщика для одного prefix блокируются; не запускайте одновременно старый
установщик, который такой блокировки не знает.

Если процесс установки аварийно остановился между переименованиями, сверьте prefix
и соседние каталоги `.workflow-stage-*` и `.workflow-backup-*` до повторного
запуска. Для отката дождитесь того же безопасного окна, переименуйте новую
программу в отдельный каталог и верните указанный backup на прежний prefix.
Данные не откатываются; сначала убедитесь в совместимости старой версии с ними.
Не откатывайте данные поверх параллельной работы.

Версию, в которой команды `update` ещё нет, нужно один раз обновить прежним
способом: скачать и проверить поставку, распаковать её вне prefix и вызвать её
`bin/control-workflow install --prefix "$CW_PROGRAM"`. Для исходников сначала
соберите бинарник и вызывайте install из checkout вне prefix. После этого доступно
штатное обновление из GitHub. Не запускайте установщик из заменяемого каталога.

### Из локальных исходников

В checkout на Linux или macOS с Rust 1.91.0 и linker обновите выбранный prefix
одной командой (пути в примере замените своими):

```sh
/path/to/control-workflow/scripts/install.sh --prefix "$CW_PROGRAM"
```

Скрипт собирает текущее содержимое checkout через `cargo build --locked --release`
для host target и передаёт установку штатному `install`. Он не выполняет `git pull`,
`reset` или `checkout` и не требует опубликованного release. При ошибке сборки
установленная программа не меняется. Сборка может потребовать доступ к реестру
Cargo, если зависимости отсутствуют локально. JSON результата установки содержит путь backup. Относительный prefix разрешается
от каталога вызова. Это также способ однократно перейти на версию с `update`.

### Из GitHub Releases

Для уже установленной версии с командой `update` на той же машине выполните:

```sh
"$CW_PROGRAM/bin/control-workflow" update --check  # текущая и последняя стабильная версия
"$CW_PROGRAM/bin/control-workflow" update
# При необходимости выбрать конкретный release:
"$CW_PROGRAM/bin/control-workflow" update --version v0.1.0
```

Команда определяет prefix по собственному бинарнику. Нужны `curl`, `gzip`, HTTPS и
доступ к GitHub Releases; GitHub API сообщает последний стабильный выпуск. Можно
указать конкретный опубликованный tag (в том числе `vX.Y.Z-rc.N`). Вывод JSON
содержит версию, платформу, результат и путь `installation.backup`. Одинаковая
версия даёт `unchanged`; --check возвращает сведения без установки. Для предварительного
выбора конкретной версии используйте вместе `--check --version vX.Y.Z`.
Проверка суммы обнаруживает повреждение передачи, но сумма и архив получены из
одного источника и не подтверждают авторство. Выпуски macOS не подписаны и не
notarized. Поддержка ограничена Linux x86_64/ARM64 и macOS ARM64.

Снять только свои ссылки: bin/control-workflow skills --destination /path/skills
--action remove. Чужие файлы и ссылки блокируют операцию до изменений.
Перенос программы в другой prefix требует снять прежние подключения и
подключить новые. Ветки задач и их артефакты при этом сохраняются отдельно.

## Вызов скиллов

Вызывай нужный скилл явно: в Codex — $control-list, в Claude Code — /control-list.
Обычные запросы тоже поддерживаются. Явное имя не убирает каталог скиллов
из контекста. Скилл выбирает инструкцию, а экземпляр
задают путь программы и параметры: [control-entry.md](control-entry.md).
Ссылки внутри скилла читай по их условиям; уже известный контракт повторно
нужен только при смене экземпляра или восстановлении контекста.
