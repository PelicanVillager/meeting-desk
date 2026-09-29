//! 会议整理：把拖进来的文件夹导入本机库，并记录"谁是议程、谁是议题主材料、
//! 谁是附件"这些人工指定的关系。
//!
//! 设计纪律（见 docs/00-需求分析与技术方案-v0.1.md）：
//!   · 文件字节只存一份（内容寻址），关系可以有多份
//!   · 角色一律由人指定，程序不猜（自动识别是后续的"建议"，仍要人确认）
//!   · 原始文件只读，绝不覆盖

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::library;

const SKIP_NAMES: [&str; 3] = [".DS_Store", "Thumbs.db", ".localized"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedFile {
    pub document_id: i64,
    pub filename: String,
    pub display_name: String,
    pub ext: String,
    pub size: i64,
    pub folder_hint: Option<String>,
    pub is_duplicate: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub meeting_id: i64,
    pub meeting_title: String,
    pub meeting_date: Option<String>,
    pub files: Vec<ImportedFile>,
    pub skipped: Vec<String>,
    pub duplicate_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeDocument {
    pub document_id: i64,
    pub display_name: String,
    pub filename: String,
    pub ext: String,
    pub size: i64,
    pub role: String,
    pub attachment_label: Option<String>,
    pub folder_hint: Option<String>,
    pub order_index: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicNode {
    pub topic_id: i64,
    pub title: String,
    pub department: Option<String>,
    pub order_index: i64,
    pub main: Option<TreeDocument>,
    pub attachments: Vec<TreeDocument>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingTree {
    pub meeting_id: i64,
    pub title: String,
    pub meeting_date: Option<String>,
    pub agenda: Option<TreeDocument>,
    pub topics: Vec<TopicNode>,
    pub unassigned: Vec<TreeDocument>,
    pub others: Vec<TreeDocument>,
    pub total_documents: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummary {
    pub meeting_id: i64,
    pub title: String,
    pub meeting_date: Option<String>,
    pub document_count: i64,
    pub created_at: String,
}

// ── 文件名清洗与日期猜测 ─────────────────────────────────────────────

/// 生成用于显示的清洗名：控制字符（含换行）→ 空格，压缩连续空白，去掉非法字符。
/// 真实材料里确实存在"文件名带换行符"的情况（网页复制粘贴的产物）。
pub fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
}

/// 从会议名里猜日期：支持 20260929、2026.09.29、2026年9月29日 等写法。
pub fn guess_meeting_date(title: &str) -> Option<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in title.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }

    let valid = |month: u32, day: u32| (1..=12).contains(&month) && (1..=31).contains(&day);

    // 形如 20260929
    if let Some(run) = runs.iter().find(|run| run.len() == 8) {
        let (year, month, day) = (&run[0..4], &run[4..6], &run[6..8]);
        let (m, d) = (month.parse().ok()?, day.parse().ok()?);
        if valid(m, d) {
            return Some(format!("{year}-{m:02}-{d:02}"));
        }
    }

    // 形如 2026.09.29 / 2026年9月29日
    if runs.len() >= 3 && runs[0].len() == 4 {
        let year = runs[0].clone();
        let m: u32 = runs[1].parse().ok()?;
        let d: u32 = runs[2].parse().ok()?;
        if valid(m, d) {
            return Some(format!("{year}-{m:02}-{d:02}"));
        }
    }

    None
}

// ── 目录扫描 ────────────────────────────────────────────────────────

struct SourceFile {
    path: PathBuf,
    folder_hint: Option<String>,
}

fn collect_dir(root: &Path) -> Result<Vec<SourceFile>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|err| format!("读取目录失败 {}：{err}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|err| err.to_string())?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || SKIP_NAMES.contains(&name.as_str()) {
                continue;
            }
            let path = entry.path();
            let meta = entry.metadata().map_err(|err| err.to_string())?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let folder_hint = path
                .parent()
                .and_then(|parent| parent.strip_prefix(root).ok())
                .map(|rel| rel.to_string_lossy().to_string())
                .filter(|rel| !rel.is_empty());
            out.push(SourceFile { path, folder_hint });
        }
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

// ── 导入 ────────────────────────────────────────────────────────────

/// 导入用户拖进来的路径：文件夹会递归展开，文件按单份材料处理。
pub fn import_paths(
    conn: &Connection,
    library_root: &Path,
    paths: &[String],
) -> Result<ImportResult, String> {
    if paths.is_empty() {
        return Err("没有收到任何路径".to_string());
    }

    let mut sources: Vec<SourceFile> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut root_dirs: Vec<PathBuf> = Vec::new();

    for raw in paths {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            root_dirs.push(path.clone());
            sources.extend(collect_dir(&path)?);
        } else if path.is_file() {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string());
            if let Some(name) = name {
                if name.starts_with('.') || SKIP_NAMES.contains(&name.as_str()) {
                    skipped.push(name);
                    continue;
                }
            }
            sources.push(SourceFile {
                path,
                folder_hint: None,
            });
        } else {
            skipped.push(raw.clone());
        }
    }

    if sources.is_empty() {
        return Err("这些路径里没有找到可导入的文件".to_string());
    }

    // 会议标题：单个文件夹用文件夹名，否则用共同父目录名
    let meeting_title = match root_dirs.len() {
        1 if paths.len() == 1 => root_dirs[0]
            .file_name()
            .map(|name| sanitize_name(&name.to_string_lossy()))
            .unwrap_or_else(|| "未命名会议".to_string()),
        _ => {
            let first = sources[0].path.parent().unwrap_or(Path::new(""));
            let name = first
                .file_name()
                .map(|name| sanitize_name(&name.to_string_lossy()))
                .unwrap_or_else(|| "未命名会议".to_string());
            if sources.len() > 1 {
                format!("{name}（{count} 份材料）", count = sources.len())
            } else {
                name
            }
        }
    };
    let meeting_date = guess_meeting_date(&meeting_title);

    conn.execute(
        "INSERT INTO meetings (title, meeting_date) VALUES (?1, ?2)",
        params![meeting_title, meeting_date],
    )
    .map_err(|err| err.to_string())?;
    let meeting_id = conn.last_insert_rowid();

    let mut files = Vec::new();
    let mut duplicate_count = 0usize;

    for source in sources {
        let sha256 = library::sha256_file(&source.path)
            .map_err(|err| format!("计算哈希失败 {}：{err}", source.path.display()))?;
        let ext = source
            .path
            .extension()
            .map(|value| value.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let filename = source
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "未命名".to_string());
        let display_name = sanitize_name(
            source
                .path
                .file_stem()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| filename.clone())
                .as_str(),
        );
        let size = std::fs::metadata(&source.path)
            .map(|meta| meta.len() as i64)
            .unwrap_or(0);

        let blob_exists: Option<String> = conn
            .query_row("SELECT sha256 FROM blobs WHERE sha256 = ?1", [&sha256], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|err| err.to_string())?;

        let rel_path = library::blob_rel_path(&sha256, &ext);
        let target = library_root.join(&rel_path);
        if blob_exists.is_none() || !target.exists() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            std::fs::copy(&source.path, &target)
                .map_err(|err| format!("复制到材料库失败 {}：{err}", source.path.display()))?;
        }
        if blob_exists.is_none() {
            conn.execute(
                "INSERT INTO blobs (sha256, rel_path, size, ref_count) VALUES (?1, ?2, ?3, 0)",
                params![sha256, rel_path.to_string_lossy(), size],
            )
            .map_err(|err| err.to_string())?;
        }

        let existing_document: Option<i64> = conn
            .query_row(
                "SELECT id FROM documents WHERE sha256 = ?1 AND filename_original = ?2",
                params![sha256, filename],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| err.to_string())?;

        let (document_id, is_duplicate) = match existing_document {
            Some(id) => (id, true),
            None => {
                conn.execute(
                    "INSERT INTO documents
                       (sha256, filename_original, filename_display, ext, size, source_path, parse_status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'imported')",
                    params![
                        sha256,
                        filename,
                        display_name,
                        ext,
                        size,
                        source.path.to_string_lossy()
                    ],
                )
                .map_err(|err| err.to_string())?;
                let id = conn.last_insert_rowid();
                conn.execute(
                    "UPDATE blobs SET ref_count = ref_count + 1 WHERE sha256 = ?1",
                    [&sha256],
                )
                .map_err(|err| err.to_string())?;
                (id, false)
            }
        };
        if is_duplicate {
            duplicate_count += 1;
        }

        conn.execute(
            "INSERT OR IGNORE INTO meeting_documents (meeting_id, document_id, folder_hint)
             VALUES (?1, ?2, ?3)",
            params![meeting_id, document_id, source.folder_hint],
        )
        .map_err(|err| err.to_string())?;

        files.push(ImportedFile {
            document_id,
            filename,
            display_name,
            ext,
            size,
            folder_hint: source.folder_hint,
            is_duplicate,
        });
    }

    Ok(ImportResult {
        meeting_id,
        meeting_title,
        meeting_date,
        files,
        skipped,
        duplicate_count,
    })
}

