use std::fs;
use std::path::Path;

use rusqlite::Connection;

mod migrations;

pub use migrations::SCHEMA_VERSION;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite 错误：{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("文件系统错误：{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// 打开（必要时创建）数据库，配置连接参数并应用迁移。
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrations::run(&conn)?;
    Ok(conn)
}

/// 内存库（用于单测）。
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrations::run(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    // WAL：异常退出后的恢复能力（文档第 7.3 节）
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    // 预写日志超过约 1MB 就自动合并回主库，避免 WAL 无限增长
    conn.pragma_update(None, "wal_autocheckpoint", 256)?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// 数据库各类计数，用于首页状态与自检。
pub fn stats(conn: &Connection) -> Result<DbStats> {
    let count = |table: &str| -> Result<i64> {
        Ok(conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))?)
    };

    Ok(DbStats {
        meetings: count("meetings")?,
        documents: count("documents")?,
        blobs: count("blobs")?,
        blob_bytes: conn.query_row("SELECT ifnull(sum(size), 0) FROM blobs", [], |row| row.get(0))?,
        annotations: count("annotations")?,
        schema_version: schema_version(conn)?,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStats {
    pub meetings: i64,
    pub documents: i64,
    pub blobs: i64,
    pub blob_bytes: i64,
    pub annotations: i64,
    pub schema_version: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 迁移可重复执行且版本正确() {
        let conn = open_in_memory().expect("打开内存库");
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
        // 再跑一次不应报错（幂等）
        migrations::run(&conn).expect("重复迁移");
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn 关键表均已建立() {
        let conn = open_in_memory().unwrap();
        for table in [
            "meetings",
            "blobs",
            "documents",
            "meeting_documents",
            "topics",
            "topic_documents",
            "page_text",
            "text_blocks",
            "ocr_jobs",
            "ai_results",
            "ai_citations",
            "annotations",
            "reading_state",
            "ai_suggestions",
            "tasks",
            "jobs",
            "net_audit",
            "settings",
            "search_fts",
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "缺少表：{table}");
        }
    }

    #[test]
    fn 默认设置已写入() {
        let conn = open_in_memory().unwrap();
        let value: String = conn
            .query_row("SELECT value FROM settings WHERE key = 'ai.enabled'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, "false");
    }

    #[test]
    fn 同一议题同一材料同一角色只允许一条关系() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO blobs (sha256, rel_path, size) VALUES ('a', 'a.pdf', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (sha256, filename_original, filename_display, ext, size)
             VALUES ('a', 'a.pdf', 'a.pdf', 'pdf', 1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO meetings (title) VALUES ('测试会议')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO topics (meeting_id, title) VALUES (1, '人事处议题')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO topic_documents (meeting_id, topic_id, document_id, role)
             VALUES (1, 1, 1, 'attachment')",
            [],
        )
        .unwrap();
        let duplicated = conn.execute(
            "INSERT INTO topic_documents (meeting_id, topic_id, document_id, role)
             VALUES (1, 1, 1, 'attachment')",
            [],
        );
        assert!(duplicated.is_err(), "重复的附件关系应当被唯一索引拦下");
    }
}
