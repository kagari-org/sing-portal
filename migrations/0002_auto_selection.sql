ALTER TABLE selections ADD COLUMN mode TEXT NOT NULL DEFAULT 'manual';

CREATE TABLE measurements (
    node_name   TEXT NOT NULL,
    tag         TEXT NOT NULL,
    latency_ms  REAL,
    down_mbps   REAL,
    score       REAL NOT NULL,
    measured_at INTEGER NOT NULL,
    PRIMARY KEY (node_name, tag)
);

CREATE INDEX idx_measurements_node ON measurements (node_name);
