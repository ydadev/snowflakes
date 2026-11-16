# Проверенная сборка и статический аудит клиента

Дата: 16 ноября 2026 года. Область: EN-10, правила публикации и основа Windows-клиента; упаковка P6 остаётся отложенной. Решение: [ADR 0096](adr/0096-neutral-build-paths.md).

## Наблюдения настоящего EXE

| Свойство | До исправления | После окончательного общего прогона |
| --- | --- | --- |
| SHA-256 локального EXE | 605AC47DA4F2B22CC4980F20C9F95A176778F3F67C049EACA371BADFA855D254 | 77250833B439916D5BA94CCE08D12649F8448E81C07FBC4B28F0162E1902E55A |
| Размер, байт | 2252800 | 2250752 |
| Формат | PE32+, AMD64, Windows GUI | Тот же формат |
| Вхождения частного профиля в диагностических Rust-путях | 35, все Cargo registry | 0; 35 нейтральных /cargo registry-путей |
| CodeView-ссылка | snowflakes.pdb | snowflakes.pdb |
| Прямые DLL-модули | 23 | Прежние 23 |
| Resource Directory RVA/size | Отдельно не снималось | 0/0, встроенного manifest нет |

Это хеш конкретного локального файла; одинаковый хеш на разных машинах не требуется и не заявляется. Бинарник не исполнялся. DUMPBIN и независимое чтение PE-полей подтвердили формат/импорты/ресурсы, новая постоянная проверка — заголовок и известные корни путей. EXE/PDB и исходные локальные дампы исключены из Git и не публиковались.

Новый [build-client](../tools/build-client.ps1) Passed. [Проверка артефакта](../tools/check-client-artifact.ps1) сначала отклонила прежний EXE, после пересборки Passed. Путь PDB уже был нейтральным, linker-флаги не менялись: устранены именно диагностические Rust-пути.

## EN-10 и зависимости

Нормальный [Gpu::new/draw](../crates/snowflakes-client/src/platform/weather.rs) получает поверхность из собственного swap.GetBuffer и рисует World/геометрию. [Prototype Renderer](../crates/snowflakes-client/src/prototype.rs) также создаёт собственный target; input-probe — отдельное собственное синтетическое окно со счётчиками. Диагностические --input-probe/--overlay-prototype явно маршрутизируются из main и остаются в release. Они не запускаются обычной погодной веткой.

CopyFromBitmap/Map для чтения пикселей находятся в weather::tests под cfg(test); worker_probe, control_probe и вызов capture_before_present также ограничены cfg(test). Тексты панели читаются у её собственных элементов, Scene читает класс/рамку окна; это не пиксели чужого окна. Производственных BitBlt, PrintWindow, GetWindowDC, Desktop Duplication или GraphicsCapture путей в просмотренном исходном контуре не найдено. В PE нет выбранных прямых импортов захвата и четырёх проверенных маркеров Native harness; эти отрицательные поиски служат только дополнением к анализу связей и cfg.

Прямые графические DLL: d2d1, d3d11, dcomp, dwmapi, dxgi, gdi32. Остальные Windows/UCRT-модули: advapi32, api-ms-win-core-synch-l1-2-0, api-ms-win-crt-{heap,locale,math,runtime,stdio,string}-l1-1-0, bcryptprimitives, comctl32, kernel32, ntdll, oleaut32, shell32, user32, wtsapi32. Отдельно импортируется vcruntime140.dll. Это прямые зависимости EXE, не полный транзитивный граф загрузки.

Для будущей поставки нужно обеспечить совместимый x64 Visual C++ Runtime согласно [правилам Microsoft](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170). Его наличие на всех ПК не предполагается. Runtime не устанавливался, CRT не переводился в static, пакет не готовился; офлайн-поставка/лицензионные уведомления остаются задачами P6.

## Проверки

| Проверка окончательных файлов | Результат |
| --- | --- |
| Аргументы/PE/UTF guard, 27 assertions | Passed в PowerShell 7 и Windows PowerShell |
| build-client.ps1 и настоящий EXE | Passed |
| Полный tools/check.ps1 с новой политикой | Passed: 566 + 3 + 23 выполнений Rust, 47 ignored |
| Восемь прежних smoke-отпечатков | Passed, сохранены |
| CI окончательных файлов | Passed: [3805dc1](https://github.com/ydadev/snowflakes/actions/runs/43779052453) |

CI исходников `3805dc1fb2133cc7f0affbd14bae35faba01e5a7` завершил оба задания успешно. Декодированный Windows-журнал подтвердил 592 выполнения Rust, восемь прежних smoke-отпечатков, сборщики 35/31 и новые 27 assertions в обеих редакциях PowerShell. Шаги Neutral build paths и Client artifact paths and PE Passed; последний проверил настоящий CI-release без запуска. Schema 60, PHP 60+5 и SQLite/HTTP также Passed. Хеш/35 вхождений в таблице относятся к локальным файлам, не к побитовому сравнению с CI.

Rust-код и число тестов не менялись: 592 выполнения/569 уникальных, библиотека 252. Пять cursor cadence и десять adaptive quality сценариев сохраняют debug/release. Полный локальный прогон включал 27 новых assertions и проверку настоящего release; сборщики 35/31 Passed в одной локальной редакции PowerShell. Schema 60, PHP 60+5 и SQLite/HTTP остались прежними регрессиями компонентов.

## Остаток

Найден отдельный незавершённый артефакт ТЗ: Windows manifest. У окончательного PE каталог ресурсов имеет RVA/size 0/0, manifest/rc/build.rs в исходниках нет. Runtime-запрос PMv2 остаётся существующим кодом, но не заменяет поставляемый manifest. Следующий шаг — добавить и проверить этот клиентский ресурс, не начиная упаковку.

Этот срез относится к исходникам 3805dc1; последующая [реализация Windows manifest](TEST_RESULTS_WINDOWS_MANIFEST.md) устраняет найденное отсутствие. Исходная таблица не переименовывается в результат более новой сборки.

EN-10 подтверждён в пределах просмотренного производственного пути и статического состава этого EXE; будущее расширение рендера требует повторной сверки. Статический анализ не подтверждает фактическую загрузку всех DLL, поведение/видимый ввод обеих ОС, S_OK полного Worker, DPI/hotplug, ресурсы или восемь часов. Эти проверки **Not run**. Рабочий клиент не заменялся и не включался, вода/сайт/поставка отложены.

## Повторение

~~~powershell
./tools/test-build-paths.ps1
./tools/build-client.ps1
./tools/check-client-artifact.ps1
./tools/check.ps1
~~~
