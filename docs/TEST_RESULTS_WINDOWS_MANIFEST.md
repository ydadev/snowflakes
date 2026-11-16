# Windows manifest и начальный DPI

Дата: 16 ноября 2026 года. Область: Windows manifest из поставки ТЗ, OS-04, P1/P2. Решение: [ADR 0097](adr/0097-embedded-windows-manifest.md).

## Результат

Добавлены исходный manifest и std-only linker-встраивание RT_MANIFEST/#1. Ресурс задаёт amd64, PerMonitorV2/legacy true/pm, совместимость Windows 10/11 и asInvoker/uiAccess=false. Для прототипов повторный DPI-вызов с ERROR_ACCESS_DENIED принимается только при подтверждённом PMv2; другие исходы не маскируются. Уровень существующего токена не изменяется.

До встраивания новый обязательный guard отклонил настоящий прежний EXE с Resource Directory 0/0. После выделения helper, но до изменения прежнего возврата Result, управляемая регрессия manifest-denial Failed; отдельный случай успеха/других ошибок Passed. После исправления Passed все три Rust-проверки, включая read-only запрос PMv2 у потока реально загруженного бинарного harness.

Первая release-сборка встроила корректный XML, но скриптовая проверка Failed из-за неверной формы аргумента SDK -manifest. Независимое извлечение и проверка XML подтвердили содержимое; аргумент исправлен на отдельный. Окончательные build-client и guard Passed в обеих редакциях PowerShell. Это исправление проверки, не принятие файла обходным путём.

| Проверка окончательных файлов | Результат |
| --- | --- |
| Три dpi_initialization | Passed в debug/release |
| Manifest XML policy, 20 assertions | Passed в PowerShell 7 и Windows PowerShell |
| Прежние build paths, 27 assertions с обязательным manifest | Passed в обеих редакциях |
| Настоящий build-client / SDK extraction + schema + XML policy | Passed |
| fmt/clippy | Passed |
| Общий tools/check.ps1 | Passed: 569 + 3 + 26 выполнений Rust, 47 ignored |
| CI окончательных файлов | Pending |

Окончательный общий прогон подтвердил 598 выполнений/572 уникальные проверки Rust, библиотеку 252 и прежние восемь smoke-отпечатков. Все три новых случая прошли в обеих сборках. Сборщики 35/31/27 и новая XML policy 20 assertions Passed в одной локальной редакции PowerShell; обе редакции отдельно проверили policy/guard. Schema 60, PHP 60+5 и SQLite/HTTP сохранили прежние результаты компонентов.

У окончательного локального EXE SHA-256 `7B682C0A164ECFCAA827E7D3B65A945B00466D7698DD4F826233BD8879ADDCA3`, размер 2252800 байт. Независимое чтение PE даёт Resource Directory RVA 2256896/size 1400; SDK извлёк ресурс #1 и проверил его XML. Вхождений частного профиля 0, прежний guard известных корней Passed. Нормальный EXE не исполнялся; read-only статус живого старого клиента подтвердил manual/rendering=false.

## Область проверок

Два Rust-сценария используют управляемые API-результаты и query callback: отказ доступа/PMv2, отказ доступа/неверный контекст, успех и иной HRESULT. Успех и другая ошибка не выполняют запрос контекста. Третья проверка вызывает GetThreadDpiAwarenessContext/AreDpiAwarenessContextsEqual у своего тестового потока; она не создаёт HWND, не вызывает setters, не читает ввод и не меняет desktop/масштаб Windows.

XML-проверки учитывают namespace, identity/архитектуру/версию, Windows GUID, оба DPI-элемента, права, дубликаты, повреждённый XML, DTD, autoElevate и неподтверждённый maxversiontested. SDK проверяет уже извлечённый настоящий ресурс; одна лишь копия XML рядом с EXE не принимается. SDK-проверка не является запуском клиента.

Прежние отрицательные PE/UTF-fixtures продолжают проверять конкретные ранние ошибки путей/заголовка. Нейтральный bare header отдельно обязан не пройти из-за отсутствия manifest. Ожидаемый ненулевой native status после такого случая очищается только по завершении всех успешных assertions.

## Ограничения

SupportedOS GUID — декларация семейства ОС, не протокол испытаний Windows 10/11. Read-only PMv2 получен в тестовом бинарном target; нормальный графический клиент не запускался. Доставка WM_DPICHANGED, смешанный DPI/перенос, ввод/фокус/права, реальный S_OK и ресурсы полного клиента, hotplug и длительный прогон **Not run**. CRT-зависимость из прежнего аудита и офлайн-поставка остаются открытыми. P1/P2/P3 и полный проект не объявляются завершёнными.

## Повторение

~~~powershell
./tools/test-client-manifest.ps1
./tools/build-client.ps1
./tools/check-client-artifact.ps1
./tools/check.ps1
~~~

~~~text
cargo test -p snowflakes-client --locked --bin snowflakes dpi_initialization
cargo test -p snowflakes-client --release --locked --bin snowflakes dpi_initialization
~~~

Команды Rust не запускают UI; две — управляемые проверки, одна — чтение собственного контекста. Полный прогон использует прежние ограниченные тестовые стенды. Вода, сайт и поставка отложены; живой клиент не заменялся и не включался.
