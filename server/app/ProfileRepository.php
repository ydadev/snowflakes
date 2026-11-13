<?php
declare(strict_types=1);

namespace Snowflakes;

use PDO;
use RuntimeException;
use stdClass;

final class ProfileRepository
{
    public function __construct(private readonly PDO $db) {}

    /** Used by the prototype tests; the browser installer is a later milestone. */
    public function initialize(stdClass $settings): void
    {
        Configuration::validateSettings($settings);
        Database::transaction($this->db, function (PDO $db) use ($settings): void {
            if ((int) $db->query('SELECT COUNT(*) FROM app_state')->fetchColumn() !== 0) {
                throw new RuntimeException('Database is already initialized');
            }
            $insert = $db->prepare('INSERT INTO app_state VALUES (1, ?, 1, 1)');
            $insert->execute([Database::uuid()]);
            $json = json_encode($settings, JSON_THROW_ON_ERROR | JSON_UNESCAPED_UNICODE);
            $db->prepare("INSERT INTO profiles VALUES ('default', ?)")->execute([$json]);
            $db->prepare('INSERT INTO profile_history VALUES (1, ?, 1)')->execute([$json]);
        });
    }

    public function current(): stdClass
    {
        $row = $this->db->query(
            "SELECT generation_id, revision, global_enabled, settings_json
             FROM app_state JOIN profiles ON profiles.id = 'default'
             WHERE singleton = 1"
        )->fetch();
        if ($row === false) {
            throw new RuntimeException('Database is not initialized');
        }
        $profile = (object) [
            'schema_version' => Configuration::SCHEMA_VERSION,
            'generation_id' => $row['generation_id'],
            'revision' => (int) $row['revision'],
            'profile_id' => 'default',
            'global_enabled' => (bool) $row['global_enabled'],
            'settings' => json_decode($row['settings_json'], false, 32, JSON_THROW_ON_ERROR),
        ];
        Configuration::validate($profile);
        return $profile;
    }

    /** Optimistic concurrency rejects stale forms without losing a newer edit. */
    public function publish(stdClass $settings, bool $enabled, int $expectedRevision): int
    {
        Configuration::validateSettings($settings);
        return Database::transaction($this->db, function (PDO $db) use ($settings, $enabled, $expectedRevision): int {
            $revision = (int) $db->query('SELECT revision FROM app_state WHERE singleton = 1')->fetchColumn();
            if ($revision !== $expectedRevision || $revision >= Configuration::MAX_REVISION) {
                throw new RuntimeException('Profile revision conflict');
            }
            $revision++;
            $json = json_encode($settings, JSON_THROW_ON_ERROR | JSON_UNESCAPED_UNICODE);
            $db->prepare("UPDATE profiles SET settings_json = ? WHERE id = 'default'")->execute([$json]);
            $db->prepare('UPDATE app_state SET revision = ?, global_enabled = ? WHERE singleton = 1')->execute([$revision, (int) $enabled]);
            $db->prepare('INSERT INTO profile_history VALUES (?, ?, ?)')->execute([$revision, $json, (int) $enabled]);
            return $revision;
        });
    }
}
