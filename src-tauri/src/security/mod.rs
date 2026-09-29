//! 网络安全守卫与出站审计。
//!
//! 这是整个应用唯一的"出网闸门"：除本机回环地址（本地大模型）外，
//! 任何目标都会被拒绝，并留下审计记录。

use rusqlite::Connection;
use url::Url;

/// 唯一允许的出站目标：本机回环地址。
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "::1", "localhost"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GuardError {
    #[error("地址无法解析：{0}")]
    Invalid(String),
    #[error("协议不允许：{0}（只允许 http / https）")]
    Scheme(String),
    #[error("目标不是本机回环地址：{0}")]
    NonLoopback(String),
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardDecision {
    pub target: String,
    pub allowed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetAuditEntry {
    pub id: i64,
    pub ts: String,
    pub target: String,
    pub purpose: String,
    pub allowed: bool,
    pub note: Option<String>,
}

/// 判断某个地址是否允许出站访问。这是唯一的判定入口。
pub fn check(raw: &str) -> Result<Url, GuardError> {
    let url = Url::parse(raw).map_err(|err| GuardError::Invalid(err.to_string()))?;

    match url.scheme() {
        "http" | "https" => {}
        other => return Err(GuardError::Scheme(other.to_string())),
    }

    let host = url
        .host_str()
        .ok_or_else(|| GuardError::Invalid("缺少主机名".to_string()))?;

    // url crate 对 IPv6 会带上方括号（[::1]），统一去掉再比较
    let host = host.trim_start_matches('[').trim_end_matches(']');

    // 必须精确匹配回环地址，避免 127.0.0.1.evil.com 之类的前缀伪装
    if LOOPBACK_HOSTS.contains(&host) {
        Ok(url)
    } else {
        Err(GuardError::NonLoopback(host.to_string()))
    }
}

/// 判定并写审计：probe 供设置页自检与 AI 适配器调用。
pub fn probe(conn: &Connection, raw: &str, purpose: &str) -> GuardDecision {
    match check(raw) {
        Ok(url) => {
            let target = url.to_string();
            let _ = audit(conn, &target, purpose, true, None);
            GuardDecision {
                target,
                allowed: true,
                reason: "本机回环地址，允许".to_string(),
            }
        }
        Err(err) => {
            let reason = err.to_string();
            let _ = audit(conn, raw, purpose, false, Some(&reason));
            GuardDecision {
                target: raw.to_string(),
                allowed: false,
                reason,
            }
        }
    }
}

pub fn audit(
    conn: &Connection,
    target: &str,
    purpose: &str,
    allowed: bool,
    note: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO net_audit (target, purpose, allowed, note) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![target, purpose, if allowed { 1 } else { 0 }, note],
    )?;
    Ok(())
}

pub fn recent(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<NetAuditEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, target, purpose, allowed, note
         FROM net_audit ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(NetAuditEntry {
            id: row.get(0)?,
            ts: row.get(1)?,
            target: row.get(2)?,
            purpose: row.get(3)?,
            allowed: row.get::<_, i64>(4)? == 1,
            note: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// 被允许的出站请求数量：正常情况下（未配置本地 AI）应当为 0。
pub fn permitted_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT count(*) FROM net_audit WHERE allowed = 1", [], |row| {
        row.get(0)
    })
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)] // 测试名用中文描述，读起来更直观

    use super::*;

    #[test]
    fn 允许本机回环地址() {
        assert!(check("http://127.0.0.1:11434/api/chat").is_ok());
        assert!(check("http://localhost:8080/v1/chat/completions").is_ok());
        assert!(check("http://[::1]:11434/api/tags").is_ok());
    }

    #[test]
    fn 拒绝公网地址并给出原因() {
        let err = check("https://api.openai.com/v1/chat/completions").unwrap_err();
        assert!(matches!(err, GuardError::NonLoopback(_)), "实际：{err:?}");
    }

    #[test]
    fn 拒绝伪装地址() {
        // userinfo 伪装：真实主机是 evil.com
        assert!(check("http://127.0.0.1@evil.com/upload").is_err());
        // 前缀伪装
        assert!(check("http://127.0.0.1.evil.com/").is_err());
        // 局域网地址也不允许
        assert!(check("http://192.168.1.10:8000/").is_err());
    }

    #[test]
    fn 拒绝非HTTP协议() {
        assert!(matches!(check("file:///etc/passwd"), Err(GuardError::Scheme(_))));
        assert!(matches!(
            check("ftp://127.0.0.1/x"),
            Err(GuardError::Scheme(_))
        ));
    }

    #[test]
    fn 审计记录被拒绝的尝试() {
        let conn = crate::db::open_in_memory().unwrap();
        let decision = probe(&conn, "https://example.com/upload", "单元测试");
        assert!(!decision.allowed);

        let entries = recent(&conn, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].allowed);
        assert_eq!(permitted_count(&conn).unwrap(), 0, "未配置本地 AI 时不应有放行的请求");
    }
}
