<?php
declare(strict_types=1);

use Snowflakes\Configuration;
use Snowflakes\Database;
use Snowflakes\ProfileRepository;

require_once dirname(__DIR__) . '/app/bootstrap.php';

function expect(bool $condition, string $message): void
{
    if (!$condition) {
        throw new RuntimeException("FAILED: $message");
    }
}

function request(string $url, string $method = 'GET'): array
{
    $context = stream_context_create(['http' => [
        'method' => $method, 'ignore_errors' => true, 'timeout' => 2,
    ]]);
    $body = @file_get_contents($url, false, $context);
    $headers = $http_response_header ?? [];
    if ($body === false || $headers === []) {
        throw new RuntimeException('No HTTP response');
    }
    preg_match('/^HTTP\/\S+ (\d+)/', $headers[0], $status);
    return [(int) $status[1], $body, implode("\n", $headers)];
}

// An ephemeral loopback port keeps this test independent of installed services.
$socket = stream_socket_server('tcp://127.0.0.1:0', $errno, $error);
if ($socket === false) {
    throw new RuntimeException('Cannot reserve test port');
}
$address = stream_socket_get_name($socket, false);
fclose($socket);
$directory = sys_get_temp_dir() . '/snowflakes-http-' . bin2hex(random_bytes(8));
mkdir($directory, 0700);
$log = tmpfile();
$environment = getenv();
$environment['SNOWFLAKES_STORAGE_DIR'] = $directory;
$process = proc_open(
    [PHP_BINARY, '-S', $address, '-t', dirname(__DIR__) . '/public'],
    [0 => ['pipe', 'r'], 1 => $log, 2 => $log],
    $pipes,
    null,
    $environment,
    ['bypass_shell' => true, 'create_new_console' => false]
);
if (!is_resource($process)) {
    throw new RuntimeException('Cannot start PHP test server');
}
fclose($pipes[0]);
$base = 'http://' . $address;
try {
    $available = false;
    for ($attempt = 0; $attempt < 50; $attempt++) {
        try {
            [$status, $body, $headers] = request($base . '/api.php?action=health');
            $available = $status === 200;
            if ($available) {
                break;
            }
        } catch (RuntimeException) {
            // The child has not started listening yet.
        }
        usleep(100_000);
    }
    if (!$available) {
        rewind($log);
        throw new RuntimeException('PHP test server unavailable: ' . stream_get_contents($log, 2048));
    }
    expect(json_decode($body, true)['status'] === 'ok', 'health JSON');
    expect(stripos($headers, 'Cache-Control: no-store') !== false, 'API is not cached');
    [$status, $body] = request($base . '/api.php?action=ready');
    expect($status === 503 && json_decode($body, true)['status'] === 'not_ready', 'missing database readiness');
    expect(!is_file($directory . '/snowflakes.sqlite'), 'readiness creates no database');

    $db = Database::open($directory . '/snowflakes.sqlite', true);
    Database::migrate($db);
    $fixture = Configuration::decode(file_get_contents(dirname(__DIR__, 2) . '/protocol/fixtures/configuration.json'));
    (new ProfileRepository($db))->initialize($fixture->settings);
    [$status, $body] = request($base . '/api.php?action=ready');
    expect($status === 200 && json_decode($body, true)['status'] === 'ready', 'installed readiness');
    foreach (['/app/bootstrap.php', '/config/', '/storage/snowflakes.sqlite', '/migrations/001.sql', '/api.php?action=client.sync', '/api.php?action[]=health'] as $path) {
        expect(request($base . $path)[0] === 404, 'unavailable path: ' . $path);
    }
    expect(request($base . '/api.php?action=health', 'POST')[0] === 405, 'wrong method');
    [$status, $body] = request($base . '/');
    expect($status === 200 && str_contains($body, 'href="api.php?action=health"'), 'relative links preserve IP and port');
    echo "HTTP: health, readiness, methods, no-store and private paths passed\n";
} finally {
    proc_terminate($process);
    proc_close($process);
    fclose($log);
    unset($db);
    foreach (glob($directory . '/*') as $file) {
        unlink($file);
    }
    rmdir($directory);
}
