//! 导出「会议包」：把一场会议的全部材料打包成**一个 HTML 文件**。
//!
//! 用途（用户场景）：开会时带一个 U 盘，里面放这个 HTML，插到会议室电脑上
//! 双击打开、按 F 全屏投到电视上阅读；不需要装软件，也不需要联网。
//!
//! 为什么所有东西都要内嵌：
//! `file://` 协议下浏览器会阻止 `fetch`/ES module/Worker，
//! 外部文件一旦漏拷就整包失效——所以模板、脚本、图片、文档全部塞进同一个文件。

use std::path::{Path, PathBuf};

use base64::Engine;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};

use crate::ocr;
use crate::organize::{self, MeetingTree, TreeDocument};

const TEMPLATE: &str = include_str!("template/index.html");
const APP_CSS: &str = include_str!("template/app.css");
const APP_JS: &str = include_str!("template/app.js");
const VENDOR_JSZIP: &str = include_str!("../../vendor/jszip.min.js");
const VENDOR_DOCX_PREVIEW: &str = include_str!("../../vendor/docx-preview.min.js");

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub out_path: String,
    pub bytes: u64,
    pub document_count: usize,
    pub pdf_pages: usize,
    pub warnings: Vec<String>,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn mime_for(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

/// 取材料在本机库里的实际文件路径。
fn blob_path_for(conn: &Connection, document_id: i64) -> Result<PathBuf, String> {
    let rel: String = conn
        .query_row(
            "SELECT b.rel_path FROM documents d
             JOIN blobs b ON b.sha256 = d.sha256
             WHERE d.id = ?1",
            [document_id],
            |row| row.get(0),
        )
        .map_err(|err| format!("找不到材料 {document_id} 的文件：{err}"))?;
    Ok(rel.into())
}

/// 把一份材料转成内嵌 JSON（图片/文档数据都在里面）
fn embed_document(
    conn: &Connection,
    library_root: &Path,
    doc: &TreeDocument,
    render_dir: &Path,
    warnings: &mut Vec<String>,
    pdf_pages: &mut usize,
) -> Result<Value, String> {
    let rel = blob_path_for(conn, doc.document_id)?;
    let full = library_root.join(&rel);
    if !full.exists() {
        warnings.push(format!("找不到文件，已跳过：{}", doc.filename));
        return Ok(json!(null));
    }

    let ext = doc.ext.to_lowercase();
    let mut item = json!({
        "id": doc.document_id.to_string(),
        "title": doc.display_name,
        "filename": doc.filename,
        "ext": ext,
        "role": doc.role,
        "label": doc.attachment_label,
    });

    match ext.as_str() {
        "docx" => {
            let bytes = std::fs::read(&full).map_err(|err| err.to_string())?;
            item["kind"] = json!("docx");
            item["data"] = json!(b64(&bytes));
            // 纯文本兜底：docx 是 zip，直接抽 XML 里的文本够用了
            item["text"] = json!(extract_docx_text(&bytes));
        }
        "pdf" => {
            let out = render_dir.join(format!("doc-{}", doc.document_id));
            match ocr::render_pdf_pages(&full, &out) {
                Ok(files) => {
                    let mut pages = Vec::with_capacity(files.len());
                    for file in &files {
                        let bytes = std::fs::read(file).map_err(|err| err.to_string())?;
                        pages.push(json!(format!("data:image/jpeg;base64,{}", b64(&bytes))));
                    }
                    *pdf_pages += pages.len();
                    item["kind"] = json!("pdf");
                    item["pages"] = json!(pages);
                }
                Err(err) => {
                    warnings.push(format!("{} 渲染失败：{err}", doc.filename));
                    item["kind"] = json!("other");
                    item["data"] = json!(b64(&std::fs::read(&full).map_err(|e| e.to_string())?));
                }
            }
        }
        other => {
            let bytes = std::fs::read(&full).map_err(|err| err.to_string())?;
            match mime_for(other) {
                Some(mime) => {
                    item["kind"] = json!("image");
                    item["dataUri"] = json!(format!("data:{mime};base64,{}", b64(&bytes)));
                }
                None if other == "txt" || other == "md" || other == "csv" => {
                    item["kind"] = json!("text");
                    item["text"] = json!(String::from_utf8_lossy(&bytes).to_string());
                    item["data"] = json!(b64(&bytes));
                }
                None => {
                    item["kind"] = json!("other");
                    item["data"] = json!(b64(&bytes));
                }
            }
        }
    }

    Ok(item)
}

/// 从 docx（zip）里粗取正文文字，用于搜索与渲染失败时的兜底显示。
fn extract_docx_text(bytes: &[u8]) -> String {
    // 只在 <w:t> 标签之间取文字即可，不追求解析完整 XML
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut rest = text.as_ref();
    while let Some(start) = rest.find("<w:t") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else { break };
        let content = &after[open_end + 1..];
        let Some(close) = content.find("</w:t>") else { break };
        out.push_str(&content[..close]);
        rest = &content[close..];
    }
    // zip 是压缩的，直接扫字符串通常拿不到正文；拿不到就返回空
    out.chars()
        .filter(|ch| !ch.is_control() || *ch == '\n')
        .take(20000)
        .collect()
}

