# ADR 0081 Проверка Worker на заданном startup desktop

Статус: принято для испытательного стенда P1-03/04 и P2-04. Дата: 15 ноября 2026 года. [Результаты и ограничения](../TEST_RESULTS_PRIVATE_WORKER.md).

## Основание

SetThreadDesktop у тестового родителя не задаёт desktop для всех будущих потоков процесса. Worker создаёт новый поток, затем собственные очередь, hooks, Observer и графические окна. Для безопасной проверки этого пути нужен отдельный процесс с явно указанным startup desktop, а не предположение о наследовании desktop вызывающего теста.

## Решение

Явно запускаемый hardware probe создаёт desktop с уникальным синтетическим именем и передаёт его в STARTUPINFOW.lpDesktop при CreateProcessW того же библиотечного тестового EXE. Родитель не присоединяется к нему, не наследует handles, не вызывает SwitchDesktop и использует CREATE_NO_WINDOW/STARTF_FORCEOFFFEEDBACK. Его handle не запрашивает DESKTOP_SWITCHDESKTOP. Имя и UOI_IO=false проверяются до запуска и после завершения ребёнка.

Только в cfg(test) Worker проверяет startup desktop и desktop собственного потока до создания очереди, hooks и окон. Дочерний сценарий работает с временным UserStore и отдельными View/Report/Conditions. Он не запускает трей, IPC, сетевой опрос или рабочий экземпляр. Данные передаются частным Unicode environment block без изменения окружения родителя. Ребёнок ограничен 30 секундами; при отказе guard завершает только принадлежащий ему process handle и закрывает оба handle.

Настоящий Observer и причины системной паузы не подменяются. Если состояние сеанса разрешает работу, проверяются Present-счётчики каждого слоя, частицы, ручная пауза, неподвижность счётчиков/частиц, очистка, возобновление с прежними HWND и уничтожение слоёв. Если сеанс запрещает работу, проверяются готовность Observer, отсутствие слоёв/частиц/Present и удержание запрета; полный активный цикл отмечается Not run. Успешное завершение диагностической пробы не означает Passed для всех её фаз.

После остановки Worker отдельно испытывается нижний графический слой на том же невходном desktop: настоящий Layer/Gpu, PMv2, DPI от HWND, буфер 256×256, синтетическая снежинка после появления, чтение alpha и четыре вызова Layer::draw. Это отдельная проверка рендера, а не обход состояния Worker. Исходные HRESULT Present записываются через test-only thread-local наблюдатель; счётчик увеличивается только для S_OK. Положительный статус DXGI не объявляется успешной отправкой. Ошибка после создания ресурсов проваливает пробу; недоступность начального создания фиксируется отдельно как Not run.

## Границы

Обе точки входа ignored и не запускаются обычным CI. Поля worker_cycle/gpu_layer и исходные коды обязательны для чтения результата; probe_completed описывает завершение диагностики. Не делать вывод о видимом кадре, реальном вводе, FPS DWM, mixed DPI, device loss, ресурсах всего клиента или Windows 10/11 по этой пробе. Повторять её при новом сценарии или изменении стенда, а не для накопления одинаковых OCCLUDED.

Основания Windows API: [подключение потока к desktop](https://learn.microsoft.com/en-us/windows/win32/winstation/thread-connection-to-a-desktop), [STARTUPINFOW.lpDesktop](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-startupinfow), [CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw), [SetThreadDesktop](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setthreaddesktop) и [статусы DXGI](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-status).