// ── 角色与结构 ──────────────────────────────────────────────────────

/// 指定某份材料在会议里的角色。
/// role: agenda | main | attachment | other
pub fn assign_document(
    conn: &Connection,
    meeting_id: i64,
    document_id: i64,
    role: &str,
    topic_id: Option<i64>,
    attachment_label: Option<String>,
) -> Result<(), String> {
    if !["agenda", "main", "attachment", "other"].contains(&role) {
        return Err(format!("未知角色：{role}"));
    }
    if matches!(role, "main" | "attachment") && topic_id.is_none() {
        return Err("议题主材料与附件必须归属某个议题".to_string());
    }

    // 一份材料在一个会议里只有一个角色，先清掉旧关系
    conn.execute(
        "DELETE FROM topic_documents WHERE meeting_id = ?1 AND document_id = ?2",
        params![meeting_id, document_id],
    )
    .map_err(|err| err.to_string())?;

    let order_index: i64 = conn
        .query_row(
            "SELECT ifnull(max(order_index), 0) + 1000 FROM topic_documents
             WHERE meeting_id = ?1 AND ifnull(topic_id, 0) = ifnull(?2, 0)",
            params![meeting_id, topic_id],
            |row| row.get(0),
        )
        .map_err(|err| err.to_string())?;

    let label = match role {
        "attachment" => {
            let topic = topic_id.ok_or_else(|| "附件必须归属某个议题".to_string())?;
            Some(match attachment_label {
                Some(value) if !value.trim().is_empty() => value,
                _ => auto_attachment_label(conn, meeting_id, topic)?,
            })
        }
        _ => None,
    };

    conn.execute(
        "INSERT INTO topic_documents
           (meeting_id, topic_id, document_id, role, order_index, attachment_label, origin)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'manual')",
        params![meeting_id, topic_id, document_id, role, order_index, label],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

fn auto_attachment_label(
    conn: &Connection,
    meeting_id: i64,
    topic_id: i64,
) -> Result<String, String> {
    let used: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT attachment_label FROM topic_documents
                 WHERE meeting_id = ?1 AND topic_id = ?2 AND role = 'attachment'",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map(params![meeting_id, topic_id], |row| row.get::<_, Option<String>>(0))
            .map_err(|err| err.to_string())?;
        rows.filter_map(Result::ok).flatten().collect()
    };

    for index in 1..=99 {
        let candidate = format!("附件{index}");
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Ok("附件".to_string())
}

pub fn clear_document_role(
    conn: &Connection,
    meeting_id: i64,
    document_id: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM topic_documents WHERE meeting_id = ?1 AND document_id = ?2",
        params![meeting_id, document_id],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

pub fn create_topic(
    conn: &Connection,
    meeting_id: i64,
    title: &str,
    department: Option<&str>,
) -> Result<i64, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("议题名称不能为空".to_string());
    }
    let order_index: i64 = conn
        .query_row(
            "SELECT ifnull(max(order_index), 0) + 1000 FROM topics WHERE meeting_id = ?1",
            [meeting_id],
            |row| row.get(0),
        )
        .map_err(|err| err.to_string())?;

    conn.execute(
        "INSERT INTO topics (meeting_id, title, kind, notes, order_index) VALUES (?1, ?2, 'department', ?3, ?4)",
        params![meeting_id, title, department, order_index],
    )
    .map_err(|err| err.to_string())?;
    Ok(conn.last_insert_rowid())
}

fn map_tree_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<TreeDocument> {
    Ok(TreeDocument {
        document_id: row.get(0)?,
        display_name: row.get(1)?,
        filename: row.get(2)?,
        ext: row.get(3)?,
        size: row.get(4)?,
        role: row.get(5)?,
        attachment_label: row.get(6)?,
        folder_hint: row.get(7)?,
        order_index: row.get(8)?,
    })
}

