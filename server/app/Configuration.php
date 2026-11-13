<?php
declare(strict_types=1);

namespace Snowflakes;

use InvalidArgumentException;
use JsonException;
use stdClass;

/** Independent implementation of protocol/schemas/configuration-v2.schema.json. */
final class Configuration
{
    public const SCHEMA_VERSION = 2;
    public const MAX_BYTES = 65_536;
    public const MAX_REVISION = 9_007_199_254_740_991;

    public static function decode(string $json): stdClass
    {
        self::check(strlen($json) <= self::MAX_BYTES, 'document size');
        try {
            $value = json_decode($json, false, 32, JSON_THROW_ON_ERROR);
        } catch (JsonException) {
            throw new InvalidArgumentException('Invalid JSON profile');
        }
        self::rejectDuplicateKeys($json);
        self::validate($value);
        return $value;
    }

    public static function validate(mixed $value): void
    {
        self::fields($value, [
            'schema_version', 'generation_id', 'revision',
            'profile_id', 'global_enabled', 'settings',
        ]);
        self::check($value->schema_version === self::SCHEMA_VERSION, 'schema_version');
        self::check(
            is_string($value->generation_id)
                && preg_match('/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/D', $value->generation_id) === 1,
            'generation_id'
        );
        self::integer($value->revision, 1, self::MAX_REVISION, 'revision');
        self::check(
            is_string($value->profile_id)
                && preg_match('/^[a-z][a-z0-9-]{0,63}$/D', $value->profile_id) === 1,
            'profile_id'
        );
        self::boolean($value->global_enabled, 'global_enabled');
        self::validateSettings($value->settings);
    }

    public static function validateSettings(mixed $settings): void
    {
        self::fields($settings, [
            'effects_enabled', 'weather', 'snow', 'rain',
            'interaction', 'performance', 'display',
        ]);
        self::boolean($settings->effects_enabled, 'effects_enabled');
        self::fields($settings->weather, ['mode', 'opacity_percent']);
        self::check(in_array($settings->weather->mode, ['snow', 'rain'], true), 'weather.mode');
        self::integer($settings->weather->opacity_percent, 10, 100, 'weather.opacity_percent');

        foreach (['snow' => ['drift_size_percent', 'melting'], 'rain' => ['puddle_size_percent', 'evaporation']] as $name => [$size, $decay]) {
            $section = $settings->$name;
            $fields = ['intensity_percent', $size, 'wind_percent', 'on_windows', 'on_ground', $decay];
            if ($name === 'snow') {
                $fields[] = 'flake_size_percent';
                $fields[] = 'flake_visibility_percent';
            }
            self::fields($section, $fields);
            if ($name === 'snow') {
                self::integer($section->flake_size_percent, 25, 200, 'snow.flake_size_percent');
                self::integer($section->flake_visibility_percent, 0, 200, 'snow.flake_visibility_percent');
            }
            self::integer($section->intensity_percent, 0, 100, "$name.intensity_percent");
            self::integer($section->$size, 0, 100, "$name.$size");
            self::integer($section->wind_percent, -100, 100, "$name.wind_percent");
            foreach (['on_windows', 'on_ground', $decay] as $field) {
                self::boolean($section->$field, "$name.$field");
            }
        }

        self::fields($settings->interaction, ['cursor_enabled']);
        self::boolean($settings->interaction->cursor_enabled, 'interaction.cursor_enabled');
        self::fields($settings->performance, ['target_fps', 'battery_saver', 'pause_fullscreen', 'pause_remote_session']);
        self::check(in_array($settings->performance->target_fps, [30, 60], true), 'performance.target_fps');
        foreach (['battery_saver', 'pause_fullscreen', 'pause_remote_session'] as $field) {
            self::boolean($settings->performance->$field, "performance.$field");
        }

        self::fields($settings->display, ['selection', 'monitor_ids']);
        $display = $settings->display;
        self::check(in_array($display->selection, ['all', 'primary', 'selected'], true), 'display.selection');
        self::check(is_array($display->monitor_ids) && count($display->monitor_ids) <= 16, 'display.monitor_ids');
        self::check(($display->selection === 'selected') !== ($display->monitor_ids === []), 'display selection');
        $seen = [];
        foreach ($display->monitor_ids as $id) {
            self::check(
                is_string($id) && $id !== ''
                    && preg_match_all('/./us', $id) <= 256
                    && preg_match('/[\x00-\x1f]/', $id) === 0
                    && !in_array($id, $seen, true),
                'display.monitor_ids entry'
            );
            $seen[] = $id;
        }
    }

    private static function fields(mixed $value, array $expected): void
    {
        self::check($value instanceof stdClass, 'object type');
        $actual = array_keys(get_object_vars($value));
        sort($actual);
        sort($expected);
        self::check($actual === $expected, 'object fields');
    }

    private static function integer(mixed $value, int $min, int $max, string $field): void
    {
        self::check(is_int($value) && $value >= $min && $value <= $max, $field);
    }

    private static function boolean(mixed $value, string $field): void
    {
        self::check(is_bool($value), $field);
    }

    private static function check(bool $valid, string $field): void
    {
        if (!$valid) {
            throw new InvalidArgumentException("Invalid $field");
        }
    }

    /** json_decode otherwise silently keeps the last value of duplicate keys. */
    private static function rejectDuplicateKeys(string $json): void
    {
        $pattern = <<<'REGEX'
/"(?:[^"\\]|\\.)*"|[{}\[\]:,]/s
REGEX;
        preg_match_all($pattern, $json, $matches);
        $tokens = $matches[0];
        $stack = [];
        foreach ($tokens as $index => $token) {
            if ($token === '{' || $token === '[') {
                $stack[] = $token === '{' ? [] : null;
            } elseif ($token === '}' || $token === ']') {
                array_pop($stack);
            } elseif ($token[0] === '"' && ($tokens[$index + 1] ?? null) === ':') {
                $key = json_decode($token, true, 2, JSON_THROW_ON_ERROR);
                $top = count($stack) - 1;
                self::check(!array_key_exists($key, $stack[$top]), 'duplicate object key');
                $stack[$top][$key] = true;
            }
        }
    }
}
