//! Purpose: console MySQL connection pool, baseline drizzle migration runner and isolated
//! fixture databases for tests. Repositories in sibling modules build on `ConsoleDb`.
//! Scope: process-wide pool, per-test database lifecycle.
//! Uses:
//! `-- none
//! External:
//! |-- hex::encode
//! |-- mysql_async::Conn
//! |-- mysql_async::Error
//! |-- mysql_async::Opts
//! |-- mysql_async::OptsBuilder
//! |-- mysql_async::Pool
//! |-- mysql_async::prelude::Queryable
//! |-- sha2::Digest
//! |-- sha2::Sha256
//! |-- str::to_string
//! |-- str::trim
//! |-- time::Date::from_calendar_date
//! |-- time::Month::try_from
//! |-- time::PrimitiveDateTime::new
//! `-- time::Time::from_hms
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/state.rs
//! `-- hosted/src/main.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/mod.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-adapters/tests/console_db.rs
//! |-- hosted-adapters/tests/console_identity.rs
//! `-- hosted-adapters/tests/console_identity_auth.rs
//! Invariants: migrations are read verbatim from `packages/console/core/migrations/*/migration.sql`,
//! split on `--> statement-breakpoint`, applied in folder order and journaled in
//! `__drizzle_migrations` (hash = sha256 hex of the file, created_at = folder timestamp millis,
//! name = folder name) like drizzle-orm `mysql-core/dialect.js` `migrate`. No schema fork.
//! Every connection uses session time zone UTC.
//! Contract tests: hosted-adapters/tests/console_db.rs

use std::path::{Path, PathBuf};

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, Pool};
use sha2::{Digest, Sha256};

/// Errors from the console MySQL adapter. Keeps retryable vs conflict vs validation apart.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("mysql: {0}")]
    Mysql(#[from] mysql_async::Error),
    #[error("migration io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid migration folder name: {0}")]
    InvalidMigration(String),
    #[error("invalid database url: {0}")]
    Url(String),
}

impl DbError {
    /// Duplicate-key errors (MySQL 1062) map to domain conflicts.
    pub fn is_duplicate(&self) -> bool {
        matches!(self, DbError::Mysql(mysql_async::Error::Server(e)) if e.code == 1062)
    }
}

/// One baseline migration folder.
#[derive(Debug, Clone)]
pub struct Migration {
    pub name: String,
    pub hash: String,
    pub folder_millis: i64,
    pub statements: Vec<String>,
}

/// Default location of the baseline migrations relative to `rust/crates/hosted-adapters`.
pub fn baseline_migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packages/console/core/migrations")
}

/// Reads every migration folder in name order (drizzle `readMigrationFiles` equivalent).
pub fn read_migrations(dir: &Path) -> Result<Vec<Migration>, DbError> {
    let mut folders: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|e| e.path().join("migration.sql").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    folders.sort();
    folders
        .into_iter()
        .map(|name| {
            let sql = std::fs::read_to_string(dir.join(&name).join("migration.sql"))?;
            let folder_millis = folder_millis(&name)?;
            Ok(Migration {
                hash: hex::encode(Sha256::digest(sql.as_bytes())),
                statements: sql
                    .split("--> statement-breakpoint")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
                folder_millis,
                name,
            })
        })
        .collect()
}

fn folder_millis(name: &str) -> Result<i64, DbError> {
    let bad = || DbError::InvalidMigration(name.to_string());
    let stamp = name.get(..14).ok_or_else(bad)?;
    let n = |r: std::ops::Range<usize>| stamp[r].parse::<i64>().map_err(|_| bad());
    let date = time::Date::from_calendar_date(
        n(0..4)? as i32,
        time::Month::try_from(n(4..6)? as u8).map_err(|_| bad())?,
        n(6..8)? as u8,
    )
    .map_err(|_| bad())?;
    let t = time::Time::from_hms(n(8..10)? as u8, n(10..12)? as u8, n(12..14)? as u8)
        .map_err(|_| bad())?;
    Ok(time::PrimitiveDateTime::new(date, t)
        .assume_utc()
        .unix_timestamp()
        * 1000)
}