pub fn meeting_tree(conn: &Connection, meeting_id: i64) -> Result<MeetingTree, String> {
    let (title, meeting_date): (String, Option<String>) = conn
        .query_row(
            "SELECT title, meeting_date FROM meetings WHERE id = ?1",
            [meeting_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| format!("找不到会议 {meeting_id}"))?;

    let mut stmt = conn
        .prepare(
            "SELECT td.document_id, d.filename_display, d.filename_original, d.ext,
                    d.size, td.role, td.attachment_label, md.folder_hint, td.order_index,
                    td.topic_id
             FROM topic_documents td
             JOIN documents d ON d.id = td.document_id
             LEFT JOIN meeting_documents md
                    ON md.document_id = td.document_id AND md.meeting_id = td.meeting_id
             WHERE td.meeting_id = ?1
             ORDER BY td.order_index, td.id",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map([meeting_id], |row| {
            Ok((map_tree_document(row)?, row.get::<_, Option<i64>>(9)?))
        })
        .map_err(|err| err.to_string())?;

    let mut agenda = None;
    let mut per_topic: std::collections::HashMap<i64, Vec<TreeDocument>> = Default::default();
    let mut others = Vec::new();
    for row in rows {
        let (document, topic_id) = row.map_err(|err| err.to_string())?;
        match (document.role.as_str(), topic_id) {
            ("agenda", _) => agenda = Some(document),
            ("other", _) => others.push(document),
            (_, Some(topic_id)) => per_topic.entry(topic_id).or_default().push(document),
            _ => others.push(document),
        }
    }

    let mut topics = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, notes, order_index FROM topics
                 WHERE meeting_id = ?1 ORDER BY order_index, id",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([meeting_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|err| err.to_string())?;

        for row in rows {
            let (topic_id, title, department, order_index) = row.map_err(|err| err.to_string())?;
            let items = per_topic.remove(&topic_id).unwrap_or_default();
            let main = items
                .iter()
                .find(|item| item.role == "main")
                .cloned();
            let mut attachments: Vec<TreeDocument> = items
                .into_iter()
                .filter(|item| item.role == "attachment")
                .collect();
            attachments.sort_by(|a, b| {
                a.order_index
                    .cmp(&b.order_index)
                    .then_with(|| a.display_name.cmp(&b.display_name))
            });
            topics.push(TopicNode {
                topic_id,
                title,
                department,
                order_index,
                main,
                attachments,
            });
        }
    }

    let mut unassigned = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT d.id, d.filename_display, d.filename_original, d.ext, md.folder_hint, d.size
                 FROM meeting_documents md
                 JOIN documents d ON d.id = md.document_id
                 WHERE md.meeting_id = ?1
                   AND NOT EXISTS (
                     SELECT 1 FROM topic_documents td
                     WHERE td.meeting_id = md.meeting_id AND td.document_id = md.document_id)
                 ORDER BY md.folder_hint, d.filename_display",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([meeting_id], |row| {
                Ok(TreeDocument {
                    document_id: row.get(0)?,
                    display_name: row.get(1)?,
                    filename: row.get(2)?,
                    ext: row.get(3)?,
                    size: row.get(5)?,
                    role: "unassigned".to_string(),
                    attachment_label: None,
                    folder_hint: row.get(4)?,
                    order_index: 0,
                })
            })
            .map_err(|err| err.to_string())?;
        for row in rows {
            unassigned.push(row.map_err(|err| err.to_string())?);
        }
    }

    let total_documents: i64 = conn
        .query_row(
            "SELECT count(*) FROM meeting_documents WHERE meeting_id = ?1",
            [meeting_id],
            |row| row.get(0),
        )
        .map_err(|err| err.to_string())?;

    Ok(MeetingTree {
        meeting_id,
        title,
        meeting_date,
        agenda,
        topics,
        unassigned,
        others,
        total_documents,
    })
}

pub fn list_meetings(conn: &Connection) -> Result<Vec<MeetingSummary>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.title, m.meeting_date, m.created_at,
                    (SELECT count(*) FROM meeting_documents md WHERE md.meeting_id = m.id)
             FROM meetings m
             ORDER BY m.id DESC
             LIMIT 100",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(MeetingSummary {
                meeting_id: row.get(0)?,
                title: row.get(1)?,
                meeting_date: row.get(2)?,
                created_at: row.get(3)?,
                document_count: row.get(4)?,
            })
        })
        .map_err(|err| err.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|err| err.to_string())
}

// ── 删除会议 ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub meeting_title: String,
    pub documents_removed: usize,
    pub blobs_moved_to_trash: usize,
    pub bytes_freed: i64,
    pub trash_dir: Option<String>,
}

/// 删除一场会议。
///
/// 处理原则：
///   · 关系、议题、备注等数据库记录直接删除（可从备份恢复）
///   · 文件**不硬删**，移到 `library/trash/<时间>-<会议名>/`，误删了还能捞回来
///   · 如果某份材料还被别的会议引用，则只解除本会议的关系，不动文件
pub fn delete_meeting(
    conn: &Connection,
    library_root: &Path,
    meeting_id: i64,
) -> Result<DeleteResult, String> {
    let title: String = conn
        .query_row("SELECT title FROM meetings WHERE id = ?1", [meeting_id], |row| {
            row.get(0)
        })
        .map_err(|_| format!("找不到会议 {meeting_id}"))?;

    let documents: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT d.id, d.sha256 FROM meeting_documents md
                 JOIN documents d ON d.id = md.document_id
                 WHERE md.meeting_id = ?1",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([meeting_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|err| err.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|err| err.to_string())?
    };

    // 删会议：topics / topic_documents / meeting_documents / annotations / reading_state
    // 等都会按外键级联删除
    conn.execute("DELETE FROM meetings WHERE id = ?1", [meeting_id])
        .map_err(|err| err.to_string())?;

    let stamp: String = conn
        .query_row("SELECT strftime('%Y%m%d-%H%M%S','now','localtime')", [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|_| "unknown".to_string());
    let trash_dir = library_root
        .join("library")
        .join("trash")
        .join(format!("{stamp}-{}", sanitize_name(&title)));

    let mut documents_removed = 0usize;
    let mut blobs_moved_to_trash = 0usize;
    let mut bytes_freed = 0i64;
    let mut used_trash = false;

    for (document_id, sha256) in documents {
        // 还被别的会议引用？那就只解除本会议的关系
        let still_referenced: i64 = conn
            .query_row(
                "SELECT count(*) FROM meeting_documents WHERE document_id = ?1",
                [document_id],
                |row| row.get(0),
            )
            .map_err(|err| err.to_string())?;
        if still_referenced > 0 {
            continue;
        }

        let _ = crate::search::remove_document(conn, document_id);
        conn.execute("DELETE FROM documents WHERE id = ?1", [document_id])
            .map_err(|err| err.to_string())?;
        documents_removed += 1;

        let blob_in_use: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents WHERE sha256 = ?1",
                [&sha256],
                |row| row.get(0),
            )
            .map_err(|err| err.to_string())?;
        if blob_in_use > 0 {
            continue;
        }

        let rel: Option<String> = conn
            .query_row("SELECT rel_path FROM blobs WHERE sha256 = ?1", [&sha256], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|err| err.to_string())?;

        if let Some(rel) = rel {
            let source = library_root.join(&rel);
            if source.exists() {
                std::fs::create_dir_all(&trash_dir).map_err(|err| err.to_string())?;
                let file_name = source
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| sha256.clone());
                let target = trash_dir.join(file_name);
                bytes_freed += std::fs::metadata(&source).map(|meta| meta.len() as i64).unwrap_or(0);
                match std::fs::rename(&source, &target) {
                    Ok(()) => {}
                    Err(_) => {
                        // 跨卷等情况：退回复制 + 删除
                        std::fs::copy(&source, &target).map_err(|err| err.to_string())?;
                        std::fs::remove_file(&source).map_err(|err| err.to_string())?;
                    }
                }
                used_trash = true;
            }
            conn.execute("DELETE FROM blobs WHERE sha256 = ?1", [&sha256])
                .map_err(|err| err.to_string())?;
            blobs_moved_to_trash += 1;
        }
    }

    Ok(DeleteResult {
        meeting_title: title,
        documents_removed,
        blobs_moved_to_trash,
        bytes_freed,
        trash_dir: used_trash.then(|| trash_dir.display().to_string()),
    })
}

