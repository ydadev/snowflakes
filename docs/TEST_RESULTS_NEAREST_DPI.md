# Проверки DPI у границы экрана

Дата: 13 ноября 2026 года. Windows 11, Rust 1.99.1/MSVC x64. P2-02/04, P3-03/04; OS-04, частичное покрытие GE-09 и SN-05/06. Активен снег, вода Deferred.

Исправлен выбор масштаба для точек вне дисплеев: scale_at использует тот же ближайший экран, что пауза и допуск частиц. [ADR 0032](adr/0032-nearest-display-scale.md). Изменение — в общей геометрии; новые функции воды не разрабатывались.

## Воспроизведённые регрессии

| Проверка | До исправления | После |
| --- | --- | --- |
| offscreen_scale_follows_the_nearest_display_in_a_staggered_desktop | Failed: над экраном 200% возвращалось 1.0 | Passed: верхнее/нижнее поле, зазор рядом с более высоким экраном, отрицательное начало координат, внутренние точки 100/200%, пустая сцена |
| snow_in_the_upper_margin_advances_in_its_monitors_logical_units | Failed: на 150% шаг составлял 0,338 DIP | Passed: для 100/150/200/250% шаг укладывается в 0,49–0,53 DIP при начальной скорости 30 DIP/с, массе и силах исходной снежинки |
| moving_a_stocked_roof_to_the_monitor_top_does_not_resize_its_snow | Failed: на 150% высота менялась с 7,5 до 3,333 физических пикселей | Passed: перенос к верхней границе сохраняет высоту; закрытие сохраняет слой 5 DIP, ширину ячейки и массу пластов на четырёх масштабах |
| pending_profiles_above_a_high_dpi_monitor_keep_their_physical_span | Failed: 484 пикселя вместо 284 | Passed: два профиля по 42 DIP над экраном 200% дают правильный общий размах; центр, масса и скорость сохранены |

Все входные условия синтетические. Тест крыши меняет геометрию без накопленной задержки движения, чтобы изолировать DPI от инерционного срыва. Профиль крыши задаётся непосредственно и учитывается в балансе. Тест объединения проверяет размах профиля, а не обещает точное сохранение каждой детали округлённого контура.

Полный tools/check.ps1 Passed: 224 теста клиента, включая 100 в snow и 36 модульных, 3 теста протокола, 33 проверки ресурсов, 31 проверка кадров, fmt/clippy/release, JSON Schema и PHP/SQLite/HTTP. Один аппаратный тест кадров остаётся исключённым из обычного прогона. Существующий поток 340 закрытий с паузами/отключением экранов проходит после изменения общего выбора DPI; это конечная модельная проверка.

~~~text
cargo test -p snowflakes-client --lib pending_profiles_above_a_high_dpi_monitor_keep_their_physical_span --locked
cargo test -p snowflakes-client --test snow offscreen_scale_follows_the_nearest_display_in_a_staggered_desktop --locked
cargo test -p snowflakes-client --test snow snow_in_the_upper_margin_advances_in_its_monitors_logical_units --locked
cargo test -p snowflakes-client --test snow moving_a_stocked_roof_to_the_monitor_top_does_not_resize_its_snow --locked
~~~

## Открытая приёмка

Not run: видимый результат, настоящий смешанный DPI, плавность пересечения разных экранов, физическое отключение/возврат мониторов, Windows 10, CT-10, CPU/GPU/память и восьмичасовой прогон. Выбор ближайшего текущего дисплея не сохраняет исторический DPI отключённого экрана. Локальность/маски при переустройстве экранов остаются открытыми. Рабочий эффект и CUA не запускались. [Матрица](SNOW_REQUIREMENTS.md), [конечный поток](TEST_RESULTS_SNOW_STRESS.md).
