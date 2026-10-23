PRAGMA foreign_keys = ON;

CREATE TABLE subscriptions (
    id                    TEXT PRIMARY KEY,
    name                  TEXT NOT NULL,
    slug                  TEXT NOT NULL UNIQUE,
    source                TEXT NOT NULL CHECK (source IN ('url', 'manual')),
    url                   TEXT,
    user_agent            TEXT NOT NULL DEFAULT '',
    format                TEXT NOT NULL,
    content               TEXT NOT NULL,
    outbounds             TEXT NOT NULL,
    enabled               INTEGER NOT NULL DEFAULT 1,
    auto_refresh          INTEGER NOT NULL DEFAULT 1,
    refresh_interval_secs INTEGER NOT NULL DEFAULT 21600,
    next_refresh_at       INTEGER,
    last_fetched_at       INTEGER,
    last_error            TEXT NOT NULL DEFAULT '',
    validated             INTEGER NOT NULL DEFAULT 0,
    failures              INTEGER NOT NULL DEFAULT 0,
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL
);

CREATE INDEX idx_subscriptions_next_refresh ON subscriptions (auto_refresh, next_refresh_at);

CREATE TABLE selections (
    node_name  TEXT PRIMARY KEY,
    outbound   TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE node_reports (
    node_name   TEXT PRIMARY KEY,
    revision    TEXT NOT NULL DEFAULT '',
    selection   TEXT NOT NULL DEFAULT '',
    health      TEXT NOT NULL DEFAULT '',
    error       TEXT NOT NULL DEFAULT '',
    reported_at INTEGER NOT NULL
);

CREATE TABLE audit_log (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    at     INTEGER NOT NULL,
    actor  TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT ''
);

CREATE INDEX idx_audit_log_at ON audit_log (at DESC);
