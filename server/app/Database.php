<?php
declare(strict_types=1);

namespace Snowflakes;

use PDO;
use RuntimeException;
use Throwable;

final class Database
{
    public static function open(string $path, bool $create = false): PDO
    {
        if (!$create && !is_file($path)) {
            throw new RuntimeException('Database is not installed');
        }
        $db = new PDO('sqlite:' . $path, null, null, [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
            PDO::ATTR_DEFAULT_FETCH_MODE => PDO::FETCH_ASSOC,
            PDO::ATTR_EMULATE_PREPARES => false,
        ]);
        $db->exec('PRAGMA foreign_keys = ON');
        $db->exec('PRAGMA busy_timeout = 5000');
        return $db;
    }

    /** A failed migration or callback rolls back all of its writes. */
    public static function transaction(PDO $db, callable $operation): mixed
    {
        $db->exec('BEGIN IMMEDIATE');
        try {
            $result = $operation($db);
            $db->exec('COMMIT');
            return $result;
        } catch (Throwable $error) {
            $db->exec('ROLLBACK');
            throw $error;
        }
    }

    public static function migrate(PDO $db): void
    {
        $version = (int) $db->query('PRAGMA user_version')->fetchColumn();
        if ($version === 1) {
            return;
        }
        if ($version !== 0) {
            throw new RuntimeException('Unsupported database schema');
        }
        $sql = file_get_contents(dirname(__DIR__) . '/migrations/001.sql');
        if ($sql === false) {
            throw new RuntimeException('Migration is missing');
        }
        self::transaction($db, static function (PDO $db) use ($sql): void {
            // Recheck after acquiring the writer lock; two installers can race.
            $version = (int) $db->query('PRAGMA user_version')->fetchColumn();
            if ($version === 0) {
                $db->exec($sql);
                $db->exec('PRAGMA user_version = 1');
            } elseif ($version !== 1) {
                throw new RuntimeException('Unsupported database schema');
            }
        });
    }

    public static function uuid(): string
    {
        $bytes = random_bytes(16);
        $bytes[6] = chr((ord($bytes[6]) & 0x0f) | 0x40);
        $bytes[8] = chr((ord($bytes[8]) & 0x3f) | 0x80);
        $hex = bin2hex($bytes);
        return substr($hex, 0, 8) . '-' . substr($hex, 8, 4) . '-'
            . substr($hex, 12, 4) . '-' . substr($hex, 16, 4) . '-' . substr($hex, 20);
    }
}
