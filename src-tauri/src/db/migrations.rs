use rusqlite::Connection;

use super::Result;

/// 当前 schema 版本（与 migrations/ 目录中最后一个迁移脚本对应）。
pub const SCHEMA_VERSION: i64 = 1;

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

/// 迁移只增不改：已发布的脚本永不修改，新变更一律追加新文件。
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "0001_init",
    sql: include_str!("../../migrations/0001_init.sql"),
}];

pub fn run(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    for migration in MIGRATIONS {
        if migration.version <= current {
            continue;
        }

        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(migration.sql)?;
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;

        eprintln!(
            "[meeting-desk] 已应用数据库迁移 {} (v{})",
            migration.name, migration.version
        );
    }

    Ok(())
}