fn document_group_name(index: usize, title: &str) -> String {
    format!("{:02} {}", index + 1, title)
}

/// 导出一场会议为单个 HTML 文件。
pub fn export_single_html(
    conn: &Connection,
    library_root: &Path,
    meeting_id: i64,
    out_dir: &Path,
) -> Result<ExportResult, String> {
    let tree: MeetingTree = organize::meeting_tree(conn, meeting_id)?;

    if let Some(parent) = out_dir.parent() {
        let _ = parent;
    }
    std::fs::create_dir_all(out_dir).map_err(|err| format!("创建导出目录失败：{err}"))?;

    let render_dir = std::env::temp_dir().join(format!(
        "meetingdesk-export-{}-{}",
        std::process::id(),
        meeting_id
    ));
    let _ = std::fs::remove_dir_all(&render_dir);
    std::fs::create_dir_all(&render_dir).map_err(|err| err.to_string())?;

    let mut warnings: Vec<String> = Vec::new();
    let mut pdf_pages = 0usize;
    let mut document_count = 0usize;

    // 目录顺序：议程 →（按部门名聚合，部门内按议题序号）→ 其他材料
    // 排序键：(分区序号, 部门名, 议题序号)
    let mut ordered: Vec<(i32, String, i64, Value)> = Vec::new();

    // 议程
    if let Some(agenda) = &tree.agenda {
        let item = embed_document(conn, library_root, agenda, &render_dir, &mut warnings, &mut pdf_pages)?;
        if !item.is_null() {
            document_count += 1;
            ordered.push((
                0,
                String::new(),
                0,
                json!({ "name": "会议议程", "department": null, "items": [item] }),
            ));
        }
    }

    // 议题：主材料在前，附件按编号在后
    for (index, topic) in tree.topics.iter().enumerate() {
        let mut items = Vec::new();
        if let Some(main) = &topic.main {
            let item =
                embed_document(conn, library_root, main, &render_dir, &mut warnings, &mut pdf_pages)?;
            if !item.is_null() {
                document_count += 1;
                items.push(item);
            }
        }
        for attachment in &topic.attachments {
            let item = embed_document(
                conn,
                library_root,
                attachment,
                &render_dir,
                &mut warnings,
                &mut pdf_pages,
            )?;
            if !item.is_null() {
                document_count += 1;
                items.push(item);
            }
        }
        if !items.is_empty() {
            // 部门名相同的议题会排在一起，目录里合并成一组
            ordered.push((
                1,
                topic.department.clone().unwrap_or_default(),
                topic.order_index,
                json!({
                    "name": document_group_name(index, &topic.title),
                    "department": topic.department,
                    "items": items,
                }),
            ));
        }
    }

    // 其他 / 尚未归类的材料
    let mut rest = Vec::new();
    for doc in tree.others.iter().chain(tree.unassigned.iter()) {
        let item = embed_document(conn, library_root, doc, &render_dir, &mut warnings, &mut pdf_pages)?;
        if !item.is_null() {
            document_count += 1;
            rest.push(item);
        }
    }
    if !rest.is_empty() {
        ordered.push((
            2,
            String::new(),
            i64::MAX,
            json!({ "name": "其他材料", "department": null, "items": rest }),
        ));
    }

    ordered.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let groups: Vec<Value> = ordered.into_iter().map(|(_, _, _, value)| value).collect();

    let data = json!({
        "meeting": tree.title,
        "date": tree.meeting_date,
        "generatedAt": now_string(conn),
        "groups": groups,
    });

    // 注意替换顺序：数据最后注入，避免材料标题里恰好含有占位符造成误替换
    let html = TEMPLATE
        .replace("<title>__TITLE__</title>", &format!("<title>{}</title>", escape_html(&tree.title)))
        .replace("/*__CSS__*/", APP_CSS)
        .replace("/*__JSZIP__*/", VENDOR_JSZIP)
        .replace("/*__DOCX_PREVIEW__*/", VENDOR_DOCX_PREVIEW)
        .replace("/*__APP__*/", APP_JS)
        .replace(
            "/*__DATA__*/",
            &serde_json::to_string(&data)
                .map_err(|err| err.to_string())?
                .replace("</", "<\\/"),
        );

    // 有日期就带上日期；没有日期就不要硬塞"无日期"三个字
    let file_name = match &tree.meeting_date {
        Some(date) => format!("会议包-{}-{date}.html", sanitize_file_name(&tree.title)),
        None => format!("会议包-{}.html", sanitize_file_name(&tree.title)),
    };
    let out_path = out_dir.join(file_name);
    std::fs::write(&out_path, html.as_bytes())
        .map_err(|err| format!("写入会议包失败：{err}"))?;
    let _ = std::fs::remove_dir_all(&render_dir);

    let bytes = std::fs::metadata(&out_path).map(|meta| meta.len()).unwrap_or(0);
    Ok(ExportResult {
        out_path: out_path.display().to_string(),
        bytes,
        document_count,
        pdf_pages,
        warnings,
    })
}

