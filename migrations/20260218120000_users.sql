CREATE TABLE users (
    id BLOB PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    is_admin INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    password_hash TEXT NOT NULL DEFAULT ''
);

CREATE INDEX users_created_at_idx ON users(created_at);

CREATE TABLE user_sessions (
    id BLOB PRIMARY KEY NOT NULL,
    user_id BLOB NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX user_sessions_user_idx ON user_sessions(user_id);
CREATE INDEX user_sessions_created_at_idx ON user_sessions(created_at);
