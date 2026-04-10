PRAGMA foreign_keys = OFF;

CREATE TABLE IF NOT EXISTS inbounds_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    tag TEXT NOT NULL UNIQUE,
    allow_lan_access INTEGER NOT NULL DEFAULT 0,
    tls_id INTEGER DEFAULT NULL,
    addrs TEXT NOT NULL DEFAULT '[]',
    out_json TEXT NOT NULL DEFAULT '{}',
    options TEXT NOT NULL DEFAULT '{}',
    FOREIGN KEY (tls_id) REFERENCES tls (id) ON DELETE RESTRICT
);

INSERT INTO inbounds_new (id, kind, tag, allow_lan_access, tls_id, addrs, out_json, options)
SELECT
    id,
    kind,
    tag,
    allow_lan_access,
    NULLIF(tls_id, 0),
    addrs,
    out_json,
    options
FROM inbounds;

DROP TABLE inbounds;
ALTER TABLE inbounds_new RENAME TO inbounds;

CREATE TABLE IF NOT EXISTS services_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    tag TEXT NOT NULL UNIQUE,
    tls_id INTEGER DEFAULT NULL,
    options TEXT NOT NULL DEFAULT '{}',
    FOREIGN KEY (tls_id) REFERENCES tls (id) ON DELETE RESTRICT
);

INSERT INTO services_new (id, kind, tag, tls_id, options)
SELECT
    id,
    kind,
    tag,
    NULLIF(tls_id, 0),
    options
FROM services;

DROP TABLE services;
ALTER TABLE services_new RENAME TO services;

CREATE INDEX IF NOT EXISTS idx_stats_resource_tag_time ON stats(resource, tag, date_time);
CREATE INDEX IF NOT EXISTS idx_changes_datetime ON changes(date_time);

PRAGMA foreign_keys = ON;
