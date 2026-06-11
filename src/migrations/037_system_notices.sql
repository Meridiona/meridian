-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Centralised fault bus. Daemon raises named notices when something breaks and
-- clears them on recovery. The UI polls this table via SSE and surfaces notices
-- as banners on every page — no user has to check terminal logs.
CREATE TABLE system_notices (
    notice_id  TEXT PRIMARY KEY,
    severity   TEXT NOT NULL CHECK (severity IN ('error', 'warning')),
    title      TEXT NOT NULL,
    detail     TEXT NOT NULL,
    remedy     TEXT,
    raised_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
