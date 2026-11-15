# Регулярные задачи

Расписание хранится в выбранном `state-root` и содержит снимок выбранной задачи-шаблона. Расписание задаёт разрешённую для повторения работу; каждое срабатывание создаёт новую задачу и запускает её через обычный lifecycle. Приёмка и review остаются обязательными по настройкам задачи. Шаблон с зависимостями не поддерживается.

```sh
control-workflow --herdr-session SESSION schedule-create weekday-check \
  --template TASK_ID --days mon,tue,wed,thu,fri --time 09:00 --timezone Europe/Moscow
control-workflow schedule-list
control-workflow schedule-show weekday-check
control-workflow schedule-pause weekday-check
control-workflow schedule-resume weekday-check
control-workflow schedule-cancel weekday-check
```

При создании явно выберите экземпляр обычными параметрами state, bindings и config и работающую сессию herdr. Расписание привязано к её namespace и запускается только при совпадении namespace. Команда `schedule-tick` проверяет наступившие срабатывания; она требует тот же выбранный экземпляр и `--herdr-session SESSION` вне herdr. Программа не устанавливает фоновый scheduler. Для автоматической работы пользователь самостоятельно настраивает внешний периодический вызов tick на нужной машине. В выводе просмотра доступны режим внешнего tick и время последнего обычного tick (при восстановлении прерванного срабатывания оно не обновляется); отсутствие или давность проверки указывают, что автоматический запуск не подтверждён. Ошибки проверки namespace возвращаются вызывающему процессу; ошибки запуска задачи записываются в историю и в результат tick.

Tick рассматривает только последнее срабатывание за последние 24 часа, пропуская более старые. При возобновлении отсчёт начинается заново. Если предыдущая задача ещё не принята или не отменена, новое срабатывание записывается как пропущенное. Отмена расписания прекращает будущие срабатывания, но не останавливает уже созданную задачу. Tick и отмена сериализуются общей блокировкой: выигравший её tick может создать запуск до отмены.

При переходе часов несуществующее местное время пропускается; при повторе местного времени выбирается первое вхождение. База часовых поясов включена в бинарник и обновляется вместе с программой. История содержит время, задачу и запуск. После прерывания записи намерения запуска состояние `uncertain` требует ручной проверки задачи и запусков перед дальнейшими действиями; повторный tick автоматически не запускает её заново. До принятия или отмены этой задачи следующие срабатывания пропускаются; после ручной проверки при необходимости отмените задачу штатной командой task-set. Для сбоя с активным запуском используйте штатное восстановление запуска. При отказе запуска причина сохраняется в истории, а созданная задача остаётся для разбора.

## Linux: запуск через systemd user timer

Таймер вызывает tick через минуту после завершения предыдущей проверки, от имени пользователя, у которого работает herdr. Перед настройкой проверьте установку программы и доступ к нужной сессии. В примере программа установлена в `/opt/control-workflow`, сессия называется default. Замените эти значения и пути экземпляра на свои; `%h` в unit-файлах означает домашний каталог пользователя. Путь PATH должен включать каталоги herdr, Git и CLI выбранного агента.

Создайте файл `~/.config/systemd/user/control-workflow-tick.service`:

```ini
[Unit]
Description=Check control-workflow schedules

[Service]
Type=oneshot
Environment="PATH=%h/.local/bin:%h/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
ExecStart=/opt/control-workflow/bin/control-workflow --program-root /opt/control-workflow --state-root %h/.local/state/control-workflow --bindings-root %h/.local/state/control-workflow/bindings --config %h/.config/control-workflow/config.json --herdr-session default schedule-tick
TimeoutStartSec=infinity
KillMode=process
```

KillMode=process сохраняет подготовители, запущенные командой tick, после завершения service. Они продолжают обычный запуск исполнителя.

Рядом создайте `control-workflow-tick.timer`:

```ini
[Unit]
Description=Check control-workflow schedules every minute

[Timer]
OnActiveSec=1min
OnUnitInactiveSec=1min
AccuracySec=1s
Unit=control-workflow-tick.service

[Install]
WantedBy=timers.target
```

На выбранной Linux-машине следующие команды проверяют файлы и включают пользовательский таймер. Ручной запуск service сразу обработает наступившие расписания, поэтому выполняйте его после проверки выбранного экземпляра и разрешённых шаблонов.

```sh
systemd-analyze --user verify ~/.config/systemd/user/control-workflow-tick.service ~/.config/systemd/user/control-workflow-tick.timer
systemctl --user daemon-reload
systemctl --user start control-workflow-tick.service
journalctl --user -u control-workflow-tick.service -n 30 --no-pager
systemctl --user enable --now control-workflow-tick.timer
systemctl --user list-timers control-workflow-tick.timer
```

Журнал содержит JSON-результат tick: проверяйте ошибки в items и истории срабатываний, даже если service завершился успешно. Если herdr недоступен или namespace изменился, задачи автоматически не запустятся. Расписание хранит своё время и часовой пояс; минутный таймер только проверяет его, поэтому запуск может произойти немного позже указанного времени.

Пользовательский systemd обычно работает в течение пользовательской сессии. Для запуска после загрузки без входа пользователя администратор может включить linger командой `sudo loginctl enable-linger USERNAME`. Это сохраняет пользовательский менеджер systemd, но herdr и его сессию тоже нужно запускать независимо от интерактивного входа.

Отключить будущие проверки:

```sh
systemctl --user disable --now control-workflow-tick.timer
```

Отключение таймера не прерывает текущий tick или уже работающих исполнителей. Чтобы остановить только одно расписание, используйте schedule-pause или schedule-cancel.
