CREATE TABLE app_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    generation_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision BETWEEN 1 AND 9007199254740991),
    global_enabled INTEGER NOT NULL CHECK (global_enabled IN (0, 1))
);

CREATE TABLE profiles (
    id TEXT PRIMARY KEY,
    settings_json TEXT NOT NULL
);

CREATE TABLE profile_history (
    revision INTEGER PRIMARY KEY,
    settings_json TEXT NOT NULL,
    global_enabled INTEGER NOT NULL CHECK (global_enabled IN (0, 1))
);