fn now_string(conn: &Connection) -> String {
    conn.query_row("SELECT datetime('now','localtime')", [], |row| row.get(0))
        .unwrap_or_default()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn sanitize_file_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;
    use std::fs;

    #[test]
    fn 导出的会议包是单个自包含文件() {
        let root = std::env::temp_dir().join(format!("md-bundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let source = root.join("20260929-测试会议");
        let topic = source.join("办公室").join("2-审议关于某事项的请示");
        fs::create_dir_all(&topic).unwrap();
        fs::write(source.join("会议议程.docx"), "议程".as_bytes()).unwrap();
        fs::write(topic.join("会议材料2-1：请示.docx"), "正文".as_bytes()).unwrap();
        fs::write(topic.join("会议材料2-2：附件.pdf"), b"%PDF-1.4 broken").unwrap();

        let conn = crate::db::open_in_memory().unwrap();
        let imported =
            organize::import_paths(&conn, &root, &[source.to_string_lossy().to_string()]).unwrap();
        organize::auto_draft_topics(&conn, imported.meeting_id).unwrap();

        let out_dir = root.join("导出");
        let result =
            export_single_html(&conn, &root, imported.meeting_id, &out_dir).unwrap();

        let html = fs::read_to_string(&result.out_path).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("window.MEETING_BUNDLE"));
        assert!(html.contains("MEETING_BUNDLE ="), "数据必须内嵌");
        // 关键：没有任何外部引用（file:// 下外部文件一漏拷就整包失效）
        assert!(!html.contains("<script src="), "不应引用外部脚本");
        assert!(!html.contains("<link rel=\"stylesheet\""), "样式必须内联");
        assert!(!html.contains("fetch("), "模板里不应出现 fetch");
        // 渲染引擎随包携带
        assert!(html.contains("JSZip"), "应内嵌 JSZip");
        assert!(html.contains("docx-preview"), "应内嵌 docx-preview");
        assert!(result.document_count >= 2);
        assert!(result.bytes > 50_000, "体积应包含内嵌的渲染引擎");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn 文件名清洗不会留下非法字符() {
        assert_eq!(sanitize_file_name("20260929-党委会会议材料"), "20260929-党委会会议材料");
        assert_eq!(sanitize_file_name("a/b:c*d?"), "a_b_c_d_");
    }

    /// 用真实会议数据导出单个 HTML（只读材料库，产物写到临时目录）。
    /// 跑法：
    ///   MD_REAL_DB="<库文件>" MD_REAL_LIBRARY="<库根目录>" \
    ///   cargo test 真实会议导出单个HTML -- --ignored --nocapture
    #[test]
    #[ignore]
    fn 真实会议导出单个HTML() {
        let (Ok(db_path), Ok(library_root)) =
            (std::env::var("MD_REAL_DB"), std::env::var("MD_REAL_LIBRARY"))
        else {
            eprintln!("未设置 MD_REAL_DB / MD_REAL_LIBRARY，跳过");
            return;
        };

        let work = std::env::temp_dir().join(format!("md-export-{}", std::process::id()));
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).unwrap();

        let copy = work.join("copy.sqlite");
        fs::copy(&db_path, &copy).unwrap();
        for suffix in ["-wal", "-shm"] {
            let side = format!("{db_path}{suffix}");
            if std::path::Path::new(&side).exists() {
                fs::copy(&side, format!("{}{suffix}", copy.display())).unwrap();
            }
        }

        let conn = crate::db::open(&copy).unwrap();
        let meetings = organize::list_meetings(&conn).unwrap();
        let target = meetings
            .iter()
            .max_by_key(|meeting| meeting.document_count)
            .expect("数据库里没有会议");

        // 先按目录生成草稿（真实材料里人工只指定过两份，其余靠草稿补齐）
        organize::auto_draft_topics(&conn, target.meeting_id).unwrap();

        let out_dir = work.join("导出");
        let started = std::time::Instant::now();
        let result = export_single_html(
            &conn,
            Path::new(&library_root),
            target.meeting_id,
            &out_dir,
        )
        .unwrap();
        let elapsed = started.elapsed();

        println!("会议：{}", target.title);
        println!(
            "导出完成：{} 份材料，{} 页 PDF，{:.1} MB，耗时 {:.1} 秒",
            result.document_count,
            result.pdf_pages,
            result.bytes as f64 / 1024.0 / 1024.0,
            elapsed.as_secs_f64()
        );
        println!("产物：{}", result.out_path);
        for warning in &result.warnings {
            println!("  提示：{warning}");
        }

        let html = fs::read_to_string(&result.out_path).unwrap();
        println!(
            "自包含检查：外部脚本 {} 个，外部样式 {} 个，fetch {} 处",
            html.matches("<script src=").count(),
            html.matches("<link rel=\"stylesheet\"").count(),
            html.matches("fetch(").count()
        );
        assert_eq!(html.matches("<script src=").count(), 0);
        assert!(result.document_count > 0);
        assert!(result.bytes > 1_000_000, "真实材料应产生有内容的文件");

        // 不要删掉产物：随后用无头浏览器验证渲染
        println!("__OUT__={}", result.out_path);
    }
}
