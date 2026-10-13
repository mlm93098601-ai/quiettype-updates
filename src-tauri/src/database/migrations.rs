use rusqlite::Connection;

/// Run all database migrations in order.
pub fn run(conn: &Connection) -> Result<(), String> {
    // Create migrations tracking table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("Failed to create schema_version table: {}", e))?;

    let current_version: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), -1) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(-1);

    let migrations: Vec<(i32, &str)> = vec![
        // Migration 0000: Create history table (matches Typeless core schema)
        (0, "
            CREATE TABLE IF NOT EXISTS history (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),

                -- Audio
                audio BLOB,
                audio_local_path TEXT,
                duration REAL,
                mic_device TEXT,
                mic_device_info BLOB,
                audio_metadata TEXT,

                -- Text
                refined_text TEXT,
                edited_text TEXT,
                edited_text_status TEXT DEFAULT 'NOT_EXTRACTED',
                edited_text_attempts INTEGER DEFAULT 0,
                has_reverted_ai INTEGER DEFAULT 0,

                -- Language
                languages TEXT,
                detected_language TEXT,

                -- Context (focused app info)
                focused_app TEXT,
                focused_app_name TEXT,
                focused_app_bundle_id TEXT,
                focused_app_window_title TEXT,
                focused_app_window_web_title TEXT,
                focused_app_window_web_domain TEXT,
                focused_app_window_web_url TEXT,
                ax_text TEXT,
                ax_html TEXT,

                -- Mode & metadata
                mode TEXT DEFAULT 'voice_transcript',
                mode_meta TEXT,
                audio_context TEXT,
                status TEXT DEFAULT 'completed',
                app_version TEXT,
                debug_info TEXT,
                client_metadata BLOB
            );
        "),
        // Migration 0001: Create indexes
        (1, "
            CREATE INDEX IF NOT EXISTS idx_history_created_at ON history(created_at);
            CREATE INDEX IF NOT EXISTS idx_history_mode ON history(mode);
            CREATE INDEX IF NOT EXISTS idx_history_status ON history(status);
            CREATE INDEX IF NOT EXISTS idx_history_focused_app_name ON history(focused_app_name);
            CREATE INDEX IF NOT EXISTS idx_history_focused_app_bundle_id ON history(focused_app_bundle_id);
            CREATE INDEX IF NOT EXISTS idx_history_focused_app_window_web_domain ON history(focused_app_window_web_domain);
            CREATE INDEX IF NOT EXISTS idx_history_detected_language ON history(detected_language);
            CREATE INDEX IF NOT EXISTS idx_history_user_id ON history(user_id);
        "),
        // Migration 0002: Usage stats table
        (2, "
            CREATE TABLE IF NOT EXISTS usage_stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                month_key TEXT NOT NULL,
                stt_count INTEGER DEFAULT 0,
                llm_count INTEGER DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(month_key)
            );
        "),
    ];

    for (version, sql) in migrations {
        if version > current_version {
            conn.execute_batch(sql)
                .map_err(|e| format!("Migration {} failed: {}", version, e))?;
            conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [version],
            )
            .map_err(|e| format!("Failed to record migration {}: {}", version, e))?;
        }
    }

    Ok(())
}
