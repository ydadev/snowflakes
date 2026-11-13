<?php
declare(strict_types=1);

use Snowflakes\Configuration;
use Snowflakes\Database;
use Snowflakes\ProfileRepository;

require_once dirname(__DIR__) . '/app/bootstrap.php';

function check(bool $passed, string $name): void
{
    if (!$passed) {
        throw new RuntimeException("FAILED: $name");
    }
}

function fails(callable $operation, string $name): void
{
    try {
        $operation();
    } catch (Throwable) {
        return;
    }
    throw new RuntimeException("FAILED: $name should fail");
}

function change(object $document, string $path, mixed $value, bool $remove = false): void
{
    $keys = explode('.', $path);
    $last = array_pop($keys);
    $parent = $document;
    foreach ($keys as $key) {
        $parent = $parent->$key;
    }
    if ($remove) {
        unset($parent->$last);
    } else {
        $parent->$last = $value;
    }
}

$root = dirname(__DIR__, 2);
$base = file_get_contents($root . '/protocol/fixtures/configuration.json');
$cases = json_decode(file_get_contents($root . '/protocol/fixtures/configuration-cases.json'), false, 32, JSON_THROW_ON_ERROR);
foreach ($cases as $case) {
    $document = json_decode($base, false, 32, JSON_THROW_ON_ERROR);
    foreach ($case->set ?? [] as $path => $value) {
        change($document, $path, $value);
    }
    foreach ($case->remove ?? [] as $path) {
        change($document, $path, null, true);
    }
    try {
        Configuration::decode(json_encode($document, JSON_THROW_ON_ERROR));
        $valid = true;
    } catch (InvalidArgumentException) {
        $valid = false;
    }
    check($valid === $case->valid, 'contract: ' . $case->name);
}
foreach ([
    preg_replace('/"schema_version": 1/', '"schema_version": 1, "schema_version": 1', $base, 1),
    preg_replace('/"intensity_percent": 35/', '"intensity_percent": 35, "intensity_\\u0070ercent": 35', $base, 1),
    preg_replace('/"revision": 1/', '"revision": 1.0', $base, 1),
    $base . ' {}',
    str_repeat(' ', Configuration::MAX_BYTES + 1),
] as $ambiguous) {
    fails(static fn () => Configuration::decode($ambiguous), 'invalid transport');
}
echo 'Contract: ' . count($cases) . " shared cases + 5 transport cases passed\n";

$settings = Configuration::decode($base)->settings;
$directory = sys_get_temp_dir() . '/snowflakes-test-' . bin2hex(random_bytes(8));
mkdir($directory, 0700);
$path = $directory . '/snowflakes.sqlite';
try {
    fails(static fn () => Database::open($path), 'opening missing database does not create it');
    check(!is_file($path), 'no empty database after readiness check');
    $db = Database::open($path, true);
    Database::migrate($db);
    Database::migrate($db);
    $repository = new ProfileRepository($db);
    $repository->initialize($settings);
    $original = $repository->current();
    check($original->revision === 1 && $original->settings == $settings, 'initial complete profile');
    fails(static fn () => $repository->initialize($settings), 'repeat initialization');
    check($repository->current() == $original, 'repeat initialization preserves profile');

    $updated = unserialize(serialize($settings), ['allowed_classes' => ['stdClass']]);
    $updated->rain->intensity_percent = 100;
    $updated->rain->puddle_size_percent = 0;
    check($repository->publish($updated, false, 1) === 2, 'publish revision');
    $published = $repository->current();
    check(!$published->global_enabled && $published->settings == $updated, 'atomic profile and master switch');
    fails(static fn () => $repository->publish($settings, true, 1), 'stale form');
    check($repository->current() == $published, 'stale form preserves latest edit');

    // Fail after the profile UPDATE to prove the entire operation rolls back.
    $db->prepare('INSERT INTO profile_history VALUES (3, ?, 1)')->execute([json_encode($settings)]);
    fails(static fn () => $repository->publish($settings, true, 2), 'forced SQL conflict');
    check($repository->current() == $published, 'rollback restores settings and revision');
    $db->exec('DELETE FROM profile_history WHERE revision = 3');

    $bad = clone $updated;
    $bad->rain = clone $bad->rain;
    $bad->rain->puddle_size_percent = 101;
    fails(static fn () => $repository->publish($bad, true, 2), 'invalid profile before SQL');
    $reopened = new ProfileRepository(Database::open($path));
    check($reopened->current() == $published, 'profile survives connection restart');
    unset($reopened, $repository, $db);
    echo "SQLite: migration, initialization, publication, stale edit, rollback and durability passed\n";
} finally {
    unset($reopened, $repository, $db);
    foreach (glob($directory . '/*') as $file) {
        unlink($file);
    }
    rmdir($directory);
}