/// Every pooled connection runs in UTC so `timestamp(3)` values and `now()` match PlanetScale.
fn utc(opts: Opts) -> Opts {
    OptsBuilder::from_opts(opts)
        .init(vec!["SET time_zone = '+00:00'"])
        .into()
}

/// Console database handle (PlanetScale-compatible MySQL 8).
#[derive(Clone, Debug)]
pub struct ConsoleDb {
    pool: Pool,
    database: String,
}

impl ConsoleDb {
    pub fn connect(url: &str) -> Result<Self, DbError> {
        let opts = Opts::from_url(url).map_err(|e| DbError::Url(e.to_string()))?;
        let database = opts.db_name().unwrap_or_default().to_string();
        Ok(Self {
            pool: Pool::new(utc(opts)),
            database,
        })
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub async fn conn(&self) -> Result<Conn, DbError> {
        Ok(self.pool.get_conn().await?)
    }

    /// Applies pending baseline migrations, journaling them like drizzle.
    pub async fn migrate(&self, migrations: &[Migration]) -> Result<usize, DbError> {
        let mut conn = self.conn().await?;
        conn.query_drop(
            "CREATE TABLE IF NOT EXISTS `__drizzle_migrations` (id SERIAL PRIMARY KEY, hash TEXT NOT NULL, created_at BIGINT, name TEXT, applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP)",
        )
        .await?;
        let last: Option<i64> = conn
            .query_first(
                "SELECT created_at FROM `__drizzle_migrations` ORDER BY created_at DESC LIMIT 1",
            )
            .await?;
        let pending: Vec<_> = migrations
            .iter()
            .filter(|m| last.is_none_or(|last| m.folder_millis > last))
            .collect();
        for m in &pending {
            for stmt in &m.statements {
                conn.query_drop(stmt.as_str()).await?;
            }
            conn.exec_drop(
                "INSERT INTO `__drizzle_migrations` (`hash`, `created_at`, `name`) VALUES (?, ?, ?)",
                (&m.hash, m.folder_millis, &m.name),
            )
            .await?;
        }
        Ok(pending.len())
    }

    pub async fn disconnect(self) -> Result<(), DbError> {
        Ok(self.pool.disconnect().await?)
    }
}

/// Fixture-only helper: creates a uniquely named database on a local test server, migrates it
/// with the baseline migrations and drops it on `drop_database`. Reads `OPENCODE_P12_MYSQL_URL`
/// (default `mysql://root:p12fixture@127.0.0.1:33612`, the local Docker container).
pub struct TestDatabase {
    pub db: ConsoleDb,
    admin: Pool,
}

pub const TEST_MYSQL_ENV: &str = "OPENCODE_P12_MYSQL_URL";
pub const TEST_MYSQL_DEFAULT: &str = "mysql://root:p12fixture@127.0.0.1:33612";

impl TestDatabase {
    pub async fn create(label: &str) -> Result<Self, DbError> {
        let base = std::env::var(TEST_MYSQL_ENV).unwrap_or_else(|_| TEST_MYSQL_DEFAULT.to_string());
        let admin_opts = Opts::from_url(&base).map_err(|e| DbError::Url(e.to_string()))?;
        let admin = Pool::new(admin_opts.clone());
        let name = format!(
            "p12_{}_{}",
            label
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>(),
            hex::encode(rand::random::<[u8; 6]>())
        );
        admin
            .get_conn()
            .await?
            .query_drop(format!("CREATE DATABASE `{name}`"))
            .await?;
        let opts = OptsBuilder::from_opts(admin_opts).db_name(Some(name.clone()));
        let db = ConsoleDb {
            pool: Pool::new(utc(opts.into())),
            database: name,
        };
        db.migrate(&read_migrations(&baseline_migrations_dir())?)
            .await?;
        Ok(Self { db, admin })
    }

    pub async fn drop_database(self) -> Result<(), DbError> {
        let name = self.db.database.clone();
        self.db.disconnect().await?;
        self.admin
            .get_conn()
            .await?
            .query_drop(format!("DROP DATABASE `{name}`"))
            .await?;
        Ok(self.admin.disconnect().await?)
    }
}
