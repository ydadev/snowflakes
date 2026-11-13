<?php
declare(strict_types=1);
header('Content-Type: text/html; charset=utf-8');
header('Cache-Control: no-store');
if (!in_array(parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH), ['/', '/index.php'], true)) {
    http_response_code(404);
    echo 'Not found';
    exit;
}
?>
<!doctype html>
<html lang="ru">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Snowflakes — технический прототип</title>
<h1>Snowflakes</h1>
<p>Технический прототип PHP API. Панель управления пока в разработке.</p>
<p><a href="api.php?action=health">Проверить доступность</a></p>
</html>