// ── 按目录结构生成议题草稿 ──────────────────────────────────────────
//
// 真实会议材料的组织方式（实测）：
//   人事人才部/3-审议关于××的申请/会议材料1-1：××.docx
//   人事人才部/5-审议关于××的汇报/会议材料3-2：档案专审材料/周某/××.pdf
// 即：部门文件夹 → 议题文件夹 → 材料（可能还有更深一层的附件分组）。
//
// 草稿只做"按目录分组"这一件确定性的事，不猜谁是主材料：
//   议题文件夹里的材料先全部挂为附件（按路径顺序编号），
//   用户在界面里点一下"设为主材料"，其余自动重编号为附件1..N。

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftResult {
    pub topics_created: usize,
    pub documents_grouped: usize,
    pub left_unassigned: usize,
    pub topics: Vec<DraftTopic>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftTopic {
    pub topic_id: i64,
    pub title: String,
    pub department: Option<String>,
    pub document_count: usize,
}

/// 拆出议题文件夹名里的序号：`3-审议关于××` → (Some(3), "审议关于××")
fn split_leading_number(name: &str) -> (Option<i64>, String) {
    let trimmed = name.trim();
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return (None, trimmed.to_string());
    }
    let rest = trimmed[digits.len()..]
        .trim_start_matches(['-', '－', '.', '、', ' ', '\u{3000}']);
    if rest.is_empty() {
        return (None, trimmed.to_string());
    }
    (digits.parse().ok(), rest.to_string())
}

pub fn auto_draft_topics(conn: &Connection, meeting_id: i64) -> Result<DraftResult, String> {
    // 只处理"还没归类"的材料，绝不覆盖人工已经安排好的结构
    let pending: Vec<(i64, Option<String>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT d.id, md.folder_hint
                 FROM meeting_documents md
                 JOIN documents d ON d.id = md.document_id
                 WHERE md.meeting_id = ?1
                   AND NOT EXISTS (
                     SELECT 1 FROM topic_documents td
                     WHERE td.meeting_id = md.meeting_id AND td.document_id = md.document_id)
                 ORDER BY md.folder_hint, d.filename_display",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([meeting_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(|err| err.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|err| err.to_string())?
    };

    // 按 (部门, 议题文件夹) 归组；目录层级不够的文件保持未归类
    let mut groups: Vec<(Option<String>, String, Vec<i64>)> = Vec::new();
    let mut left_unassigned = 0usize;

    for (document_id, folder_hint) in pending {
        let segments: Vec<&str> = folder_hint
            .as_deref()
            .unwrap_or("")
            .split('/')
            .filter(|part| !part.trim().is_empty())
            .collect();

        // 至少要有"部门/议题"两层，才认为它属于某个议题
        if segments.len() < 2 {
            left_unassigned += 1;
            continue;
        }

        let department = sanitize_name(segments[0]);
        let topic_folder = sanitize_name(segments[1]);

        if let Some(existing) = groups
            .iter_mut()
            .find(|(dept, title, _)| {
                dept.as_deref() == Some(department.as_str()) && title == &topic_folder
            })
        {
            existing.2.push(document_id);
        } else {
            groups.push((Some(department), topic_folder, vec![document_id]));
        }
    }

    let mut created = Vec::new();
    let mut documents_grouped = 0usize;

    for (department, folder_name, document_ids) in groups {
        let (number, title) = split_leading_number(&folder_name);
        let order_index = match number {
            // 文件夹自带序号（2-、3-…）直接对建议题顺序，与议程一致
            Some(value) => value * 1000,
            None => {
                let max: i64 = conn
                    .query_row(
                        "SELECT ifnull(max(order_index), 0) FROM topics WHERE meeting_id = ?1",
                        [meeting_id],
                        |row| row.get(0),
                    )
                    .map_err(|err| err.to_string())?;
                max + 1000
            }
        };

        // 同名议题已存在就复用，避免重复点按钮时建出一堆空议题
        let existing_topic: Option<i64> = conn
            .query_row(
                "SELECT id FROM topics WHERE meeting_id = ?1 AND title = ?2",
                params![meeting_id, title],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| err.to_string())?;

        let topic_id = match existing_topic {
            Some(id) => id,
            None => {
                conn.execute(
                    "INSERT INTO topics (meeting_id, title, kind, notes, order_index)
                     VALUES (?1, ?2, 'department', ?3, ?4)",
                    params![meeting_id, title, department, order_index],
                )
                .map_err(|err| err.to_string())?;
                conn.last_insert_rowid()
            }
        };

        let mut order: i64 = 0;
        let mut label_index = count_attachments(conn, meeting_id, topic_id)?;
        for document_id in &document_ids {
            order += 1000;
            label_index += 1;
            conn.execute(
                "INSERT OR REPLACE INTO topic_documents
                   (meeting_id, topic_id, document_id, role, order_index, attachment_label, origin)
                 VALUES (?1, ?2, ?3, 'attachment', ?4, ?5, 'auto_rule')",
                params![
                    meeting_id,
                    topic_id,
                    document_id,
                    order,
                    format!("附件{label_index}")
                ],
            )
            .map_err(|err| err.to_string())?;
        }
        documents_grouped += document_ids.len();
        created.push(DraftTopic {
            topic_id,
            title,
            department,
            document_count: document_ids.len(),
        });
    }

    Ok(DraftResult {
        topics_created: created.len(),
        documents_grouped,
        left_unassigned,
        topics: created,
    })
}

