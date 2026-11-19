use crate::AppResult;
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, JsonSchema)]
pub struct EngineReport {
    pub application_version: String,
    pub sqlite_version: String,
    pub sqlite_version_number: i32,
    pub fts5_available: bool,
    pub wal_reset_fix_present: bool,
    pub data_dir: String,
    pub data_dir_exists: bool,
    pub persistent_database_initialized: bool,
}

pub fn inspect(data_dir: PathBuf) -> AppResult<EngineReport> {
    let connection = Connection::open_in_memory()?;
    let sqlite_version: String =
        connection.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
    let sqlite_version_number = rusqlite::version_number();
    if sqlite_version_number < 3_051_003 {
        return Err("bundled SQLite 低于本项目要求的 3.51.3（WAL-reset 修复）".into());
    }
    connection.execute_batch("CREATE VIRTUAL TABLE diagnostic_fts USING fts5(body);")?;
    connection.execute("INSERT INTO diagnostic_fts(body) VALUES (?1)", ["retenixa"])?;
    let hits: i64 = connection.query_row(
        "SELECT count(*) FROM diagnostic_fts WHERE diagnostic_fts MATCH ?1",
        ["retenixa"],
        |r| r.get(0),
    )?;
    if hits != 1 {
        return Err("FTS5 引擎诊断失败".into());
    }
    Ok(EngineReport {
        application_version: env!("CARGO_PKG_VERSION").into(),
        sqlite_version,
        sqlite_version_number,
        fts5_available: true,
        wal_reset_fix_present: true,
        data_dir_exists: data_dir.is_dir(),
        data_dir: data_dir.to_string_lossy().into_owned(),
        persistent_database_initialized: false,
    })
}
