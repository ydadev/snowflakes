# Проверки исключения эффекта из захвата

Дата: 14 ноября 2026 года. Windows 11 x64, Rust 1.99.1/MSVC. P2-01/04, OS-11. [Решение](adr/0044-capture-exclusion.md).

## Реализованный результат

Местный exclude_from_capture=false сохраняется отдельно от погоды. Панель и --exclude-from-capture on/off управляют им при любом источнике. Ошибка записи оставляет старый выбор. Windows получает запрос для каждого создаваемого или существующего слоя, включая скрытые слои на паузе; результат читается обратно. Статус различает запрос, ожидание, подтверждённые слои и ошибки. [Описание команд](CLIENT_CLI.md).

## Подтверждение

| Проверка | Результат |
| --- | --- |
| capture_preference_is_optional_typed_and_separate_from_the_weather_contract | Старый TOML, bool, неверный тип/предупреждение, запись/повторное чтение и неизменный строгий Settings |
| capture_preference_persists_in_both_sources_without_changing_pause_or_weather | CLI/JSON-команда, местный и серверный источник, повторное открытие, неизменные паузы, пресет, профиль и clear_epoch |
| capture_preference_is_not_applied_when_saving_fails | Отказ хранилища сохраняет прежние конфигурацию и ручную паузу |
| capture_requests_coalesce_independently_and_keep_source_barriers | Отдельный ключ, объединение быстрых переключений и сохранение границ источника |
| failures_are_bounded_per_request_and_new_windows_apply_independently | 200 одинаковых запросов дают один вызов; изменение значения и новое окно допускают повтор |
| reports_keep_partial_failure_and_stale_requests_distinct | Два слоя с разными результатами, API/прочитанный другой флаг, старый запрос и JSON-отчёт |
| hidden_capture_control_is_local_and_distinguishes_requested_and_applied | Скрытая панель, клавиатурный TabStop, доступность при серверном источнике, реальная запись и пояснение неподтверждённого применения |
| capture_failures_during_pause_keep_material_and_do_not_trigger_graphics_recovery | Отказ API на тестовом недействительном окне не меняет массу, положение, паузу или счётчик восстановления GPU |
| capture_exclusion_reversions_break_epochs_without_changing_the_weather_profile | false → true → false образует три наблюдённые конфигурации без изменения Settings |
| capture_preference_round_trips_over_ipc_without_starting_a_renderer | Отсутствующий процесс не запускается; изолированный headless-клиент сохраняет оба значения, ручную паузу и ноль графических слоёв |

Все десять Passed. Полный tools/check.ps1 Passed: 296 тестов клиента, включая 76 модульных, 16 control, 5 IPC, 122 snow и 5 проверок примера; 3 теста протокола, 35 проверок ресурсов, 31 проверка кадров, fmt/clippy/release, baseline smoke, Ajv и PHP/SQLite/HTTP. Сборщик ресурсов отдельно Passed в Windows PowerShell 5.1, 35 проверок.

Первый общий прогон выявил конфликт ID нового элемента с существующей подписью. Он исправлен; повторный прогон всей скрытой панели и полный штатный прогон Passed. Новая функция не заявляется регрессией Failed → Passed на прежнем выпущенном поведении.

## Нативная проверка

Коммит b8cb56e прошёл [GitHub CI](https://github.com/ydadev/snowflakes/actions/runs/43550057312): Windows, PowerShell 7/5.1, release, запуск модели без графики и contract-and-php. Аудит сверил 174 файла по blob SHA и HEAD, историю/метаданные и 430 местных Markdown-ссылок. Нативный тест захвата ниже запускался отдельно на локальном стенде.

native_capture_affinity_round_trips_on_a_hidden_gpu_layer явно запущен отдельно с --ignored и Passed. Создан настоящий Layer::new: D3D11, Direct2D и DirectComposition. Для false → true → false → true → false подтверждены результат Set/GetWindowDisplayAffinity и отсутствие видимости окна. Ни одного Present или показа окна, трея либо рабочего эффекта не выполнялось.

Повтор: cargo test -p snowflakes-client --lib native_capture_affinity_round_trips_on_a_hidden_gpu_layer -- --ignored. Нужны интерактивный DWM и D3D11; этот тест исключён из обычного CI вместе с существующей аппаратной проверкой кадров. Два исключения в штатном прогоне не означают два подтверждённых нативных результата; здесь отдельно принят только флаг захвата на указанном стенде.

SetWindowDisplayAffinity ограничивает совместимые пути захвата; успешное чтение флага не доказывает скрытие во всех программах записи. [Ограничения Windows](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity).

## Открытая приёмка

Not run: изображение в конкретных программах захвата, оба режима настройки на Windows 10, видимая панель/эффект, настоящий hotplug/смена DPI и восстановление после реальной потери GPU. OS-11, P2/P3 и проект остаются In progress.

Release-сборка подготовлена отдельно. Чтение существующего клиента подтвердило ручную паузу и rendering=false; команды изменения предпочтений ему не отправлялись. Рабочий процесс не заменялся, CUA не возобновлялась; дождь, лужи, сайт и поставка Deferred.