fn count_attachments(conn: &Connection, meeting_id: i64, topic_id: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT count(*) FROM topic_documents
         WHERE meeting_id = ?1 AND topic_id = ?2 AND role = 'attachment'",
        params![meeting_id, topic_id],
        |row| row.get(0),
    )
    .map_err(|err| err.to_string())
}

/// 把某个议题里的一份材料设为主材料，其余材料自动重编号为附件1..N。
pub fn set_topic_main(
    conn: &Connection,
    meeting_id: i64,
    topic_id: i64,
    document_id: i64,
) -> Result<(), String> {
    let belongs: i64 = conn
        .query_row(
            "SELECT count(*) FROM topic_documents
             WHERE meeting_id = ?1 AND topic_id = ?2 AND document_id = ?3",
            params![meeting_id, topic_id, document_id],
            |row| row.get(0),
        )
        .map_err(|err| err.to_string())?;
    if belongs == 0 {
        return Err("这份材料不属于该议题".to_string());
    }

    // 原主材料降为附件
    conn.execute(
        "UPDATE topic_documents SET role = 'attachment'
         WHERE meeting_id = ?1 AND topic_id = ?2 AND role = 'main'",
        params![meeting_id, topic_id],
    )
    .map_err(|err| err.to_string())?;

    conn.execute(
        "UPDATE topic_documents SET role = 'main', attachment_label = NULL, origin = 'manual'
         WHERE meeting_id = ?1 AND topic_id = ?2 AND document_id = ?3",
        params![meeting_id, topic_id, document_id],
    )
    .map_err(|err| err.to_string())?;

    // 其余附件按当前顺序重新编号
    let attachments: Vec<i64> = {
        let mut stmt = conn
            .prepare(
                "SELECT document_id FROM topic_documents
                 WHERE meeting_id = ?1 AND topic_id = ?2 AND role = 'attachment'
                 ORDER BY order_index, id",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map(params![meeting_id, topic_id], |row| row.get(0))
            .map_err(|err| err.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|err| err.to_string())?
    };

    for (index, id) in attachments.iter().enumerate() {
        conn.execute(
            "UPDATE topic_documents SET attachment_label = ?1
             WHERE meeting_id = ?2 AND topic_id = ?3 AND document_id = ?4",
            params![format!("附件{}", index + 1), meeting_id, topic_id, id],
        )
        .map_err(|err| err.to_string())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;
    use std::fs;

    /// 每个用例用独立目录：测试是并行跑的，共用目录会互相删除文件
    fn 建临时素材(tag: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("md-organize-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let source = root.join("20260929-党委会会议材料");
        fs::create_dir_all(source.join("人事人才部")).unwrap();
        fs::write(
            source.join("党委扩大会议题第17次2026.09.29.docx"),
            "议程内容".as_bytes(),
        )
        .unwrap();
        fs::write(
            source
                .join("人事人才部")
                .join("会议材料1-1：关于兼职情况的请示.docx"),
            "主材料".as_bytes(),
        )
        .unwrap();
        fs::write(
            source.join("人事人才部").join("附件1：实施方案.docx"),
            "附件一".as_bytes(),
        )
        .unwrap();
        fs::write(
            source.join("人事人才部").join("附件2：人员名单.pdf"),
            "附件二".as_bytes(),
        )
        .unwrap();
        fs::write(source.join(".DS_Store"), "junk".as_bytes()).unwrap();
        (root, source)
    }

    #[test]
    fn 导入文件夹会跳过系统文件并保留目录线索() {
        let (root, source) = 建临时素材("import");
        let conn = crate::db::open_in_memory().unwrap();
        let result = import_paths(&conn, &root, &[source.to_string_lossy().to_string()]).unwrap();

        assert_eq!(result.files.len(), 4, "应导入 4 份材料（跳过 .DS_Store）");
        assert_eq!(result.meeting_title, "20260929-党委会会议材料");
        assert_eq!(result.meeting_date.as_deref(), Some("2026-09-29"));
        let grouped = result
            .files
            .iter()
            .filter(|file| file.folder_hint.as_deref() == Some("人事人才部"))
            .count();
        assert_eq!(grouped, 3, "人事人才部 下应有 3 份");
        assert_eq!(result.duplicate_count, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn 同一份文件再次导入不会产生第二份字节() {
        let (root, source) = 建临时素材("dedupe");
        let conn = crate::db::open_in_memory().unwrap();
        let path = source.to_string_lossy().to_string();
        import_paths(&conn, &root, &[path.clone()]).unwrap();

        // 再导入两次
        import_paths(&conn, &root, &[path.clone()]).unwrap();
        import_paths(&conn, &root, &[path]).unwrap();

        let blobs: i64 = conn.query_row("SELECT count(*) FROM blobs", [], |row| row.get(0)).unwrap();
        let documents: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |row| row.get(0))
            .unwrap();
        assert_eq!(blobs, 4, "文件内容只有 4 份，不应重复存放");
        assert_eq!(documents, 4, "材料记录也不应重复");
        let meetings: i64 = conn
            .query_row("SELECT count(*) FROM meetings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(meetings, 3, "每次导入是一场独立会议");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn 指定角色后能组装出会议结构() {
        let (root, source) = 建临时素材("assign");
        let conn = crate::db::open_in_memory().unwrap();
        let result = import_paths(&conn, &root, &[source.to_string_lossy().to_string()]).unwrap();
        let meeting_id = result.meeting_id;

        let by_name = |needle: &str| -> i64 {
            result
                .files
                .iter()
                .find(|file| file.filename.contains(needle))
                .unwrap_or_else(|| panic!("找不到包含 {needle} 的文件"))
                .document_id
        };

        // 选议程
        assign_document(&conn, meeting_id, by_name("党委扩大会议题"), "agenda", None, None).unwrap();

        // 建议题并指定主材料
        let topic_id = create_topic(&conn, meeting_id, "关于兼职情况的请示", Some("人事人才部")).unwrap();
        assign_document(
            &conn,
            meeting_id,
            by_name("兼职情况的请示"),
            "main",
            Some(topic_id),
            None,
        )
        .unwrap();

        // 两个附件：一个自动编号，一个手动指定
        assign_document(&conn, meeting_id, by_name("实施方案"), "attachment", Some(topic_id), None)
            .unwrap();
        assign_document(
            &conn,
            meeting_id,
            by_name("人员名单"),
            "attachment",
            Some(topic_id),
            Some("附件二".to_string()),
        )
        .unwrap();

        let tree = meeting_tree(&conn, meeting_id).unwrap();
        assert_eq!(tree.agenda.as_ref().unwrap().filename.contains("党委扩大会议题"), true);
        assert_eq!(tree.topics.len(), 1);
        assert_eq!(tree.topics[0].title, "关于兼职情况的请示");
        assert_eq!(tree.topics[0].department.as_deref(), Some("人事人才部"));
        assert!(tree.topics[0].main.is_some(), "应有主材料");
        assert_eq!(tree.topics[0].attachments.len(), 2);

        let labels: Vec<String> = tree.topics[0]
            .attachments
            .iter()
            .filter_map(|item| item.attachment_label.clone())
            .collect();
        assert!(labels.contains(&"附件1".to_string()), "自动编号应为 附件1：{labels:?}");
        assert!(labels.contains(&"附件二".to_string()), "手动编号应保留：{labels:?}");
        assert!(tree.unassigned.is_empty(), "4 份材料都已归类");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn 一份材料可以改判角色而不会重复挂载() {
        let (root, source) = 建临时素材("reassign");
        let conn = crate::db::open_in_memory().unwrap();
        let result = import_paths(&conn, &root, &[source.to_string_lossy().to_string()]).unwrap();
        let meeting_id = result.meeting_id;
        let topic_id = create_topic(&conn, meeting_id, "议题A", None).unwrap();
        let document_id = result.files[0].document_id;

        assign_document(&conn, meeting_id, document_id, "main", Some(topic_id), None).unwrap();
        assign_document(&conn, meeting_id, document_id, "attachment", Some(topic_id), None).unwrap();

        let rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM topic_documents WHERE meeting_id = ?1 AND document_id = ?2",
                params![meeting_id, document_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1, "一份材料在同一会议里只应有一条角色关系");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn 文件名清洗与日期猜测() {
        assert_eq!(sanitize_name("关于\n兼职情况"), "关于 兼职情况");
        assert_eq!(sanitize_name("  a   b  "), "a b");
        assert_eq!(guess_meeting_date("20260929-党委会会议材料").as_deref(), Some("2026-09-29"));
        assert_eq!(guess_meeting_date("第三季度读书班"), None);
        assert_eq!(guess_meeting_date("2026.09.29 专题会").as_deref(), Some("2026-09-29"));
    }

    /// 构造一个"部门 / 议题文件夹 / 材料"的目录结构（与真实材料一致）
    fn 建议题式素材(tag: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("md-draft-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let source = root.join("20260929-党委会会议材料");
        let dept = source.join("人事人才部");
        let topic_a = dept.join("3-审议关于张某兼职补报批的申请");
        let topic_b = dept.join("5-审议关于档案专审情况的汇报");
        let nested = topic_b.join("会议材料3-2：档案专审材料").join("周某");

        fs::create_dir_all(&topic_a).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(source.join("党委扩大会议题第17次2026.09.29.docx"), "议程".as_bytes()).unwrap();
        fs::write(dept.join("打印表单.pdf"), "部门直属".as_bytes()).unwrap();
        fs::write(topic_a.join("会议材料1-1：兼职情况.docx"), "主材料A".as_bytes()).unwrap();
        fs::write(topic_a.join("会议材料1-2：个人申请.pdf"), "附件A2".as_bytes()).unwrap();
        fs::write(topic_a.join("会议材料1-3：征求意见函.pdf"), "附件A3".as_bytes()).unwrap();
        fs::write(topic_b.join("会议材料3-1：汇报.docx"), "主材料B".as_bytes()).unwrap();
        fs::write(nested.join("参保证明.pdf"), "深层附件".as_bytes()).unwrap();
        (root, source)
    }

    #[test]
    fn 按目录生成议题草稿并保留部门与序号() {
        let (root, source) = 建议题式素材("draft");
        let conn = crate::db::open_in_memory().unwrap();
        let imported =
            import_paths(&conn, &root, &[source.to_string_lossy().to_string()]).unwrap();
        let meeting_id = imported.meeting_id;

        let draft = auto_draft_topics(&conn, meeting_id).unwrap();
        assert_eq!(draft.topics_created, 2, "应生成两个议题");
        assert_eq!(draft.documents_grouped, 5, "议题文件夹里的 5 份材料应被归入");
        assert_eq!(draft.left_unassigned, 2, "根目录议程 + 部门直属文件应留待手动指定");

        let tree = meeting_tree(&conn, meeting_id).unwrap();
        assert_eq!(tree.topics.len(), 2);

        // 文件夹自带序号 3-/5- 变成了议题顺序，标题里的序号被去掉
        let first = &tree.topics[0];
        assert_eq!(first.title, "审议关于张某兼职补报批的申请");
        assert_eq!(first.department.as_deref(), Some("人事人才部"));
        assert_eq!(first.order_index, 3000);
        assert_eq!(tree.topics[1].order_index, 5000);

        // 草稿阶段全部先挂为附件，等待用户指定主材料
        assert!(first.main.is_none());
        assert_eq!(first.attachments.len(), 3);
        let labels: Vec<String> = first
            .attachments
            .iter()
            .filter_map(|item| item.attachment_label.clone())
            .collect();
        assert_eq!(labels, vec!["附件1", "附件2", "附件3"]);

        // 深层子目录里的材料也归到了它所属的议题
        assert_eq!(tree.topics[1].attachments.len(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn 指定主材料后其余附件自动重编号() {
        let (root, source) = 建议题式素材("main");
        let conn = crate::db::open_in_memory().unwrap();
        let imported =
            import_paths(&conn, &root, &[source.to_string_lossy().to_string()]).unwrap();
        let meeting_id = imported.meeting_id;
        auto_draft_topics(&conn, meeting_id).unwrap();

        let tree = meeting_tree(&conn, meeting_id).unwrap();
        let topic = &tree.topics[0];
        // 把第三份材料提为主材料
        let target = topic.attachments[2].document_id;
        set_topic_main(&conn, meeting_id, topic.topic_id, target).unwrap();

        let after = meeting_tree(&conn, meeting_id).unwrap();
        let topic = &after.topics[0];
        assert_eq!(topic.main.as_ref().unwrap().document_id, target);
        assert_eq!(topic.attachments.len(), 2, "其余材料仍是附件");
        let labels: Vec<String> = topic
            .attachments
            .iter()
            .filter_map(|item| item.attachment_label.clone())
            .collect();
        assert_eq!(labels, vec!["附件1", "附件2"], "编号应重新连续");

        // 再换一个主材料，原来的主材料应退回成附件并重新编号
        let other = topic.attachments[0].document_id;
        set_topic_main(&conn, meeting_id, topic.topic_id, other).unwrap();
        let after2 = meeting_tree(&conn, meeting_id).unwrap();
        let topic = &after2.topics[0];
        assert_eq!(topic.main.as_ref().unwrap().document_id, other);
        assert_eq!(topic.attachments.len(), 2);
        assert!(topic.attachments.iter().any(|item| item.document_id == target));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn 重复生成草稿不会重复建议题也不会动人工结果() {
        let (root, source) = 建议题式素材("twice");
        let conn = crate::db::open_in_memory().unwrap();
        let imported =
            import_paths(&conn, &root, &[source.to_string_lossy().to_string()]).unwrap();
        let meeting_id = imported.meeting_id;

        auto_draft_topics(&conn, meeting_id).unwrap();
        let second = auto_draft_topics(&conn, meeting_id).unwrap();
        assert_eq!(second.topics_created, 0, "第二次不应再建议题");
        assert_eq!(second.documents_grouped, 0, "已归类的材料不重复处理");

        let topics: i64 = conn
            .query_row(
                "SELECT count(*) FROM topics WHERE meeting_id = ?1",
                [meeting_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(topics, 2);

        // 人工指定的主材料在再次生成草稿后必须保持不变
        let tree = meeting_tree(&conn, meeting_id).unwrap();
        let topic_id = tree.topics[0].topic_id;
        let document_id = tree.topics[0].attachments[0].document_id;
        set_topic_main(&conn, meeting_id, topic_id, document_id).unwrap();
        auto_draft_topics(&conn, meeting_id).unwrap();

        let after = meeting_tree(&conn, meeting_id).unwrap();
        let topic = after
            .topics
            .iter()
            .find(|item| item.topic_id == topic_id)
            .unwrap();
        assert_eq!(
            topic.main.as_ref().unwrap().document_id,
            document_id,
            "人工结果不能被自动草稿覆盖"
        );
        let _ = fs::remove_dir_all(root);
    }

    fn 统计文件数(dir: &Path) -> usize {
        let mut total = 0;
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    total += 1;
                }
            }
        }
        total
    }

    #[test]
    fn 删除会议时会保留仍被其他会议共用的文件() {
        let (root, source) = 建临时素材("delete");
        let conn = crate::db::open_in_memory().unwrap();
        let path = source.to_string_lossy().to_string();
        let first = import_paths(&conn, &root, &[path.clone()]).unwrap();
        let second = import_paths(&conn, &root, &[path]).unwrap();
        let blobs_dir = root.join("library").join("blobs");
        assert_eq!(统计文件数(&blobs_dir), 4);

        // 第一场删掉：材料还被第二场用着，文件和记录都必须留着
        let result = delete_meeting(&conn, &root, first.meeting_id).unwrap();
        assert_eq!(result.documents_removed, 0, "被别人引用的材料不能删");
        assert_eq!(result.blobs_moved_to_trash, 0);
        assert!(result.trash_dir.is_none());
        assert_eq!(统计文件数(&blobs_dir), 4, "文件应原样保留");
        let meetings: i64 = conn
            .query_row("SELECT count(*) FROM meetings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(meetings, 1);
        let tree = meeting_tree(&conn, second.meeting_id).unwrap();
        assert_eq!(tree.total_documents, 4, "第二场会议材料应完好");

        // 第二场也删掉：文档与文件都该清干净，文件进回收区
        let result = delete_meeting(&conn, &root, second.meeting_id).unwrap();
        assert_eq!(result.documents_removed, 4);
        assert_eq!(result.blobs_moved_to_trash, 4);
        assert_eq!(统计文件数(&blobs_dir), 0);
        let trash = result.trash_dir.expect("应生成回收目录");
        assert_eq!(统计文件数(Path::new(&trash)), 4, "文件应在回收区里等着被找回");

        let documents: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |row| row.get(0))
            .unwrap();
        let blobs: i64 = conn
            .query_row("SELECT count(*) FROM blobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(documents, 0);
        assert_eq!(blobs, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn 删除空会议不会出错() {
        let (root, _source) = 建临时素材("emptydel");
        let conn = crate::db::open_in_memory().unwrap();
        conn.execute("INSERT INTO meetings (title) VALUES ('未命名会议')", [])
            .unwrap();
        let id = conn.last_insert_rowid();

        let result = delete_meeting(&conn, &root, id).unwrap();
        assert_eq!(result.meeting_title, "未命名会议");
        assert_eq!(result.documents_removed, 0);
        assert_eq!(result.blobs_moved_to_trash, 0);
        assert!(result.trash_dir.is_none());
        let _ = fs::remove_dir_all(root);
    }

    /// 用真实会议材料跑一遍完整导入（默认跳过，避免把真实材料拖进测试）。
    /// 跑法：MD_REAL_DIR="<会议文件夹>" cargo test 真实材料冒烟测试 -- --ignored --nocapture
    #[test]
    #[ignore]
    fn 真实材料冒烟测试() {
        let Ok(source) = std::env::var("MD_REAL_DIR") else {
            eprintln!("未设置 MD_REAL_DIR，跳过");
            return;
        };
        let root = std::env::temp_dir().join(format!("md-real-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let conn = crate::db::open_in_memory().unwrap();
        let result = import_paths(&conn, &root, &[source.clone()]).unwrap();

        println!("会议：{}（{}）", result.meeting_title, result.meeting_date.unwrap_or_default());
        println!("导入 {} 份材料，重复 {} 份", result.files.len(), result.duplicate_count);
        for file in &result.files {
            println!(
                "  [{}] {}（{}，{} 字节）{}",
                file.folder_hint.clone().unwrap_or_else(|| "根目录".into()),
                file.filename,
                file.ext,
                file.size,
                if file.is_duplicate { " ← 库里已有" } else { "" }
            );
        }

        // 模拟人工归类：第一份设为议程，其余依次建立议题
        let meeting_id = result.meeting_id;
        let first = result.files[0].document_id;
        assign_document(&conn, meeting_id, first, "agenda", None, None).unwrap();
        let topic_id = create_topic(&conn, meeting_id, "示例议题", Some("人事人才部")).unwrap();
        let main_id = result.files[1].document_id;
        assign_document(&conn, meeting_id, main_id, "main", Some(topic_id), None).unwrap();
        for file in result.files.iter().skip(2).take(3) {
            assign_document(&conn, meeting_id, file.document_id, "attachment", Some(topic_id), None)
                .unwrap();
        }

        let tree = meeting_tree(&conn, meeting_id).unwrap();
        println!(
            "结构：议程 {}，议题 {} 个（主材料 {}，附件 {}），未归类 {}",
            tree.agenda.is_some(),
            tree.topics.len(),
            tree.topics[0].main.is_some(),
            tree.topics[0].attachments.len(),
            tree.unassigned.len()
        );
        assert_eq!(tree.topics[0].attachments.len(), 3);
        assert_eq!(
            tree.unassigned.len(),
            result.files.len() - 5,
            "除议程/主材料/3 个附件外，其余都应还在未归类里"
        );

        let blobs: i64 = conn.query_row("SELECT count(*) FROM blobs", [], |row| row.get(0)).unwrap();
        assert_eq!(blobs as usize, result.files.len(), "文件字节应一份一存");

        // 再导入一次：不应新增任何字节
        let again = import_paths(&conn, &root, &[source]).unwrap();
        assert_eq!(again.duplicate_count, again.files.len(), "第二次导入应全部命中已有内容");
        let blobs_after: i64 =
            conn.query_row("SELECT count(*) FROM blobs", [], |row| row.get(0)).unwrap();
        assert_eq!(blobs_after, blobs, "重复导入不应新增文件");
        println!("重复导入验证通过：blobs 仍为 {blobs_after} 份");

        let _ = fs::remove_dir_all(root);
    }

    /// 在真实数据库的副本上跑"按目录生成草稿"，验证真实材料结构能否被正确识别。
    /// 跑法：MD_REAL_DB="<库文件路径>" cargo test 真实数据库草稿 -- --ignored --nocapture
    #[test]
    #[ignore]
    fn 真实数据库草稿测试() {
        let Ok(db_path) = std::env::var("MD_REAL_DB") else {
            eprintln!("未设置 MD_REAL_DB，跳过");
            return;
        };
        let root = std::env::temp_dir().join(format!("md-realdb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let copy = root.join("copy.sqlite");
        fs::copy(&db_path, &copy).unwrap();
        for suffix in ["-wal", "-shm"] {
            let side = format!("{db_path}{suffix}");
            if std::path::Path::new(&side).exists() {
                fs::copy(&side, format!("{}{suffix}", copy.display())).unwrap();
            }
        }

        let conn = crate::db::open(&copy).unwrap();
        let meetings = list_meetings(&conn).unwrap();
        let target = meetings
            .iter()
            .max_by_key(|meeting| meeting.document_count)
            .expect("数据库里没有会议");

        let before = meeting_tree(&conn, target.meeting_id).unwrap();
        println!(
            "目标会议：{}（id={}）材料 {} 份",
            before.title, target.meeting_id, before.total_documents
        );
        println!(
            "生成前：议题 {} 个，议程 {}，未归类 {} 份",
            before.topics.len(),
            if before.agenda.is_some() { "有" } else { "无" },
            before.unassigned.len()
        );

        let draft = auto_draft_topics(&conn, target.meeting_id).unwrap();
        println!(
            "生成草稿：新建议题 {} 个，归入材料 {} 份，留待手动 {} 份",
            draft.topics_created, draft.documents_grouped, draft.left_unassigned
        );

        let after = meeting_tree(&conn, target.meeting_id).unwrap();
        for (index, topic) in after.topics.iter().enumerate() {
            println!(
                "  {:02} {}　[{}]　主材料 {}，附件 {}",
                index + 1,
                topic.title,
                topic.department.clone().unwrap_or_else(|| "无部门".into()),
                if topic.main.is_some() { "有" } else { "待指定" },
                topic.attachments.len()
            );
        }
        println!("仍未归类：{} 份", after.unassigned.len());
        for item in after.unassigned.iter().take(8) {
            println!(
                "   · [{}] {}",
                item.folder_hint.clone().unwrap_or_else(|| "根目录".into()),
                item.filename
            );
        }

        assert!(draft.topics_created > 0, "真实材料应能生成议题草稿");
        let _ = fs::remove_dir_all(root);
    }

    /// 清理真实库里"没有任何材料"的会议（用户明确要求）。
    /// 注意：这个用例会**直接修改真实数据库**。
    /// 跑法：MD_REAL_DB="<库文件>" MD_REAL_LIBRARY="<库根目录>" \
    ///       cargo test 清理空会议 -- --ignored --nocapture
    #[test]
    #[ignore]
    fn 清理空会议() {
        let (Ok(db_path), Ok(library_root)) =
            (std::env::var("MD_REAL_DB"), std::env::var("MD_REAL_LIBRARY"))
        else {
            eprintln!("未设置 MD_REAL_DB / MD_REAL_LIBRARY，跳过");
            return;
        };

        let conn = crate::db::open(Path::new(&db_path)).unwrap();
        let empties: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT m.id, m.title FROM meetings m
                     WHERE NOT EXISTS (
                       SELECT 1 FROM meeting_documents md WHERE md.meeting_id = m.id)
                     ORDER BY m.id",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap();
            rows.filter_map(Result::ok).collect()
        };

        println!("发现 {} 场没有材料的会议", empties.len());
        for (id, title) in &empties {
            println!("  · id={id} 《{title}》");
        }
        for (id, _) in &empties {
            let result = delete_meeting(&conn, Path::new(&library_root), *id).unwrap();
            println!(
                "  已删除《{}》：材料 {} 份，回收文件 {} 个",
                result.meeting_title, result.documents_removed, result.blobs_moved_to_trash
            );
        }

        println!("清理后剩余会议：");
        for meeting in list_meetings(&conn).unwrap() {
            println!(
                "  id={} {}（{} 份材料）",
                meeting.meeting_id, meeting.title, meeting.document_count
            );
        }
    }
}
