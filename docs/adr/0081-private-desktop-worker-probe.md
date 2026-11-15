# ADR 0081 Проверка Worker на заданном startup desktop

Статус: принято для испытательного стенда P1-03/04 и P2-04. Дата: 15 ноября 2026 года. [Результаты и ограничения](../TEST_RESULTS_PRIVATE_WORKER.md).

## Основание

SetThreadDesktop у тестового родителя не задаёт desktop для всех будущих потоков процесса. Worker создаёт новый поток, затем собственные очередь, hooks, Observer и графические окна. Для безопасной проверки этого пути нужен отдельный процесс с явно указанным startup desktop, а не предположение о наследовании desktop вызывающего теста.

## Решение

Явно запускаемый hardware probe создаёт desktop с уникальным синтетическим именем и передаёт его в STARTUPINFOW.lpDesktop при CreateProcessW того же библиотечного тестового EXE. Родитель не присоединяется к нему, не наследует handles, не вызывает SwitchDesktop и использует CREATE_NO_WINDOW/STARTF_FORCEOFFFEEDBACK. Его handle не запрашивает DESKTOP_SWITCHDESKTOP. Имя и UOI_IO=false проверяются до запуска и после завершения ребёнка.

Только в cfg(test) Worker проверяет startup desktop и desktop собственного потока до создания очереди, hooks и окон. Дочерний сценарий работает с временным UserStore и отдельными View/Report/Conditions. Он не запускает трей, IPC, сетевой опрос или рабочий экземпляр. Данные передаются частным Unicode environment block без изменения окружения родителя. Ребёнок ограничен 30 секундами; при отказе guard завершает принадлежащий ему process handle.

При добавлении вложенного NativeTestHost launcher получил собственный анонимный Job Object с JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. Ребёнок создаётся CREATE_SUSPENDED, назначается в job до ResumeThread; его принадлежность проверяется. Handle не наследуется, breakaway не разрешается. Закрытие последнего handle завершает оставшихся членов собственного дерева; чужие PID не разыскиваются. После нормального выхода accounting ожидается до 2 секунд до нуля активных процессов. Job API — Windows dev-dependency. [Основание Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects).

Настоящий Observer и причины системной паузы не подменяются. Если состояние сеанса разрешает работу, проверяются Present-счётчики каждого слоя, частицы, ручная пауза, неподвижность счётчиков/частиц, очистка, возобновление с прежними HWND и уничтожение слоёв. Если сеанс запрещает работу, проверяются готовность Observer, отсутствие слоёв/частиц/Present и удержание запрета; полный активный цикл отмечается Not run. Успешное завершение диагностической пробы не означает Passed для всех её фаз.

После остановки Worker отдельно испытывается нижний графический слой на том же невходном desktop: настоящий Layer/Gpu, PMv2, DPI от HWND, буфер 256×256, синтетическая снежинка после появления, чтение alpha и четыре вызова Layer::draw. Это отдельная проверка рендера, а не обход состояния Worker. Исходные HRESULT Present записываются через test-only thread-local наблюдатель; счётчик увеличивается только для S_OK. Положительный статус DXGI не объявляется успешной отправкой. Ошибка после создания ресурсов проваливает пробу; недоступность начального создания фиксируется отдельно как Not run.

## Продолжение: выбор дисплея

[Нативный сценарий Weather/Layer](../TEST_RESULTS_NATIVE_LAYER_SELECTION.md) использует тот же launcher с отдельной exact-точкой входа и типом отчёта. PMv2 scope общий, имя/UOI_IO и владение процессом проверяются прежними guards. Дополнительная трасса Present включается только явно на тестовом потоке, ограничена 64 кодами и выключена в обычных вызовах.

Сценарий сверяет настоящие рамки мониторов с HWND, bitmap, swap chain и World.scene; переключает All/Primary/Selected/недоступный выбор/All и проверяет удержание/освобождение ресурсов. Настоящая очередь обрабатывается, готовность сцены ожидается с пределом 3 секунды без подмены Source; в ожидании не должно быть Present. Отдельный доступный второй монитор обязателен для соответствующей ветви, иначе она Not run. Это проверка нижнего update-пути, а не отмена системного запрета полного Worker.

## Продолжение: снег окна в GPU-буфере

[Связанный Native/Weather/World/Gpu](../TEST_RESULTS_NATIVE_SNOW_PIXELS.md) использует прежний Host через узкий cfg(test) re-export. Префикс startup desktop уточнён до SnowflakesNative-GpuProbe-, поэтому сохраняются оба строгих семейства проверок имени, UOI_IO, владельца и DPI. Реальная команда native-slide меняет только принадлежащее ребёнку окно с SWP_NOACTIVATE.

Для нужных кадров test-only флаг читает bitmap после EndDraw, до Present; обычные вызовы не копируют пиксели. Данные берутся из настоящего render_snow, без дополнительной перерисовки. Пиксели старой/новой крыши, падающего материала и пола сопоставляются с массой World. Шаги времени явные; ускоренный прогон не принимается за wall-производительность или активный Worker. Масса не отбрасывается для завершения, мелкие частицы ожидаются при тех же силах с отдельным пределом шагов.

## Границы

Точки входа всех аппаратных сценариев ignored и не запускаются обычным CI. Для исходной пробы поля worker_cycle/gpu_layer и исходные коды обязательны для чтения результата; probe_completed описывает завершение диагностики. У сценария выбора экрана отдельно проверяется distinct_secondary. Не делать вывод о видимом кадре, реальном вводе, FPS DWM, mixed DPI, device loss, ресурсах всего клиента или Windows 10/11 по этим пробам. Повторять их при новом сценарии или изменении стенда, а не для накопления одинаковых OCCLUDED.

Основания Windows API: [подключение потока к desktop](https://learn.microsoft.com/en-us/windows/win32/winstation/thread-connection-to-a-desktop), [STARTUPINFOW.lpDesktop](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-startupinfow), [CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw), [SetThreadDesktop](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setthreaddesktop) и [статусы DXGI](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-status).
