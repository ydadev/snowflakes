# ADR 0075 Адресация существующего сеанса

Статус: принято для P2-03. Дата: 15 ноября 2026 года. Продолжает [локальный IPC](0006-windows-runtime-ipc.md). [Проверки](../TEST_RESULTS_SESSION_TARGETING.md).

## Основание

Имя канала уже включает SID и Session ID, но CLI выбирала только собственный сеанс. Административное обращение к работающему клиенту должно выбирать точный адрес и сохранять проверку настоящего владельца сервера. Запуск процесса в чужом интерактивном сеансе остаётся отдельной задачей P6.

## Решение

Команды существующему экземпляру принимают --session ID и необязательный --user-sid SID. Без этих флагов сохраняется собственная пара пользователя и сеанса, без нового обращения к WTS. Селектор не разрешён для запуска, ensure, from-logon, обнаружения мониторов, отдельного каталога и стартовой диагностики.

Без явного SID адаптер читает WTSUserName и WTSDomainName только локального сервера, повторно сверяет пару и разрешает полное имя domain\\user через LookupAccountNameW. Пустой владелец, смена наблюдённого входа, ошибка доступа или разрешения не заменяются текущим пользователем. WTSQueryUserToken не применяется: ему нужны LocalSystem и SeTcbPrivilege. Обращение к WTS другого пользователя требует собственных разрешений Windows.

Явный полный SID проверяется и канонизируется Windows без разрешения имени учётной записи. Он не является полномочием доступа. Обычный пользователь может адресовать свой SID; чужой SID требует включённой группы Administrators в токене процесса либо LocalSystem. CheckTokenMembership получает идентификационную копию явно открытого токена процесса; ambient-токен потока не используется. Копия не назначается потоку или процессу. Группа deny-only права не даёт, автоматического повышения нет.

Полученный Endpoint имеет отдельный тип и не может использоваться для захвата mutex или старта Server. Namespace сохраняется; полное имя канала ограничено 256 символами. Зарезервированный Session ID 4294967295 запрещён; 0 допустим только как адрес уже существующего экземпляра и не разрешает GUI в Session 0.

Перед записью запроса клиент получает реальный PID сервера, проверяет его Session ID и SID токена. Несовпадение возвращает отказ до отправки команды. Имя канала само по себе не подтверждает владельца. DACL владельца/BA/SYSTEM, PIPE_REJECT_REMOTE_CLIENTS, FILE_FLAG_FIRST_PIPE_INSTANCE, identification SQOS и ограничения overlapped I/O сохраняются. Схема запроса остаётся 1; селектор не передаётся серверу.

## Ошибки и границы

Добавлен код 7 для отказа доступа. Код 5 также означает отсутствие разрешимого владельца целевого сеанса; 2 — неверный селектор, 3 — отсутствие точного экземпляра. Ответы не содержат SID, имён учётных записей и подробного текста Windows. Разрешение доменной учётной записи синхронно и зависит от служб Windows; IPC-тайм-аут не является общим сроком всей CLI. Явный SID исключает этот поиск.

Типизированные команды и политика профиля действуют как прежде: адресация не снимает ручную паузу, не меняет профиль сама по себе и не создаёт приложение. --resume и другие явные команды сохраняют свой обычный смысл. Удалённый вызов через существующий административный транспорт выполняет CLI на целевом ПК; сетевым клиентам именованного канала доступ не открывается. Реальные разные пользователи/сеансы, SYSTEM, UAC и обе ОС требуют отдельной приёмки.

Основания: [WTSQuerySessionInformationW](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsquerysessioninformationw), [LookupAccountNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-lookupaccountnamew), [WTSQueryUserToken](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsqueryusertoken), [DuplicateToken](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-duplicatetoken), [CheckTokenMembership](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-checktokenmembership), [CreateRestrictedToken](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken), [предел имени канала](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-createnamedpipew).
