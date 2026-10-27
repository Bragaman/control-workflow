# Установка и обновление

Нужны Linux/macOS, herdr и авторизованный CLI агента; для задач с кодом — Git.
Для сборки из исходников нужны Rust 1.91.0 и linker платформы. Готовая поставка
работает без Python и Rust toolchain.
Программа, herdr и CLI агента работают на одной машине. Границы проверенной
поддержки перечислены в [матрице](compatibility.md).

## Зависимости

Установи herdr по [официальной инструкции](https://herdr.dev/docs/install/).
Для Linux/macOS она предлагает установщик или пакетный менеджер. Вариант
с загрузкой скрипта позволяет сначала просмотреть его:

```sh
curl -fsSL https://herdr.dev/install.sh -o /tmp/herdr-install.sh
less /tmp/herdr-install.sh
sh /tmp/herdr-install.sh
herdr --version
```

Установи Codex CLI и пройди вход по
[официальной инструкции OpenAI](https://learn.chatgpt.com/docs/codex/cli).
Перед первым запуском панели проверь, что команда codex доступна в PATH и
интерактивный codex открывается с твоей учётной записью. Авторизация хранится
вне state панели. Для Claude Code используй его штатную установку и вход;
цикл с реальным CLI этого адаптера пока не принят.

## Новый экземпляр с Codex

Начни в корне проверенного checkout control-workflow. Команды ниже используют
новые каталоги; если они уже заняты, выбери другие. Архив нужной ОС и архитектуры доступен в артефакте native-OS-ARCH
workflow native-rust в CI репозитория: внутри лежит tar.gz и SHA256SUMS.
Сверь SHA-256 архива с SHA256SUMS и распакуй его в CW_PROGRAM; команды сборки и install
ниже тогда пропусти. В одной оболочке выполни:

```sh
export CW_PROGRAM="$HOME/.local/share/control-workflow/program"
export CW_STATE="$HOME/.local/state/control-workflow-demo"
export CW_BINDINGS="$HOME/.local/state/control-workflow-demo-bindings"
export CW_CONFIG="$HOME/.config/control-workflow-demo/config.json"
export CW_MANAGER="$HOME/control-workflow-manager-demo"
cargo build --locked --release
cargo test --locked
target/release/control-workflow install --prefix "$CW_PROGRAM"
mkdir -p "$(dirname "$CW_CONFIG")" "$CW_MANAGER"
cp "$CW_PROGRAM/examples/config-codex.json" "$CW_CONFIG"
"$CW_PROGRAM/bin/control-workflow" --state-root "$CW_STATE" --bindings-root "$CW_BINDINGS" --config "$CW_CONFIG" doctor
"$CW_PROGRAM/bin/control-workflow" --state-root "$CW_STATE" --bindings-root "$CW_BINDINGS" --config "$CW_CONFIG" environment
```

Проверь выбранные пути в environment и found в выводе doctor. Doctor создаёт
state и bindings и проверяет запись временным файлом; без --herdr-session он
не обращается к серверу. Он не проверяет авторизацию или доступность модели.
Пример config-codex подключает встроенную политику: normal → sol, complex → sol-high.
Выбор профиля и миграция старого config описаны в [model-policy.md](model-policy.md).
Пример Claude сохраняет прежний model=default; для него подготовь политику с моделями Claude.

Подключи восемь скиллов управляющего:

```sh
"$CW_PROGRAM/bin/control-workflow" skills --destination "$CW_MANAGER/.agents/skills"
"$CW_PROGRAM/bin/control-workflow" skills --destination "$CW_MANAGER/.agents/skills" --action check
```

[Codex обнаруживает .agents/skills и следует symlink](https://learn.chatgpt.com/docs/build-skills).
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
Программная установка, конфигурация, ссылки скиллов и создание записей проверены
в новом временном окружении. Загрузка зависимостей и интерактивный вход на пустой
машине не проверялись; полный цикл с реальным CLI ограничен матрицей поддержки.

## Обновление

Останови запуск новых задач. Дождись завершения команд запуска и приёмки,
а также текущего хода каждого работающего исполнителя. Распакуй новую поставку
в отдельный каталог и вызови её `bin/control-workflow install --prefix PATH`
с прежним prefix. При обновлении из исходников сначала собери новый бинарник
и вызови `target/release/control-workflow install --prefix PATH`.
Используй бинарник вне обновляемого каталога. Состояние и config не переносится.
Установщик сверяет хеши прежних управляемых файлов, отказывает при чужих файлах
или ручных изменениях, готовит новую копию и сохраняет старую в backup,
путь которого возвращает в JSON. Проверенные ссылки скиллов указывают на
стабильный prefix, поэтому повторное подключение не нужно.

Если процесс установки аварийно остановился между переименованиями, сначала
сверь prefix и соседние каталоги stage/backup. Не запускай повторное обновление
до сверки: это локальная операция без общей транзакции нескольких каталогов.
Для отката после завершения команд запуска и приёмки и текущих ходов исполнителей переименуй новую программу в
отдельный каталог и верни backup на прежний prefix. Config/state/bindings
остаются на своих местах. Не откатывай данные поверх параллельной работы.

Снять только свои ссылки: bin/control-workflow skills --destination /path/skills
--action remove. Чужие файлы и ссылки блокируют операцию до изменений.
Перенос программы в другой prefix требует снять прежние подключения и
подключить новые. Ветки задач и их артефакты при этом сохраняются отдельно.

## Вызов скиллов и объём контекста

После подключения вызывай нужный скилл явно: в Codex — `$control-list`,
в Claude Code — `/control-list` в начале сообщения. Обычный запрос тоже
поддерживается: описания оставлены для автоматического выбора. Если скилл
не зарегистрирован, укажи агенту прочитать его SKILL.md по абсолютному пути;
команда при этом не регистрируется.

Тело загружается при выборе скилла; явное имя само по себе не убирает каталог
из контекста. Установку и state укажи в запросе.
Уже прочитанный общий документ не перечитывай без изменения экземпляра или
необходимости восстановить контекст. Ссылки внутри скилла читай по их условиям.

Режим только явного вызова настраивается по-разному:
[Codex](https://learn.chatgpt.com/docs/build-skills) —
`policy.allow_implicit_invocation: false` в agents/openai.yaml;
[Claude Code](https://code.claude.com/docs/en/skills) —
`disable-model-invocation: true` во frontmatter. Здесь эти ограничения не
включены: workflow использует переходы между скиллами и обычные запросы.
Отключение через Codex skills.config enabled=false не заменяет режим явного вызова.
