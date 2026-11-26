# control-workflow

Локальная панель для работы с агентами Codex и Claude Code. Вы описываете задачу,
агент выполняет её в отдельном терминале, а вы проверяете результат и принимаете работу.
Планы, отчёты и состояние сохраняются в файлах — к задаче можно вернуться после перерыва.

<a href="https://bragaman.github.io/control-workflow/docs/guide/installation/" class="md-button md-button--primary">Начать работу</a>
<a href="https://bragaman.github.io/control-workflow/docs/guide/first-task/" class="md-button">Первая задача</a>

![Вы выбираете задачу, управляющий открывает исполнителя, исполнитель передаёт результат на проверку. Принимает работу пользователь.](docs/guide/assets/roles.svg)

## Как устроена работа

Проект объединяет контекст и задачи. У каждой задачи есть цель, критерии готовности
и один или несколько запусков с планом и результатами. Штатный Herdr TUI запускает
Codex и Claude Code. С адаптером `hc-codex` к одному терминалу Codex можно
подключиться через Herdr Desktop, консоль или свой клиент.

Для изменения кода панель создаёт отдельный Git worktree. Исследования и применение
готовых настроек можно выполнять без него. Завершённую работу агент передаёт на
проверку; приёмка и публикация требуют вашего поручения.

## Дальше

- [Работа с задачами](https://bragaman.github.io/control-workflow/docs/guide/workflow/) — статусы, вопросы, ревью и Git.
- [Настройки](https://bragaman.github.io/control-workflow/docs/guide/settings/) — хранение данных, выбор модели и подключение к исполнителю.
- [Справка](https://bragaman.github.io/control-workflow/docs/guide/reference/) — команды и решение типичных проблем.

Выпуск экспериментальный. Целевые платформы — Linux x86_64/ARM64 и macOS ARM64.
На Linux проверены запуск Codex без кода и полный цикл задачи с Git worktree
через Herdr Core. Полный цикл с реальным CLI Claude Code ещё предстоит проверить.
Windows и macOS Intel не поддерживаются.

[Сайт документации](https://bragaman.github.io/control-workflow/) ·
[Исходный код](https://github.com/Bragaman/control-workflow) · [Лицензия MIT](LICENSE)
