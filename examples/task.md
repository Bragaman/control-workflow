# Новый экземпляр и полный цикл

Все пути ниже вымышленные. Замени /path/program, /path/state, /path/bindings и
/path/config.json своими отдельными каталогами; выбери собственную named herdr
session demo-acceptance. Пример не направляется в существующий state.

```sh
python3 /path/program/scripts/doctor.py --state-root /path/state --bindings-root /path/bindings --config /path/config.json
python3 /path/program/scripts/control.py --state-root /path/state --bindings-root /path/bindings --config /path/config.json project-create demo --name 'Пример'
python3 /path/program/scripts/control.py --state-root /path/state --bindings-root /path/bindings --config /path/config.json task-create demo-check --project demo --title 'Проверить пример' --goal 'Проверить цикл панели' --acceptance 'Отчёт и checkpoint сохранены' --no-code --task-kind implementation
python3 /path/program/scripts/control.py --state-root /path/state --bindings-root /path/bindings --config /path/config.json --herdr-session demo-acceptance open demo-check
```

open вернёт RUN_ID и статус; дождись запуска в своей тестовой session.
Исполнитель прочитает assignment, покажет и сохранит план, выполнит работу и
finish с отчётом по worker.md. При вопросе общайся в текущем чате; если просишь
отложить, block сохраняет checkpoint. answer записывает ответ, после него
поручи продолжить в том же табе. По готовности прочитай результат, затем явно
поручи complete demo-check. Это принимает задачу и закрывает её таб.

Для code создай отдельный синтетический Git-проект с HEAD, привяжи его через
project-set demo --repo /path/demo-repo и создавай задачу с --code. Исполнитель
меняет только worktree; приёмка сохраняет код по docs/git-policy.md. Базовый
тест no-code не требует Git origin или доступа к чужой инфраструктуре.
