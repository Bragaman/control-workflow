# Control Workflow (рабочее имя)

Файловая панель для задач агентам: проект → выбранная задача → исполнитель
в herdr → артефакты → review → приёмка пользователя. Python 3.10+,
стандартная библиотека; Git нужен для задач с кодом.

Это кандидат для проверки. Linux проверяется автоматическими тестами с
подставным herdr. Живые циклы Codex и Claude Code и macOS требуют отдельной
приёмки: [матрица](docs/compatibility.md). Лицензия и имя пока не выбраны;
публичное распространение этой сборки не разрешено.

Программа, настройки пользователя, привязки машины и состояние хранятся
раздельно. Дистрибутив не содержит проектов, задач, запусков или credentials.

## Установка

Из проверенного checkout, в отдельный каталог программы:

```sh
python3 scripts/install.py --prefix "$HOME/.local/share/control-workflow/program"
```

Далее используй полный путь к установленному scripts/control.py.
[Установка и обновление](docs/install.md) описывают выбор профиля, подключение
восьми скиллов Codex/Claude и откат. [Пример цикла](examples/task.md) начинает
работу с новым экземпляром. [Выбор экземпляра](docs/control-entry.md),
[восстановление](docs/recovery.md) и [Git-приёмка](docs/git-policy.md)
задают рабочий контракт.

## Проверки

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -v
python3 scripts/release_audit.py
```

Тесты используют временные Git/state/config/bindings и fake herdr; модели
и живые табы не запускаются. CI повторяет тесты на Linux/macOS с Python
3.10 и 3.13. Наличие workflow в репозитории не означает успешный запуск CI.
