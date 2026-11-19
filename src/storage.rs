use crate::{
    contracts::{DomainError, ErrorCode},
    error::Result,
    permissions, util,
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
pub const SCHEMA_VERSION: i64 = 5;
pub const APPLICATION_ID: i64 = 0x52544e58;
#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
    _gate: Arc<File>,
}
impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(DomainError::invalid("数据目录必须绝对路径"));
        }
        permissions::secure_dir(root)?;
        let root = std::fs::canonicalize(root)?;
        let gate = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("maintenance.lock"))?;
        permissions::secure_file(&root.join("maintenance.lock"))?;
        gate.try_lock_shared()
            .map_err(|_| DomainError::storage("数据库正在排他维护；禁止新连接"))?;
        let store = Self {
            root,
            _gate: Arc::new(gate),
        };
        let mut conn = store.connect_create()?;
        if rusqlite::version_number() < 3_051_003 {
            return Err(DomainError::new(
                ErrorCode::SchemaUnsupported,
                "SQLite 必须包含 WAL-reset 修复",
            ));
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let app: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
        if !(0..=SCHEMA_VERSION).contains(&version) || (app != 0 && app != APPLICATION_ID) {
            return Err(DomainError::new(
                ErrorCode::SchemaUnsupported,
                "数据库 schema 或 application_id 不兼容",
            ));
        }
        if version == 0 {
            let tables:i64=tx.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0))?;
            if tables != 0 {
                return Err(DomainError::new(
                    ErrorCode::SchemaUnsupported,
                    "不将未知数据库当作空库迁移",
                ));
            }
            tx.execute_batch(include_str!("../migrations/001_kernel.sql"))?;
            tx.execute("INSERT INTO schema_migrations VALUES(1,?1)", [util::now()])?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", 1)?;
        } else if app != APPLICATION_ID {
            return Err(DomainError::new(
                ErrorCode::SchemaUnsupported,
                "数据库 application_id 不匹配",
            ));
        }
        if version < 2 {
            tx.execute_batch(include_str!("../migrations/002_search.sql"))?;
            tx.execute("INSERT INTO schema_migrations VALUES(2,?1)", [util::now()])?;
            tx.pragma_update(None, "user_version", 2)?;
        }
        migrate_after_search(&tx)?;
        tx.commit()?;
        drop(conn);
        crate::search::rebuild(&store, false)?;
        permissions::secure_file(&store.db_path())?;
        Ok(store)
    }
    pub fn db_path(&self) -> PathBuf {
        self.root.join("retenixa.sqlite3")
    }
    fn connect_create(&self) -> Result<Connection> {
        let c = Connection::open(self.db_path())?;
        configure(&c)?;
        Ok(c)
    }
    pub fn connect(&self) -> Result<Connection> {
        let c = Connection::open_with_flags(self.db_path(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure(&c)?;
        let v: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v != SCHEMA_VERSION {
            return Err(DomainError::new(
                ErrorCode::SchemaUnsupported,
                "schema 不兼容；请停止旧进程",
            ));
        }
        Ok(c)
    }
}
pub(crate) fn migrate_after_search(c: &Connection) -> Result<()> {
    let version: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version == 2 {
        c.execute_batch(include_str!("../migrations/003_project_identity.sql"))?;
        c.execute(
            "INSERT INTO schema_migrations(version,applied_at) VALUES(3,?1)",
            [util::now()],
        )?;
        c.pragma_update(None, "user_version", 3)?;
    }
    if (2..=3).contains(&version) {
        c.execute_batch(include_str!("../migrations/004_stdio_process.sql"))?;
        c.execute(
            "INSERT INTO schema_migrations(version,applied_at) VALUES(4,?1)",
            [util::now()],
        )?;
        c.pragma_update(None, "user_version", 4)?;
    }
    if (2..=4).contains(&version) {
        c.execute_batch(include_str!("../migrations/005_search_fields.sql"))?;
        c.execute(
            "INSERT INTO schema_migrations(version,applied_at) VALUES(5,?1)",
            [util::now()],
        )?;
        c.pragma_update(None, "user_version", 5)?;
    }
    Ok(())
}
pub fn assert_schema(c: &Connection) -> Result<()> {
    let version: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let app: i64 = c.pragma_query_value(None, "application_id", |r| r.get(0))?;
    if version != SCHEMA_VERSION || app != APPLICATION_ID {
        return Err(DomainError::new(
            ErrorCode::SchemaUnsupported,
            "事务视图中的 schema 不兼容",
        ));
    }
    Ok(())
}
pub fn configure(c: &Connection) -> Result<()> {
    c.busy_timeout(Duration::from_millis(750))?;
    c.pragma_update(None, "foreign_keys", true)?;
    c.pragma_update(None, "synchronous", "FULL")?;
    c.pragma_update(None, "wal_autocheckpoint", 1000)?;
    Ok(())
}
