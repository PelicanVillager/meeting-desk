//! 全文检索：SQLite FTS5 + trigram 分词器。
//!
//! 为什么用 trigram：默认 unicode61 不切分中文，连续中文会退化成一个 token，
//! 导致"预算"搜不到"关于预算的说明"。trigram 支持子串匹配，中英混排通吃。
//! 代价是查询词短于 3 个字符时无法走索引，此时回退 LIKE 扫描。

use rusqlite::{Connection, Result};

/// 索引来源：原文 / OCR / 用户备注 / AI 结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Original,
    Ocr,
    Note,
    Ai,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Original => "original",
            Source::Ocr => "ocr",
            Source::Note => "note",
            Source::Ai => "ai",
        }
    }
}

/// rowid 约定（见 migrations/0001_init.sql），保证重复索引时能幂等覆盖。
fn rowid_for(source: Source, id: i64, page_no: i64) -> i64 {
    match source {
        Source::Original | Source::Ocr => 1_000_000 * id + page_no,
        Source::Note => 1_000_000_000_000 + id,
        Source::Ai => 2_000_000_000_000 + id,
    }
}

/// 写入/覆盖一条索引。
pub fn index(
    conn: &Connection,
    source: Source,
    id: i64,
    document_id: i64,
    page_no: i64,
    text: &str,
) -> Result<()> {
    let rowid = rowid_for(source, id, page_no);
    conn.execute("DELETE FROM search_fts WHERE rowid = ?1", [rowid])?;
    if text.trim().is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO search_fts (rowid, document_id, source, page_no, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![rowid, document_id, source.as_str(), page_no, text],
    )?;
    Ok(())
}

/// 删除某份材料的全部索引（重新解析前调用）。
pub fn remove_document(conn: &Connection, document_id: i64) -> Result<usize> {
    conn.execute(
        "DELETE FROM search_fts WHERE source IN ('original','ocr') AND document_id = ?1",
        [document_id],
    )
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub document_id: i64,
    pub source: String,
    pub page_no: i64,
    pub snippet: String,
}

/// 查询。>=3 个字符走 trigram 索引；短词回退 LIKE。
pub fn query(conn: &Connection, raw: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let needle = raw.trim();
    if needle.is_empty() {
        return Ok(Vec::new());
    }

    if needle.chars().count() >= 3 {
        // FTS5 查询串需要转义双引号，并用双引号包裹以按子串匹配
        let escaped = needle.replace('"', "\"\"");
        let match_expr = format!("\"{escaped}\"");
        let mut stmt = conn.prepare(
            "SELECT document_id, source, page_no,
                    snippet(search_fts, 3, '⟦', '⟧', '…', 12)
             FROM search_fts
             WHERE search_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![match_expr, limit], map_hit)?;
        return rows.collect();
    }

    let like = format!("%{needle}%");
    let mut stmt = conn.prepare(
        "SELECT document_id, source, page_no, substr(body, 1, 80)
         FROM search_fts
         WHERE body LIKE ?1
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![like, limit], map_hit)?;
    rows.collect()
}

fn map_hit(row: &rusqlite::Row<'_>) -> Result<SearchHit> {
    Ok(SearchHit {
        document_id: row.get(0)?,
        source: row.get(1)?,
        page_no: row.get(2)?,
        snippet: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 建库并索引() -> Connection {
        let conn = crate::db::open_in_memory().unwrap();
        index(
            &conn,
            Source::Original,
            1,
            1,
            3,
            "关于人事处2026年度预算方案的请示，会议同意按128万元安排预算。",
        )
        .unwrap();
        index(
            &conn,
            Source::Note,
            7,
            1,
            3,
            "领导要求补充预算数据。",
        )
        .unwrap();
        conn
    }

    #[test]
    fn 中文长词走索引可命中() {
        let conn = 建库并索引();
        let hits = query(&conn, "预算方案", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "original");
        assert!(hits[0].snippet.contains("预算"), "片段：{}", hits[0].snippet);
    }

    #[test]
    fn 中文短词回退like也能命中() {
        let conn = 建库并索引();
        let hits = query(&conn, "预算", 10).unwrap();
        assert_eq!(hits.len(), 2, "原文与备注都应命中");
        let sources: Vec<&str> = hits.iter().map(|h| h.source.as_str()).collect();
        assert!(sources.contains(&"original"));
        assert!(sources.contains(&"note"));
    }

    #[test]
    fn 英文与数字可命中() {
        let conn = 建库并索引();
        assert_eq!(query(&conn, "2026", 10).unwrap().len(), 1);
    }

    #[test]
    fn 重复索引会覆盖而不是重复累积() {
        let conn = 建库并索引();
        index(&conn, Source::Original, 1, 1, 3, "改成了别的正文内容").unwrap();
        let hits = query(&conn, "预算", 10).unwrap();
        assert_eq!(hits.len(), 1, "原文旧索引应被覆盖");
    }

    #[test]
    fn 删除材料后索引清空() {
        let conn = 建库并索引();
        remove_document(&conn, 1).unwrap();
        assert_eq!(query(&conn, "预算", 10).unwrap().len(), 1, "只剩备注索引");
    }
}
