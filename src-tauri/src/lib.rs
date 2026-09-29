//! 会议材料工作台 —— 本地核心。
//!
//! 架构纪律（见 docs/00-需求分析与技术方案-v0.1.md 第 1 节）：
//! Rust 侧只负责"只有原生层才能做"的四件事：
//!   文件系统 / SQLite + FTS5 / 系统 OCR / 本机模型代理与网络守卫。
//! 文档解析（PDF、DOCX、XLSX）在前端 Worker 内完成。

// M0 阶段：文件库、OCR、检索模块已实现并通过单测，但命令层要到 M1–M3
// 才正式接入，期间允许出现"尚未被调用"的告警。
#![allow(dead_code)]

mod bundle;
mod commands;
mod db;
mod library;
mod ocr;
mod organize;
mod search;
mod security;

use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::Connection;
use tauri::Manager;

/// 默认材料库位置（用户可见、可直接备份）。
pub const LIBRARY_DIR_NAME: &str = "会议材料工作台";

pub struct AppPaths {
    pub library_root: PathBuf,
    pub db_path: PathBuf,
}

pub struct AppState {
    pub conn: Mutex<Connection>,
    pub paths: AppPaths,
}

/// 解析材料库位置：优先 ~/Documents/会议材料工作台，
/// 若系统拒绝访问则回退到应用数据目录（并如实报告位置）。
fn resolve_paths(app: &tauri::AppHandle) -> io::Result<AppPaths> {
    let preferred = app
        .path()
        .document_dir()
        .map(|dir| dir.join(LIBRARY_DIR_NAME))
        .map_err(|err| io::Error::other(format!("无法定位文稿目录：{err}")));

    let library_root = match preferred {
        Ok(dir) if std::fs::create_dir_all(&dir).is_ok() => dir,
        Ok(dir) => {
            eprintln!(
                "[meeting-desk] 无法使用 {}（可能被系统权限拦截），改用应用数据目录",
                dir.display()
            );
            fallback_root(app)?
        }
        Err(err) => {
            eprintln!("[meeting-desk] {err}，改用应用数据目录");
            fallback_root(app)?
        }
    };

    let db_path = library_root.join("db").join("meetingdesk.sqlite");
    Ok(AppPaths {
        library_root,
        db_path,
    })
}

fn fallback_root(app: &tauri::AppHandle) -> io::Result<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| io::Error::other(format!("无法定位应用数据目录：{err}")))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let paths = resolve_paths(app.handle())?;
            let conn = db::open(&paths.db_path)?;
            eprintln!(
                "[meeting-desk] 材料库：{}\n[meeting-desk] 数据库：{}（schema v{}）",
                paths.library_root.display(),
                paths.db_path.display(),
                db::SCHEMA_VERSION
            );
            app.manage(AppState {
                conn: Mutex::new(conn),
                paths,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::db_stats,
            commands::net_audit_list,
            commands::guard_probe,
            commands::ocr_self_test,
            commands::offline_self_check,
            commands::create_meeting,
            commands::import_paths,
            commands::list_meetings,
            commands::delete_meeting,
            commands::meeting_tree,
            commands::create_topic,
            commands::assign_document,
            commands::clear_document_role,
            commands::auto_draft_topics,
            commands::set_topic_main,
            commands::export_bundle,
            commands::reveal_path,
        ])
        .build(tauri::generate_context!())
        .expect("构建会议材料工作台失败");

    app.run(|handle, event| {
        // 退出时把 WAL 合并回主库文件：保证"直接拷贝 .sqlite 就是完整数据"，
        // 也让异常情况下的备份更简单。
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<AppState>() {
                if let Ok(conn) = state.conn.lock() {
                    match conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                        Ok(()) => eprintln!("[meeting-desk] 已检查点数据库，退出"),
                        Err(err) => eprintln!("[meeting-desk] 退出检查点失败：{err}"),
                    }
                }
            }
        }
    });
}
