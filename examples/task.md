# Новый экземпляр и полный цикл

Это синтетическая проверка инструкции в проекте «Справочник».
Сначала выполни [установку и подключение управляющего](../docs/install.md).
Все пути ниже вымышленные. Замени /path/program, /path/state, /path/bindings и
/path/config.json путями программы, state, bindings и файла config из установки.
Используй созданные для примера demo-каталоги и сессию control-workflow-demo;
рабочее хранилище с реальными задачами для примера не подходит.

```sh
python3 /path/program/scripts/doctor.py --state-root /path/state --bindings-root /path/bindings --config /path/config.json
python3 /path/program/scripts/control.py --state-root /path/state --bindings-root /path/bindings --config /path/config.json project-create demo --name 'Справочник'
python3 /path/program/scripts/control.py --state-root /path/state --bindings-root /path/bindings --config /path/config.json task-create demo-check --project demo --title 'Проверить инструкцию' --goal 'Прочитать docs/worker.md и docs/control-entry.md установленной программы; подготовить отчёт о ролях, выборе экземпляра и приёмке, указать неоднозначности, если они есть' --acceptance 'План сохранён; отчёт с примерами или ссылками на прочитанные правила передан через finish' --no-code --task-kind implementation
python3 /path/program/scripts/control.py --state-root /path/state --bindings-root /path/bindings --config /path/config.json --herdr-session control-workflow-demo open demo-check
```

open вернёт RUN_ID и статус. Управляющий показывает статус и цель, не ждёт
исполнителя; работа продолжается в отдельном табе тестовой сессии.
Исполнитель прочитает assignment, покажет и сохранит план, выполнит работу и
finish с отчётом по worker.md. При вопросе общайся в текущем чате; если просишь
отложить, block сохраняет checkpoint. answer записывает ответ, после него
поручи продолжить в том же табе. По готовности прочитай результат, затем явно
поручи управляющему complete demo-check: эта команда принимает задачу и
закрывает её таб. Если результат требует доработки, сначала попроси управляющего
вернуть задачу исполнителю. Исполнитель уточнит отчёт в том же запуске и
повторно передаст его через finish; после проверки можно поручить приёмку.

Для code создай отдельный синтетический Git-проект с HEAD, привяжи его через
project-set demo --repo /path/demo-repo и создавай задачу с --code. Исполнитель
меняет только worktree; приёмка сохраняет код по docs/git-policy.md. Базовый
тест no-code не требует Git origin или доступа к чужой инфраструктуре.
