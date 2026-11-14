# Проверка восстановления трея и статуса графики

Дата: 14 ноября 2026 года. Область: P2-01/04, OS-09/10. Решение: [ADR 0046](adr/0046-tray-and-graphics-recovery.md).

## Изменение

Восстановление значка сохраняет частичный успех Shell и повторяет неудачный этап с задержкой до 30 секунд. Подсказка показывает состояние и числовой HRESULT графики. Между попытками причина больше не заменяется нормальным статусом. Существующий предел трёх ошибок, ручная пауза и снег сохраняются.

## Воспроизведение и результаты

Среда: Windows 11 x64, Rust 1.99.1/MSVC. Полный tools/check.ps1 из корня проекта — Passed: 313 тестов клиента (88 библиотечных, 19 control, 6 IPC, 9 rain, 11 scene, 20 session, 122 snow, 22 storage, 11 UI, 5 example), 3 protocol, 35 утверждений ресурсов и 31 кадров. Два прежних нативных теста графики/захвата остаются ignored. Форматирование, clippy с запретом предупреждений, release, запуск модели без графики, 60 общих случаев Ajv/PHP, 5 транспортных случаев, SQLite и HTTP прошли.

Проверки IPC/WTS требуют обычного контекста Windows. Пробный запуск внутри ограниченной песочницы получил Access denied в трёх прежних проверках IPC/WTS; десять новых сценариев прошли. Полный прогон вне песочницы прошёл без изменения тестов.

Десять новых тестов входят в обычный cargo test, без ignored и без включения рабочего эффекта:

| Сценарий | Что проверяется | Результат |
| --- | --- | --- |
| explorer_restore_waits_after_failures_and_delivers_the_latest_reason | Отказ Add не теряет восстановление; до дедлайна нет вызовов; затем доставляется актуальная причина | Passed |
| version_failure_keeps_the_added_icon_and_retries_only_version_then_dirty_text | Успешный Add не повторяется после отказа SetVersion; новая подсказка применяется отдельно | Passed |
| persistent_shell_failure_is_rate_limited_but_late_recovery_is_possible | Попытки в 0/1/3/7/15/31/61/91/121 секунд и позднее восстановление | Passed |
| failed_text_update_keeps_version_and_can_be_superseded_or_recreated | Отказ Modify не сбрасывает версию; новая причина замещает старую, повторный TaskbarCreated начинает Add | Passed |
| graphics_retry_retains_reason_between_attempts_and_stops_after_three_failures | Границы 2/4 секунд и запрет попытки после третьей ошибки даже через сутки; отсутствие кадра не очищает код | Passed |
| accepted_submission_clears_the_reason_without_resetting_the_failure_budget | Принятый кадр убирает причину, сохраняя общий предел повторных ошибок | Passed |
| graphics_reason_reaches_status_and_tooltip_without_changing_profile_or_manual_pause | Report → Status → подсказка, HRESULT/JSON, прежний JSON без поля, ручная пауза и clear_epoch | Passed |
| notification_payload_shows_standard_tooltip_without_creating_balloon_or_native_icon | Настоящий NOTIFYICONDATAW: ID/размер, NIF_SHOWTIP, длина UTF-16, версия и отсутствие NIF_INFO; Shell API не вызывается | Passed |
| hidden_tray_messages_preserve_restore_request_and_decode_legacy_and_version4_selection | Прямые сообщения своему скрытому HWND: TaskbarCreated, сохранение запроса, выбор мышью/клавиатурой до/после подтверждения версии | Passed |
| graphics_reason_survives_paused_frames_without_creating_layers_or_losing_snow | Пятнадцать приостановленных кадров при трёх синтетических ошибках: масса/положение/возраст прежние, слои не создаются; явное выключение сбрасывает ошибку | Passed |

GitHub CI: ожидает публикации исходников.

## Границы результата

| Проверка | Результат | Причина |
| --- | --- | --- |
| Настоящий перезапуск Explorer и возврат значка/меню | Not run | Рабочая оболочка пользователя не перезапускалась |
| Shell_NotifyIcon при реальном временном отказе Explorer | Not run | Результаты API подставлены; структура и маршрутизация проверены отдельно |
| Настоящая потеря D3D11-устройства и последующий S_OK | Not run | Устройство не сбрасывалось; проверена политика, не аппаратное восстановление |
| Внешний вид подсказки и меню на Windows 10/11 | Not run | CUA остановлена, видимые проверки не возобновлялись; Windows 10 недоступна |
| Нижняя опора при настоящем перезапуске панели задач | Not run | Существуют периодическая сверка рабочей области и модельные тесты панели; нативного перехода нет |

OS-09/10 и P2 остаются In progress. Подготовленная сборка не заменяет работающий клиент, снег вручную приостановлен. Дождь, лужи, сайт и поставка отложены.
