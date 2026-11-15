# control-workflow

Локальная панель на Rust для работы с агентами через CLI и MCP stdio.
Проект хранит общий контекст, задача — цель и критерии, запуск — задание,
план и артефакты отдельной попытки. Всё хранится в файлах и доступно для продолжения
после остановки агента.

Пользователь выбирает задачу, управляющий открывает исполнителя в herdr.
Для правок репозитория создаётся отдельный Git worktree; исследования и применение
готовых настроек выполняются без него. Исполнитель сохраняет результат через
finish и при auto проводит независимое ревью. Затем пользователь принимает работу через complete.
Публикация кода и очистка выполняются по выбранному [Git-режиму](docs/git-policy.md).

[Сайт документации](https://bragaman.github.io/control-workflow/) обновляется из main.

## Требования и первый запуск

Целевые платформы: Linux x86_64/ARM64 и macOS ARM64. Нужны herdr и авторизованный
CLI Codex или Claude Code; для задач с кодом — Git. Выпуск экспериментальный:
[границы поддержки](docs/compatibility.md). Готовая поставка работает без Python и Rust.

Из исходников нужны Rust из rust-toolchain.toml и linker платформы
(на macOS — Xcode Command Line Tools). Из checkout [репозитория](https://github.com/Bragaman/control-workflow) установи программу вне исходников:

```sh
export CW_PROGRAM="$HOME/.local/share/control-workflow/program"
./scripts/install.sh --prefix "$CW_PROGRAM"
```

Эта же команда обновляет установку из текущих исходников.
Готовые архивы опубликованы в [GitHub Releases](https://github.com/Bragaman/control-workflow/releases).
[Установка](docs/install.md) описывает проверку архива, выбор отдельных state/config/bindings,
регистрацию скиллов и запуск управляющего. Для установленной версии доступны
update --check и update; безопасное окно и откат описаны там же.

`control-workflow --version` показывает версию релиза; сборка из исходников дополнительно показывает commit и, при незакоммиченных изменениях, `-dirty`. Без Git metadata выводится `commit unknown`. Эти сведения фиксируются при сборке и не требуют Git при запуске.

## Сценарии

| Действие | Инструкция |
| --- | --- |
| Выбрать экземпляр | [Команда и пути](docs/control-entry.md), [настройки и данные](docs/runtime-layout.md) |
| Создать проект и задачу | [Проект](skills/control-project/SKILL.md), [задача](skills/control-task/SKILL.md), [полный пример](examples/task.md) |
| Выбрать модель | [Тип и сложность](docs/task-profile.md), [политика профилей](docs/model-policy.md) |
| Открыть выбранные задачи | [Запуск](skills/control-start/SKILL.md), [восстановление](docs/recovery.md) |
| Выполнить задание | [Контракт исполнителя](docs/worker.md), [план](docs/planning.md), [вопросы](docs/questions.md) |
| Проверить результат | [Прогресс и результат](skills/control-review/SKILL.md), [независимое ревью](docs/agent-review.md) |
| Принять или сохранить код | [Приёмка](skills/control-complete/SKILL.md), [сохранение без закрытия](skills/control-git-publish/SKILL.md) |
| Найти задачи и знания | [Списки](skills/control-list/SKILL.md), [память](skills/control-memory/SKILL.md), [сверка](skills/control-sync/SKILL.md) |
| Повторять задачу по расписанию | [Расписания и внешний tick](docs/schedules.md) |
| Обновить программу | [Установка и обновление](docs/install.md), [скилл обновления](skills/control-update/SKILL.md) |
| Разрабатывать или подключить MCP | [Проверки, MCP и выпуск](docs/rust.md), [устройство](docs/architecture.md) |

Документация описывает реализованные возможности. Локальные пути, учётные данные
и настройки моделей конкретного пользователя в поставку не входят.

[Лицензия MIT](LICENSE). История Git сохраняет исходные метаданные авторов.
