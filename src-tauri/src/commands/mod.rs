//! 暴露给前端的强类型命令。这是界面层触达本机能力的唯一入口。

use std::sync::MutexGuard;

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::bundle::{self, ExportResult};
use crate::db::{self, DbStats};
use crate::organize::{
    self, DeleteResult, DraftResult, ImportResult, MeetingSummary, MeetingTree,
};
use crate::security::{self, GuardDecision, NetAuditEntry};
use crate::{ocr, AppPaths, AppState};

/// 权限声明文件在编译期内嵌，用于自检"没有引入网络插件权限"。
const CAPABILITIES: &str = include_str!("../../capabilities/default.json");

fn lock<'a>(state: &'a State<'_, AppState>) -> Result<MutexGuard<'a, Connection>, String> {
    state.conn.lock().map_err(|err| format!("数据库连接不可用：{err}"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    local_mode: bool,
    library_root: String,
    db_path: String,
    ocr_engine: String,
    ocr_ready: bool,
    ai_configured: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> Result<AppInfo, String> {
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        local_mode: true,
        library_root: state.paths.library_root.display().to_string(),
        db_path: state.paths.db_path.display().to_string(),
        ocr_engine: ocr::ENGINE_NAME.to_string(),
        ocr_ready: ocr::is_ready(),
        // 本地 AI 在 M8 接入，当前一律按"未配置"处理
        ai_configured: false,
    })
}

#[tauri::command]
pub fn db_stats(state: State<'_, AppState>) -> Result<DbStats, String> {
    let conn = lock(&state)?;
    db::stats(&conn).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn net_audit_list(state: State<'_, AppState>, limit: i64) -> Result<Vec<NetAuditEntry>, String> {
    let conn = lock(&state)?;
    security::recent(&conn, limit.clamp(1, 500)).map_err(|err| err.to_string())
}

/// 手动试探网络守卫：把地址丢进来，看是否被拒绝（并留下审计记录）。
#[tauri::command]
pub fn guard_probe(state: State<'_, AppState>, url: String) -> Result<GuardDecision, String> {
    let conn = lock(&state)?;
    Ok(security::probe(&conn, &url, "手动试探"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrSelfTestResult {
    ok: bool,
    engine: String,
    engine_version: String,
    elapsed_ms: u64,
    recognized_text: String,
    matched_key_phrase: bool,
    block_count: usize,
    error: Option<String>,
}

#[tauri::command]
pub fn ocr_self_test() -> Result<OcrSelfTestResult, String> {
    match ocr::selftest() {
        Ok(result) => Ok(OcrSelfTestResult {
            ok: result.ok,
            engine: result.engine,
            engine_version: result.engine_version,
            elapsed_ms: result.elapsed_ms,
            recognized_text: result.plain_text,
            matched_key_phrase: result.matched_key_phrase,
            block_count: result.blocks.len(),
            error: result.error,
        }),
        Err(err) => Ok(OcrSelfTestResult {
            ok: false,
            engine: ocr::ENGINE_NAME.to_string(),
            engine_version: String::new(),
            elapsed_ms: 0,
            recognized_text: String::new(),
            matched_key_phrase: false,
            block_count: 0,
            error: Some(err),
        }),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfCheckItem {
    id: String,
    label: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfCheckReport {
    ok: bool,
    checked_at: String,
    items: Vec<SelfCheckItem>,
}

/// 离线自检：把"完全本地"这件事变成可验证的结果，而不是宣传语。
#[tauri::command]
pub fn offline_self_check(state: State<'_, AppState>) -> Result<SelfCheckReport, String> {
    let conn = lock(&state)?;
    let mut items = Vec::new();

    // 1. 数据库可写
    match conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES ('__selftest', '1')",
        [],
    ) {
        Ok(_) => {
            let _ = conn.execute("DELETE FROM settings WHERE key = '__selftest'", []);
            items.push(item(
                "db",
                "数据库可读写",
                true,
                "SQLite 事务正常".to_string(),
            ));
        }
        Err(err) => items.push(item("db", "数据库可读写", false, err.to_string())),
    }

    // 2. FTS5 编译选项
    let fts5: i64 = conn
        .query_row("SELECT sqlite_compileoption_used('ENABLE_FTS5')", [], |row| {
            row.get(0)
        })
        .unwrap_or(0);
    items.push(item(
        "fts5",
        "SQLite 全文检索（FTS5）",
        fts5 == 1,
        if fts5 == 1 {
            "随包编译，已启用".to_string()
        } else {
            "未启用，检索不可用".to_string()
        },
    ));

    // 3. 中文子串检索（trigram 分词器实测）
    let chinese_search = probe_chinese_search(&conn);
    items.push(item(
        "chinese_search",
        "中文全文检索",
        chinese_search.is_ok(),
        chinese_search
            .map(|_| "trigram 分词器工作正常（可子串匹配）".to_string())
            .unwrap_or_else(|err| err),
    ));

    // 4. 库目录可写
    match probe_library_writable(&state.paths) {
        Ok(()) => items.push(item(
            "library",
            "材料库目录可写",
            true,
            state.paths.library_root.display().to_string(),
        )),
        Err(err) => items.push(item("library", "材料库目录可写", false, err)),
    }

    // 5. 本地 OCR
    match ocr::selftest() {
        Ok(result) if result.ok => items.push(item(
            "ocr",
            "本地 OCR（系统 Vision 框架）",
            true,
            format!(
                "引擎 {}，耗时 {} ms，识别 {} 个文本块",
                result.engine_version,
                result.elapsed_ms,
                result.blocks.len()
            ),
        )),
        Ok(result) => items.push(item(
            "ocr",
            "本地 OCR（系统 Vision 框架）",
            false,
            result.error.unwrap_or_else(|| "识别失败".to_string()),
        )),
        Err(err) => items.push(item("ocr", "本地 OCR（系统 Vision 框架）", false, err)),
    }

    // 6. 网络守卫：公网必须被拒，回环必须放行
    let public_blocked = security::check("https://example.com/collect").is_err();
    let loopback_allowed = security::check("http://127.0.0.1:11434/api/tags").is_ok();
    items.push(item(
        "guard",
        "网络守卫（只允许本机回环）",
        public_blocked && loopback_allowed,
        format!("公网拒绝：{public_blocked}　回环放行：{loopback_allowed}"),
    ));

    // 7. 能力声明中不含网络插件
    let risky = ["http:", "shell:", "process:"]
        .iter()
        .filter(|needle| CAPABILITIES.contains(*needle))
        .count();
    items.push(item(
        "capabilities",
        "应用未申请网络相关权限",
        risky == 0,
        if risky == 0 {
            "capabilities 中无 http / shell / process 权限".to_string()
        } else {
            format!("发现 {risky} 项可疑权限")
        },
    ));

    // 8. 出站请求统计
    let permitted = security::permitted_count(&conn).unwrap_or(-1);
    items.push(item(
        "egress",
        "已放行的出站请求",
        permitted == 0,
        format!("累计 {permitted} 次（未配置本地 AI 时应为 0）"),
    ));

    let ok = items.iter().all(|entry| entry.ok);
    let checked_at: String = conn
        .query_row("SELECT datetime('now', 'localtime')", [], |row| row.get(0))
        .unwrap_or_else(|_| String::from("未知"));

    Ok(SelfCheckReport {
        ok,
        checked_at,
        items,
    })
}

#[tauri::command]
pub fn create_meeting(
    state: State<'_, AppState>,
    title: String,
) -> Result<CreatedMeeting, String> {
    let conn = lock(&state)?;
    let title = if title.trim().is_empty() {
        "未命名会议".to_string()
    } else {
        title.trim().to_string()
    };
    conn.execute("INSERT INTO meetings (title) VALUES (?1)", [&title])
        .map_err(|err| err.to_string())?;
    let id = conn.last_insert_rowid();
    Ok(CreatedMeeting { id, title })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedMeeting {
    id: i64,
    title: String,
}

// ── 会议整理：导入、角色指定、结构查询 ──────────────────────────────

/// 导入用户拖进来的文件夹（递归展开）或零散文件。
#[tauri::command]
pub fn import_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<ImportResult, String> {
    let conn = lock(&state)?;
    organize::import_paths(&conn, &state.paths.library_root, &paths)
}

#[tauri::command]
pub fn list_meetings(state: State<'_, AppState>) -> Result<Vec<MeetingSummary>, String> {
    let conn = lock(&state)?;
    organize::list_meetings(&conn)
}

/// 删除一场会议。材料文件会移到 `library/trash/`，不会硬删。
#[tauri::command]
pub fn delete_meeting(
    state: State<'_, AppState>,
    meeting_id: i64,
) -> Result<DeleteResult, String> {
    let conn = lock(&state)?;
    organize::delete_meeting(&conn, &state.paths.library_root, meeting_id)
}

#[tauri::command]
pub fn meeting_tree(state: State<'_, AppState>, meeting_id: i64) -> Result<MeetingTree, String> {
    let conn = lock(&state)?;
    organize::meeting_tree(&conn, meeting_id)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedTopic {
    topic_id: i64,
}

#[tauri::command]
pub fn create_topic(
    state: State<'_, AppState>,
    meeting_id: i64,
    title: String,
    department: Option<String>,
) -> Result<CreatedTopic, String> {
    let conn = lock(&state)?;
    let topic_id = organize::create_topic(&conn, meeting_id, &title, department.as_deref())?;
    Ok(CreatedTopic { topic_id })
}

/// 手动指定某份材料的角色：agenda（议程）/ main（议题主材料）/
/// attachment（议题附件）/ other（其他材料）
#[tauri::command]
pub fn assign_document(
    state: State<'_, AppState>,
    meeting_id: i64,
    document_id: i64,
    role: String,
    topic_id: Option<i64>,
    attachment_label: Option<String>,
) -> Result<(), String> {
    let conn = lock(&state)?;
    organize::assign_document(
        &conn,
        meeting_id,
        document_id,
        &role,
        topic_id,
        attachment_label,
    )
}

#[tauri::command]
pub fn clear_document_role(
    state: State<'_, AppState>,
    meeting_id: i64,
    document_id: i64,
) -> Result<(), String> {
    let conn = lock(&state)?;
    organize::clear_document_role(&conn, meeting_id, document_id)
}

/// 按原文件夹结构生成议题草稿：部门 → 议题 → 材料。
/// 只处理尚未归类的材料，不覆盖人工已安排的结构。
#[tauri::command]
pub fn auto_draft_topics(
    state: State<'_, AppState>,
    meeting_id: i64,
) -> Result<DraftResult, String> {
    let conn = lock(&state)?;
    organize::auto_draft_topics(&conn, meeting_id)
}

/// 在某个议题里指定主材料，其余材料自动重编号为附件1..N。
#[tauri::command]
pub fn set_topic_main(
    state: State<'_, AppState>,
    meeting_id: i64,
    topic_id: i64,
    document_id: i64,
) -> Result<(), String> {
    let conn = lock(&state)?;
    organize::set_topic_main(&conn, meeting_id, topic_id, document_id)
}

/// 把整场会议导出成**一个 HTML 文件**，拷进 U 盘即可在会议室电脑上双击打开。
#[tauri::command]
pub fn export_bundle(state: State<'_, AppState>, meeting_id: i64) -> Result<ExportResult, String> {
    let conn = lock(&state)?;
    let out_dir = state.paths.library_root.join("导出");
    bundle::export_single_html(&conn, &state.paths.library_root, meeting_id, &out_dir)
}

/// 在访达里定位到某个文件（导出完成后告诉用户"文件在这儿"）。
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .args(["-R", &path])
        .spawn()
        .map_err(|err| format!("无法在访达中显示：{err}"))?;
    Ok(())
}

fn item(id: &str, label: &str, ok: bool, detail: String) -> SelfCheckItem {
    SelfCheckItem {
        id: id.to_string(),
        label: label.to_string(),
        ok,
        detail,
    }
}

fn probe_chinese_search(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS temp.fts_probe USING fts5(body, tokenize='trigram');
         DELETE FROM temp.fts_probe;
         INSERT INTO temp.fts_probe(body) VALUES ('关于2026年度预算方案的请示');",
    )
    .map_err(|err| err.to_string())?;

    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM temp.fts_probe WHERE fts_probe MATCH '\"预算方案\"'",
            [],
            |row| row.get(0),
        )
        .map_err(|err| err.to_string())?;

    let _ = conn.execute_batch("DROP TABLE IF EXISTS temp.fts_probe");

    if hits == 1 {
        Ok(())
    } else {
        Err(format!("中文子串检索未命中（命中 {hits} 条）"))
    }
}

fn probe_library_writable(paths: &AppPaths) -> Result<(), String> {
    let probe = paths.library_root.join(".meetingdesk-selftest");
    std::fs::write(&probe, b"ok").map_err(|err| format!("写入失败：{err}"))?;
    std::fs::remove_file(&probe).map_err(|err| format!("删除失败：{err}"))?;
    Ok(())
}
