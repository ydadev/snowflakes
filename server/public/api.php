<?php
declare(strict_types=1);

use Snowflakes\Database;
use Snowflakes\ProfileRepository;

require_once dirname(__DIR__) . '/app/bootstrap.php';

header('Content-Type: application/json; charset=utf-8');
header('Cache-Control: no-store');
header('X-Content-Type-Options: nosniff');

$action = $_GET['action'] ?? '';
if (!is_string($action) || !in_array($action, ['health', 'ready'], true)) {
    http_response_code(404);
    echo '{"error":"unknown_action"}';
    exit;
}
if ($_SERVER['REQUEST_METHOD'] !== 'GET') {
    header('Allow: GET');
    http_response_code(405);
    echo '{"error":"method_not_allowed"}';
    exit;
}

if ($action === 'health') {
    echo '{"status":"ok","schema_version":1}';
    exit;
}

try {
    $storage = getenv('SNOWFLAKES_STORAGE_DIR') ?: dirname(__DIR__) . '/storage';
    $db = Database::open($storage . '/snowflakes.sqlite');
    if ((int) $db->query('PRAGMA user_version')->fetchColumn() !== 1) {
        throw new RuntimeException('Database schema is not ready');
    }
    (new ProfileRepository($db))->current();
    echo '{"status":"ready","schema_version":1}';
} catch (Throwable) {
    http_response_code(503);
    echo '{"status":"not_ready"}';
}
