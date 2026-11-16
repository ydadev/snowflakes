# ADR 0097 Встроенный Windows manifest

Статус: принято для реализации; приёмка обеих ОС открыта. Дата: 16 ноября 2026 года.

## Основание

ТЗ требует Windows manifest и OS-04 PMv2. [Предыдущий аудит](../TEST_RESULTS_RELEASE_AUDIT.md) подтвердил Resource Directory 0/0 и отсутствие исходного ресурса. Runtime-настройка DPI не предоставляла этот артефакт. [Microsoft рекомендует встроенный manifest](https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests) и [объявление DPI до запуска UI](https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process).

## Решение

[snowflakes.manifest](../../crates/snowflakes-client/snowflakes.manifest) задаёт amd64/Win32 identity Snowflakes.Client, версию 0.1.0.0, Windows 10/11 supportedOS GUID, PerMonitorV2 с legacy true/pm, requestedExecutionLevel=asInvoker и uiAccess=false. Manifest не запрашивает повышение прав и не объявляет пройденную версию тестирования. Наличие supportedOS не доказывает фактическую совместимость обеих ОС. Дополнительные политики тем/путей и новые UI-функции не вводятся.

Новый std-only [build.rs](../../crates/snowflakes-client/build.rs) использует именованные Cargo linker-аргументы только для snowflakes на Windows/MSVC/x64: /MANIFEST:EMBED,ID=1 и /MANIFESTINPUT. Тот же бинарный target включает тестовый harness. Исходник перезапускает build script при изменении; identity-версия сверяется с тремя компонентами версии пакета. Cargo.lock и runtime-зависимости не меняются. Проверенные [Cargo-аргументы](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-link-arg-bin) и [MSVC-встраивание](https://learn.microsoft.com/en-us/cpp/build/reference/manifestinput-specify-manifest-input?view=msvc-170) не требуют нового resource-crate.

У [SetProcessDpiAwarenessContext](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setprocessdpiawarenesscontext) повтор после установки режима может вернуть ERROR_ACCESS_DENIED. Обе команды прототипа сохраняют свой ранний API-вызов, но принимают только этот код при дополнительно подтверждённом текущем PMv2. Иные ошибки и отказ при другом контексте продолжают возвращаться. Helper вызывается на начальном потоке до HWND; новая настройка потока или ослабление readiness не вводятся. Обычный runtime сохраняет прежний best-effort API-вызов, а manifest задаёт исходный режим до main.

## Проверка ресурса

[ClientManifest.psm1](../../tools/ClientManifest.psm1) находит mt.exe в уже требуемом Windows SDK, извлекает RT_MANIFEST/#1 из фактического файла и проверяет схему. Это чтение ресурса, без выполнения клиента. .NET XML-reader запрещает DTD/внешний resolver; namespace-aware проверка требует единственные identity/права/совместимость/DPI и отвергает повышение/ложный maxversiontested. Временный XML удаляется, диагностический вывод SDK не переносится в Git.

Прежний guard EXE теперь обязательно проверяет этот ресурс вместе с путями и PE. Bare-header fixture больше не считается готовым клиентом. Ожидаемая ошибка mt в отрицательном тесте не должна отравлять LASTEXITCODE успешного тестового скрипта; успех выставляется только после всех assertions. [Результаты](../TEST_RESULTS_WINDOWS_MANIFEST.md).

## Границы

Две управляемые проверки сохраняют корректную обработку DPI-результатов. Третья только читает настоящий контекст потока бинарного тестового harness и подтверждает PMv2 после загрузки. Ни она, ни mt не проверяют видимый клиент, Standard User/UAC-матрицу, настоящий перенос между DPI, полный Worker или обе ОС. asInvoker наследует имеющийся токен; отсутствие административного токена не заявляется. Рабочий эффект остаётся выключенным, упаковка P6/вода/сайт отложены.
