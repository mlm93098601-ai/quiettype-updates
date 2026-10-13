use super::models::HistoryRecord;
use super::Database;
use rusqlite::{params, Connection};
use tauri::State;

pub fn create_record(
    state: State<'_, Database>,
    text: &str,
    duration: Option<f64>,
) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    super::purge_expired(&conn, chrono::Utc::now()).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO history (id, created_at, updated_at, refined_text, edited_text, duration, mode, status, app_version)
         VALUES (?1, ?2, ?2, ?3, ?3, ?4, 'voice_transcript', 'completed', ?5)",
        params![uuid::Uuid::new_v4().to_string(), now, text, duration, env!("CARGO_PKG_VERSION")],
    ).map_err(|e| format!("保存记录失败: {e}"))?;
    Ok(())
}

pub fn get_records(
    state: State<'_, Database>,
    query: &str,
    offset: u32,
) -> Result<Vec<HistoryRecord>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    super::purge_expired(&conn, chrono::Utc::now()).map_err(|e| e.to_string())?;
    read_records(&conn, query, offset).map_err(|e| e.to_string())
}

pub fn delete_record(state: State<'_, Database>, id: &str) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    delete_by_id(&conn, id)
}

fn delete_by_id(conn: &Connection, id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 100 {
        return Err("记录编号无效".into());
    }
    conn.execute("DELETE FROM history WHERE id = ?1", [id])
        .map(|_| ())
        .map_err(|e| format!("删除记录失败: {e}"))
}

pub fn clear_records(state: State<'_, Database>) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM history", [])
        .map(|_| ())
        .map_err(|e| format!("清空记录失败: {e}"))
}

fn read_records(
    conn: &Connection,
    query: &str,
    offset: u32,
) -> rusqlite::Result<Vec<HistoryRecord>> {
    let query = query.trim();
    // Search text literally, including percent and underscore characters.
    let pattern = format!(
        "%{}%",
        query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    conn.prepare(
        "SELECT id, user_id, created_at, updated_at, audio_local_path, duration,
                mic_device, audio_metadata, refined_text, edited_text,
                edited_text_status, has_reverted_ai, languages, detected_language,
                focused_app, focused_app_name, focused_app_bundle_id,
                focused_app_window_title, focused_app_window_web_title,
                focused_app_window_web_domain, focused_app_window_web_url,
                ax_text, ax_html, mode, mode_meta, audio_context, status,
                app_version, debug_info
         FROM history
         WHERE ?1 = '' OR refined_text LIKE ?2 ESCAPE '\\' OR edited_text LIKE ?2 ESCAPE '\\'
               OR focused_app_name LIKE ?2 ESCAPE '\\' OR focused_app_window_title LIKE ?2 ESCAPE '\\'
         ORDER BY created_at DESC, id DESC LIMIT 50 OFFSET ?3"
    )?.query_map(params![query, pattern, offset], history_from_row)?.collect()
}

fn history_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRecord> {
    Ok(HistoryRecord {
        id: row.get(0)?,
        user_id: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
        audio_local_path: row.get(4)?,
        duration: row.get(5)?,
        mic_device: row.get(6)?,
        audio_metadata: row.get(7)?,
        refined_text: row.get(8)?,
        edited_text: row.get(9)?,
        edited_text_status: row.get(10)?,
        has_reverted_ai: row.get::<_, Option<i32>>(11)?.map(|v| v != 0),
        languages: row.get(12)?,
        detected_language: row.get(13)?,
        focused_app: row.get(14)?,
        focused_app_name: row.get(15)?,
        focused_app_bundle_id: row.get(16)?,
        focused_app_window_title: row.get(17)?,
        focused_app_window_web_title: row.get(18)?,
        focused_app_window_web_domain: row.get(19)?,
        focused_app_window_web_url: row.get(20)?,
        ax_text: row.get(21)?,
        ax_html: row.get(22)?,
        mode: row.get(23)?,
        mode_meta: row.get(24)?,
        audio_context: row.get(25)?,
        status: row.get(26)?,
        app_version: row.get(27)?,
        debug_info: row.get(28)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_is_limited_to_the_requested_record() {
        let conn = Connection::open_in_memory().unwrap();
        crate::database::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO history (id, refined_text) VALUES ('one', '保留'), ('two', '删除')",
            [],
        )
        .unwrap();
        assert!(delete_by_id(&conn, "").is_err());
        delete_by_id(&conn, "two' OR 1=1 --").unwrap();
        assert_eq!(read_records(&conn, "", 0).unwrap().len(), 2);
        delete_by_id(&conn, "two").unwrap();
        let remaining = read_records(&conn, "", 0).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "one");
    }
    #[test]
    fn old_history_survives_migrations_search_and_paging() {
        let conn = Connection::open_in_memory().unwrap();
        crate::database::migrations::run(&conn).unwrap();
        for index in 0..56 {
            let text = if index == 50 {
                "预算 50%".into()
            } else {
                format!("句子 {index}")
            };
            conn.execute("INSERT INTO history (id, created_at, refined_text, mode, has_reverted_ai, mic_device) VALUES (?1, '2020-01-01', ?2, 'lip_transcript', 1, '旧麦克风')", params![format!("{index:03}"), text]).unwrap();
        }
        crate::database::migrations::run(&conn).unwrap();
        let first = read_records(&conn, "", 0).unwrap();
        let next = read_records(&conn, "", 50).unwrap();
        assert_eq!(first.len(), 50);
        assert_eq!(next.len(), 6);
        assert_eq!(first[0].id, "055");
        assert_eq!(next[0].id, "005");
        let found = read_records(&conn, "50%", 0).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].refined_text.as_deref(), Some("预算 50%"));
        assert_eq!(found[0].mode, "lip_transcript");
        assert_eq!(found[0].has_reverted_ai, Some(true));
        assert_eq!(found[0].mic_device.as_deref(), Some("旧麦克风"));
        assert!(read_records(&conn, "不存在", 0).unwrap().is_empty());
    }
}
