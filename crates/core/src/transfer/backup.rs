//! Whole-database backup and restore.
//!
//! Each backend uses the mechanism its own ecosystem trusts, as TablePlus does:
//!
//! - **Postgres** and **MySQL/MariaDB** run the vendor client tools on this machine
//!   (`pg_dump`/`pg_restore`/`psql`, `mysqldump`/`mysql`). They capture functions,
//!   triggers, sequences and ownership exactly, and stream large databases.
//! - **SQL Server** runs `BACKUP DATABASE … WITH COPY_ONLY` / `RESTORE DATABASE` on the
//!   connection itself. The `.bak` file lives on the *server's* disk — that is how SQL
//!   Server works — and `COPY_ONLY` leaves the DBA's backup chain untouched.
//! - **SQLite** and **DuckDB** snapshot through the engine (`VACUUM INTO`,
//!   `COPY FROM DATABASE`), needing no tool; a restore replaces the database file while the
//!   app is disconnected from it.
//! - **Cassandra/ScyllaDB** have no client-side backup (`nodetool snapshot` runs on the
//!   nodes), so they report that instead.
//!
//! Secrets never reach a command line, where other local users could read them from the
//! process list: Postgres gets the password through `PGPASSWORD`, MySQL through a
//! temporary `0600` option file removed as soon as the tool exits.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::database::Database;
use crate::error::{CoreError, Result};
use crate::model::{ConnectionConfig, DbKind, SslMode};
use crate::tunnel::SshTunnel;

/// How a backend is backed up, which decides what the dialog asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Client tools on this machine write/read a local file.
    Tool,
    /// The database server writes/reads a file on its own disk.
    Server,
    /// The engine itself copies the database to/from a local file.
    Builtin,
    /// No client-side backup exists for this backend.
    Unsupported,
}

impl Method {
    pub fn of(kind: DbKind) -> Method {
        match kind {
            DbKind::Postgres | DbKind::MySql | DbKind::MariaDb => Method::Tool,
            DbKind::SqlServer => Method::Server,
            DbKind::Sqlite | DbKind::DuckDb => Method::Builtin,
            DbKind::Cassandra | DbKind::ScyllaDb => Method::Unsupported,
        }
    }
}

/// Why a backend can't be backed up from here, for [`Method::Unsupported`].
pub fn unsupported_reason(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Cassandra | DbKind::ScyllaDb => {
            "Cassandra and ScyllaDB are backed up on the nodes with `nodetool snapshot`; \
             there is no client-side backup."
        }
        _ => "",
    }
}

/// Backup flavours for Postgres and SQL Server. Other backends have a single format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DumpFormat {
    /// Postgres: `pg_dump --format=custom`, restorable selectively with `pg_restore`.
    /// SQL Server: a native `.bak`, written on the server.
    #[default]
    Archive,
    /// A plain `.sql` script: `pg_dump --format=plain` replayed with `psql`, or for SQL Server
    /// a script this app generates (CREATE TABLE + INSERT, `GO`-separated) on this computer,
    /// which can also cover chosen tables only.
    PlainSql,
}

#[derive(Debug, Clone, Default)]
pub struct BackupOptions {
    /// The file to write — local, or on the server for [`Method::Server`].
    pub path: PathBuf,
    pub format: DumpFormat,
    /// Structure only, no rows (Postgres and MySQL).
    pub schema_only: bool,
    /// Only these tables, as `(schema, name)`; empty backs up the whole database. SQL Server
    /// backups are always whole-database.
    pub tables: Vec<(Option<String>, String)>,
}

/// Whether `kind` can back up a chosen subset of tables.
pub fn supports_table_selection(kind: DbKind) -> bool {
    matches!(Method::of(kind), Method::Tool | Method::Builtin)
}

/// A `pg_dump --table` pattern matching exactly one table: double quotes keep the name's
/// case and make wildcard characters literal.
fn pg_table_pattern(schema: Option<&str>, table: &str) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    match schema {
        Some(schema) => format!("{}.{}", quote(schema), quote(table)),
        None => quote(table),
    }
}

#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// The backup to read — local, or on the server for [`Method::Server`].
    pub path: PathBuf,
    /// Drop existing objects before recreating them (Postgres archives).
    pub clean: bool,
    /// SQL Server: a server-side `.bak` ([`DumpFormat::Archive`]) or a local script.
    pub format: DumpFormat,
}

/// The file extension a backup is saved with.
pub fn default_extension(kind: DbKind, format: DumpFormat) -> &'static str {
    match (kind, format) {
        (DbKind::Postgres, DumpFormat::Archive) => "dump",
        (DbKind::Postgres | DbKind::MySql | DbKind::MariaDb, _) => "sql",
        (DbKind::SqlServer, DumpFormat::Archive) => "bak",
        (DbKind::SqlServer, DumpFormat::PlainSql) => "sql",
        (DbKind::Sqlite, _) => "sqlite",
        (DbKind::DuckDb, _) => "duckdb",
        (DbKind::Cassandra | DbKind::ScyllaDb, _) => "",
    }
}

// ─── Client tools ──────────────────────────────────────────────────────────────────────

/// The client tools a backup/restore needs, as found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    /// Writes the backup (`pg_dump`, `mysqldump`).
    pub dump: PathBuf,
    /// Reads an archive back (`pg_restore`); for MySQL the same as `load`.
    pub restore: PathBuf,
    /// Replays a plain SQL script (`psql`, `mysql`).
    pub load: PathBuf,
    /// `--version` of `dump`, for the dialog ("pg_dump (PostgreSQL) 16.2").
    pub version: String,
    /// The MySQL-family tools are MariaDB's, which spell some SSL flags differently.
    pub mariadb: bool,
}

/// Find the client tools for `kind`, or say what to install.
///
/// A GUI app launched from the Finder or a desktop menu doesn't inherit the shell's
/// `PATH`, so besides `PATH` this looks where package managers and installers put them
/// (Homebrew, Postgres.app, `/usr/lib/postgresql/<n>/bin`, `C:\Program Files\…`). Among
/// several Postgres installs the newest `pg_dump` wins: an older one refuses to dump a
/// newer server.
pub fn find_tools(kind: DbKind) -> std::result::Result<Tools, String> {
    let dirs = search_dirs();
    match kind {
        DbKind::Postgres => {
            let mut best: Option<(u32, PathBuf, String)> = None;
            for dir in &dirs {
                let dump = dir.join(exe("pg_dump"));
                if !dump.is_file() || !dir.join(exe("pg_restore")).is_file() {
                    continue;
                }
                let version = tool_version(&dump);
                let major = pg_major(&version);
                if best.as_ref().is_none_or(|(m, _, _)| major > *m) {
                    best = Some((major, dir.clone(), version));
                }
            }
            let (_, dir, version) = best.ok_or_else(|| install_hint(kind))?;
            Ok(Tools {
                dump: dir.join(exe("pg_dump")),
                restore: dir.join(exe("pg_restore")),
                load: dir.join(exe("psql")),
                version,
                mariadb: false,
            })
        }
        DbKind::MySql | DbKind::MariaDb => {
            // MariaDB ships `mariadb-dump`/`mariadb` (with `mysqldump`/`mysql` as aliases);
            // prefer the native names for the matching server.
            let pairs: &[(&str, &str)] = if kind == DbKind::MariaDb {
                &[("mariadb-dump", "mariadb"), ("mysqldump", "mysql")]
            } else {
                &[("mysqldump", "mysql"), ("mariadb-dump", "mariadb")]
            };
            for (dump, load) in pairs {
                for dir in &dirs {
                    let (dump, load) = (dir.join(exe(dump)), dir.join(exe(load)));
                    if dump.is_file() && load.is_file() {
                        let version = tool_version(&dump);
                        return Ok(Tools {
                            mariadb: version.contains("MariaDB"),
                            restore: load.clone(),
                            dump,
                            load,
                            version,
                        });
                    }
                }
            }
            Err(install_hint(kind))
        }
        _ => Err(format!("{} backups don't use client tools.", kind.label())),
    }
}

/// What to install when the tools are missing, per platform.
fn install_hint(kind: DbKind) -> String {
    let (tools, mac, linux, windows) = match kind {
        DbKind::Postgres => (
            "pg_dump and pg_restore",
            "brew install libpq",
            "sudo apt install postgresql-client",
            "the PostgreSQL installer (Command Line Tools)",
        ),
        _ => (
            "mysqldump and mysql",
            "brew install mysql-client",
            "sudo apt install mysql-client (or mariadb-client)",
            "the MySQL Shell / Server installer",
        ),
    };
    let how = if cfg!(target_os = "macos") {
        format!("Install them with `{mac}`.")
    } else if cfg!(windows) {
        format!("Install them with {windows}.")
    } else {
        format!("Install them with `{linux}`.")
    };
    format!("{tools} weren't found on this computer. {how}")
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// `PATH`, then the usual install locations (versioned directories included).
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let mut add = |dir: PathBuf| {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    let children = |parent: &str, prefixes: &[&str], suffix: &str| -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(parent) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                prefixes.iter().any(|p| name.starts_with(p))
            })
            .map(|e| e.path().join(suffix))
            .collect();
        found.sort();
        found
    };
    let families = ["postgresql", "libpq", "mysql", "mariadb"];
    if cfg!(windows) {
        for root in ["C:\\Program Files", "C:\\Program Files (x86)"] {
            for dir in children(root, &["postgresql", "mysql", "mariadb"], "") {
                // PostgreSQL\16\bin, MySQL\MySQL Server 8.0\bin, MariaDB 11.4\bin
                for sub in children(&dir.to_string_lossy(), &[""], "bin") {
                    add(sub);
                }
                add(dir.join("bin"));
            }
        }
    } else {
        for dir in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/opt/local/bin",
        ] {
            add(PathBuf::from(dir));
        }
        for root in ["/opt/homebrew/opt", "/usr/local/opt"] {
            for dir in children(root, &families, "bin") {
                add(dir);
            }
        }
        for dir in children("/Applications/Postgres.app/Contents/Versions", &[""], "bin") {
            add(dir);
        }
        for dir in children("/usr/lib/postgresql", &[""], "bin") {
            add(dir);
        }
    }
    dirs
}

fn tool_version(program: &Path) -> String {
    let mut command = std::process::Command::new(program);
    command.arg("--version");
    hide_console(&mut command);
    command
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The major version in `pg_dump (PostgreSQL) 16.2`, 0 if unreadable.
fn pg_major(version: &str) -> u32 {
    version
        .split_whitespace()
        .rev()
        .find_map(|word| word.split('.').next()?.parse().ok())
        .unwrap_or(0)
}

// ─── Command plans (pure, so they can be tested without a server) ──────────────────────

/// Where the tool connects: the connection's host, or the local end of an SSH tunnel.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    pub ssl_mode: SslMode,
    pub ssl_ca: String,
    pub ssl_cert: String,
    pub ssl_key: String,
}

impl Endpoint {
    pub fn of(cfg: &ConnectionConfig) -> Self {
        Endpoint {
            host: cfg.host.clone(),
            port: cfg.port,
            user: cfg.user.clone(),
            database: cfg.database.clone(),
            ssl_mode: cfg.ssl_mode,
            ssl_ca: cfg.ssl_ca_cert.clone(),
            ssl_cert: cfg.ssl_client_cert.clone(),
            ssl_key: cfg.ssl_client_key.clone(),
        }
    }
}

/// One tool invocation. `secret_file` is an option file to write (0600) before running
/// and delete afterwards; `stdin` a file fed to the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<PathBuf>,
    pub secret_file: Option<(PathBuf, String)>,
}

fn pg_env(ep: &Endpoint, password: Option<&str>) -> Vec<(String, String)> {
    let mut env = vec![
        ("PGCONNECT_TIMEOUT".to_string(), "15".to_string()),
        ("PGAPPNAME".to_string(), "plusplus".to_string()),
        (
            "PGSSLMODE".to_string(),
            match ep.ssl_mode {
                SslMode::Disable => "disable",
                SslMode::Prefer => "prefer",
                SslMode::Require => "require",
                SslMode::VerifyCa => "verify-ca",
                SslMode::VerifyFull => "verify-full",
            }
            .to_string(),
        ),
    ];
    for (var, value) in [
        ("PGSSLROOTCERT", &ep.ssl_ca),
        ("PGSSLCERT", &ep.ssl_cert),
        ("PGSSLKEY", &ep.ssl_key),
    ] {
        if !value.trim().is_empty() {
            env.push((var.to_string(), value.trim().to_string()));
        }
    }
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        env.push(("PGPASSWORD".to_string(), password.to_string()));
    }
    env
}

fn pg_target(ep: &Endpoint) -> Vec<String> {
    vec![
        format!("--host={}", ep.host),
        format!("--port={}", ep.port),
        format!("--username={}", ep.user),
        format!("--dbname={}", ep.database),
        "--no-password".to_string(),
    ]
}

/// `pg_dump` for `opts`.
pub fn pg_dump_command(
    tools: &Tools,
    ep: &Endpoint,
    password: Option<&str>,
    opts: &BackupOptions,
) -> ToolCommand {
    let mut args = pg_target(ep);
    args.push(
        match opts.format {
            DumpFormat::Archive => "--format=custom",
            DumpFormat::PlainSql => "--format=plain",
        }
        .to_string(),
    );
    if opts.schema_only {
        args.push("--schema-only".to_string());
    }
    for (schema, table) in &opts.tables {
        args.push(format!(
            "--table={}",
            pg_table_pattern(schema.as_deref(), table)
        ));
    }
    args.push(format!("--file={}", opts.path.display()));
    ToolCommand {
        program: tools.dump.clone(),
        args,
        env: pg_env(ep, password),
        stdin: None,
        secret_file: None,
    }
}

/// `pg_restore` for an archive, `psql` for a plain script. Ownership and grants are skipped
/// so a backup restores into a server whose roles differ from the source's.
pub fn pg_restore_command(
    tools: &Tools,
    ep: &Endpoint,
    password: Option<&str>,
    opts: &RestoreOptions,
    archive: bool,
) -> ToolCommand {
    let mut args = pg_target(ep);
    let program = if archive {
        args.extend(["--no-owner", "--no-acl"].map(String::from));
        if opts.clean {
            args.extend(["--clean", "--if-exists"].map(String::from));
        }
        args.push(opts.path.display().to_string());
        tools.restore.clone()
    } else {
        // Stop at the first error, and apply the whole script or nothing.
        args.extend(["--set=ON_ERROR_STOP=1", "--single-transaction", "--quiet"].map(String::from));
        args.push(format!("--file={}", opts.path.display()));
        tools.load.clone()
    };
    ToolCommand {
        program,
        args,
        env: pg_env(ep, password),
        stdin: None,
        secret_file: None,
    }
}

/// A `[client]` option file carrying the password, for `--defaults-extra-file`.
fn mysql_option_file(password: Option<&str>) -> (PathBuf, String) {
    let escaped = password
        .unwrap_or_default()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let name = format!(
        "plusplus-{}-{}.cnf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    (
        std::env::temp_dir().join(name),
        format!("[client]\npassword=\"{escaped}\"\n"),
    )
}

fn mysql_target(tools: &Tools, ep: &Endpoint, option_file: &Path) -> Vec<String> {
    // `--defaults-extra-file` must come first.
    let mut args = vec![
        format!("--defaults-extra-file={}", option_file.display()),
        format!("--host={}", ep.host),
        format!("--port={}", ep.port),
        format!("--user={}", ep.user),
        "--default-character-set=utf8mb4".to_string(),
    ];
    let ssl: &[&str] = match (ep.ssl_mode, tools.mariadb) {
        (SslMode::Disable, false) => &["--ssl-mode=DISABLED"],
        (SslMode::Disable, true) => &["--skip-ssl"],
        (SslMode::Prefer, _) => &[],
        (SslMode::Require, false) => &["--ssl-mode=REQUIRED"],
        (SslMode::VerifyCa, false) => &["--ssl-mode=VERIFY_CA"],
        (SslMode::VerifyFull, false) => &["--ssl-mode=VERIFY_IDENTITY"],
        (SslMode::Require, true) => &["--ssl"],
        (SslMode::VerifyCa | SslMode::VerifyFull, true) => &["--ssl", "--ssl-verify-server-cert"],
    };
    args.extend(ssl.iter().map(|s| s.to_string()));
    for (flag, value) in [
        ("--ssl-ca", &ep.ssl_ca),
        ("--ssl-cert", &ep.ssl_cert),
        ("--ssl-key", &ep.ssl_key),
    ] {
        if !value.trim().is_empty() {
            args.push(format!("{flag}={}", value.trim()));
        }
    }
    args
}

/// `mysqldump`: a consistent snapshot (one transaction) with routines and triggers.
pub fn mysql_dump_command(
    tools: &Tools,
    ep: &Endpoint,
    password: Option<&str>,
    opts: &BackupOptions,
) -> ToolCommand {
    let secret = mysql_option_file(password);
    let mut args = mysql_target(tools, ep, &secret.0);
    args.extend(["--single-transaction", "--triggers", "--hex-blob"].map(String::from));
    // Stored routines belong to the database, not to a table: only a whole-database
    // backup carries them.
    if opts.tables.is_empty() {
        args.push("--routines".to_string());
    }
    if opts.schema_only {
        args.push("--no-data".to_string());
    }
    args.push(format!("--result-file={}", opts.path.display()));
    args.push(ep.database.clone());
    args.extend(opts.tables.iter().map(|(_, table)| table.clone()));
    ToolCommand {
        program: tools.dump.clone(),
        args,
        env: Vec::new(),
        stdin: None,
        secret_file: Some(secret),
    }
}

/// `mysql` replaying the dump (which drops and recreates each table itself).
pub fn mysql_restore_command(
    tools: &Tools,
    ep: &Endpoint,
    password: Option<&str>,
    opts: &RestoreOptions,
) -> ToolCommand {
    let secret = mysql_option_file(password);
    let mut args = mysql_target(tools, ep, &secret.0);
    args.push(ep.database.clone());
    ToolCommand {
        program: tools.load.clone(),
        args,
        env: Vec::new(),
        stdin: Some(opts.path.clone()),
        secret_file: Some(secret),
    }
}

/// T-SQL string literal body: single quotes doubled.
fn tsql_literal(text: &str) -> String {
    format!("N'{}'", text.replace('\'', "''"))
}

/// `BACKUP DATABASE … WITH COPY_ONLY`: a full backup that doesn't reset the differential
/// base or truncate the log, so scheduled backups stay valid.
pub fn mssql_backup_sql(database: &str, server_path: &str) -> String {
    format!(
        "BACKUP DATABASE {} TO DISK = {} WITH COPY_ONLY, INIT, CHECKSUM, NAME = N'plusplus backup';",
        DbKind::SqlServer.quote_ident(database),
        tsql_literal(server_path),
    )
}

/// Restore over `database`: other sessions are rolled back (single-user), the backup
/// replaces the database, and multi-user access returns even if the restore fails.
pub fn mssql_restore_sql(database: &str, server_path: &str) -> String {
    let db = DbKind::SqlServer.quote_ident(database);
    let path = tsql_literal(server_path);
    format!(
        "USE [master];\n\
         ALTER DATABASE {db} SET SINGLE_USER WITH ROLLBACK IMMEDIATE;\n\
         BEGIN TRY\n\
         \x20   RESTORE DATABASE {db} FROM DISK = {path} WITH REPLACE, RECOVERY, CHECKSUM;\n\
         END TRY\n\
         BEGIN CATCH\n\
         \x20   ALTER DATABASE {db} SET MULTI_USER;\n\
         \x20   THROW;\n\
         END CATCH;\n\
         ALTER DATABASE {db} SET MULTI_USER;"
    )
}

/// The server's default backup folder (SQL Server 2019+ reports it), to prefill the path.
pub const MSSQL_DEFAULT_BACKUP_DIR_SQL: &str =
    "SELECT CAST(SERVERPROPERTY('InstanceDefaultBackupPath') AS NVARCHAR(4000))";

fn sqlite_literal(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

// ─── Running ───────────────────────────────────────────────────────────────────────────

/// Keep a Windows GUI app from flashing a console window for each tool.
fn hide_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Deletes the MySQL option file however the run ends.
struct SecretFile(PathBuf);

impl Drop for SecretFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_secret_file(path: &Path, contents: &str) -> Result<SecretFile> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let guard = SecretFile(path.to_path_buf());
    file.write_all(contents.as_bytes())?;
    Ok(guard)
}

/// The last few meaningful lines a tool printed, for the error message.
fn tail_lines(output: &str, lines: usize) -> String {
    let kept: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    kept[kept.len().saturating_sub(lines)..].join("\n")
}

/// Run one tool to completion, killing it if `cancel` fires. A non-zero exit becomes an
/// error carrying the tail of its stderr.
pub async fn run_tool(command: ToolCommand, cancel: CancellationToken) -> Result<()> {
    let _secret = match &command.secret_file {
        Some((path, contents)) => Some(write_secret_file(path, contents)?),
        None => None,
    };
    let mut std_command = std::process::Command::new(&command.program);
    std_command
        .args(&command.args)
        .envs(command.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut std_command);
    match &command.stdin {
        Some(path) => {
            std_command.stdin(std::fs::File::open(path)?);
        }
        None => {
            std_command.stdin(Stdio::null());
        }
    }
    let mut process = tokio::process::Command::from(std_command);
    process.kill_on_drop(true);
    let mut child = process.spawn().map_err(|e| {
        CoreError::Backup(format!("couldn't start {}: {e}", command.program.display()))
    })?;
    let mut stderr = child.stderr.take().expect("stderr is piped");
    // Drained concurrently: a tool blocked on a full stderr pipe would never exit.
    let reader = tokio::spawn(async move {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer).await;
        String::from_utf8_lossy(&buffer).into_owned()
    });
    let status = tokio::select! {
        status = child.wait() => status?,
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(CoreError::Backup("Cancelled".into()));
        }
    };
    let errors = reader.await.unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        let detail = tail_lines(&errors, 6);
        Err(CoreError::Backup(if detail.is_empty() {
            format!(
                "{} exited with {status}",
                command
                    .program
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            )
        } else {
            detail
        }))
    }
}

/// Everything a backup/restore needs about the connection.
pub struct Job {
    pub cfg: ConnectionConfig,
    pub password: Option<String>,
    pub ssh_secret: Option<String>,
    /// The live connection, used by the server-side and built-in methods.
    pub db: Arc<dyn Database>,
}

impl Job {
    /// Where the client tools connect. With an SSH tunnel, a fresh tunnel is opened for the
    /// job (returned so it lives as long as the tool runs).
    async fn endpoint(&self) -> Result<(Endpoint, Option<SshTunnel>)> {
        let mut endpoint = Endpoint::of(&self.cfg);
        if self.cfg.ssh_enabled && self.cfg.kind.is_server() {
            let tunnel = SshTunnel::open(&self.cfg, self.ssh_secret.as_deref()).await?;
            endpoint.host = "127.0.0.1".to_string();
            endpoint.port = tunnel.local_port;
            return Ok((endpoint, Some(tunnel)));
        }
        Ok((endpoint, None))
    }
}

/// Back up the whole database described by `job`.
pub async fn backup(job: &Job, opts: &BackupOptions, cancel: CancellationToken) -> Result<()> {
    let kind = job.cfg.kind;
    match Method::of(kind) {
        Method::Tool => {
            let tools = find_tools(kind).map_err(CoreError::Backup)?;
            let (endpoint, _tunnel) = job.endpoint().await?;
            let password = job.password.as_deref();
            let command = if kind == DbKind::Postgres {
                pg_dump_command(&tools, &endpoint, password, opts)
            } else {
                mysql_dump_command(&tools, &endpoint, password, opts)
            };
            let result = run_tool(command, cancel).await;
            if result.is_err() {
                // Don't leave a truncated dump that looks like a backup.
                let _ = std::fs::remove_file(&opts.path);
            }
            result
        }
        Method::Server if opts.format == DumpFormat::PlainSql => {
            let result = mssql_script_backup(job, opts, cancel).await;
            if result.is_err() {
                let _ = std::fs::remove_file(&opts.path);
            }
            result
        }
        Method::Server => {
            if !opts.tables.is_empty() {
                return Err(CoreError::Backup(
                    "A SQL Server .bak holds the whole database; choose the SQL script \
                     format to back up single tables."
                        .into(),
                ));
            }
            let sql = mssql_backup_sql(&job.cfg.database, &opts.path.to_string_lossy());
            job.db.execute(&sql).await.map(|_| ())
        }
        Method::Builtin => {
            // Both engines refuse to write over an existing file; the save dialog already
            // confirmed replacing it.
            if opts.path.exists() {
                std::fs::remove_file(&opts.path)?;
            }
            if kind == DbKind::Sqlite {
                job.db
                    .execute(&format!("VACUUM INTO {}", sqlite_literal(&opts.path)))
                    .await?;
                if !opts.tables.is_empty() {
                    // Keep only the chosen tables in the copy. ATTACH is per connection, so
                    // the whole step goes as one batch on one pooled connection.
                    let all = sqlite_tables(&*job.db, "main").await?;
                    let drops: String = unselected(&all, &opts.tables, None)
                        .map(|t| {
                            format!(
                                "DROP TABLE plusplus_backup.{};\n",
                                DbKind::Sqlite.quote_ident(t)
                            )
                        })
                        .collect();
                    job.db
                        .execute(&format!(
                            "ATTACH {} AS plusplus_backup;\n{drops}DETACH plusplus_backup;",
                            sqlite_literal(&opts.path)
                        ))
                        .await?;
                }
            } else {
                let alias = "plusplus_backup";
                let current = job
                    .db
                    .execute("SELECT current_database()")
                    .await?
                    .rows
                    .first()
                    .and_then(|row| row.first())
                    .map(|v| v.display())
                    .ok_or_else(|| CoreError::Backup("couldn't read the database name".into()))?;
                job.db
                    .execute(&format!("ATTACH {} AS {alias}", sqlite_literal(&opts.path)))
                    .await?;
                let mut copied = job
                    .db
                    .execute(&format!(
                        "COPY FROM DATABASE {} TO {alias}",
                        DbKind::DuckDb.quote_ident(&current)
                    ))
                    .await
                    .map(|_| ());
                if copied.is_ok() && !opts.tables.is_empty() {
                    copied = drop_unselected_duckdb(&*job.db, alias, &opts.tables).await;
                }
                let _ = job.db.execute(&format!("DETACH {alias}")).await;
                copied?;
            }
            Ok(())
        }
        Method::Unsupported => Err(CoreError::Backup(unsupported_reason(kind).into())),
    }
}

// ─── SQL Server scripts ─────────────────────────────────────────────────────────────────

/// Rows per `INSERT … VALUES` — SQL Server's limit for a table value constructor.
const MSSQL_INSERT_ROWS: usize = 1000;

fn mssql_name(schema: Option<&str>, table: &str) -> String {
    let q = |s: &str| DbKind::SqlServer.quote_ident(s);
    format!("{}.{}", q(schema.unwrap_or("dbo")), q(table))
}

/// `rowversion`/`timestamp` columns are filled by the server and can't be inserted.
fn mssql_insertable(column: &crate::model::ColumnInfo) -> bool {
    let t = column.data_type.to_ascii_lowercase();
    t != "timestamp" && t != "rowversion"
}

fn mssql_identity(column: &crate::model::ColumnInfo) -> bool {
    column.generated
        && !column
            .default
            .as_deref()
            .is_some_and(|d| d.to_ascii_lowercase().contains("next value for"))
}

/// `CREATE TABLE` for an introspected table: types with their lengths, identity, NULL-ness,
/// defaults, column checks and the primary key. Foreign keys and indexes come later in the
/// script, after the data.
pub fn mssql_create_table_sql(table: &crate::model::TableInfo) -> String {
    let q = |s: &str| DbKind::SqlServer.quote_ident(s);
    let mut lines: Vec<String> = table
        .columns
        .iter()
        .map(|c| {
            let mut line = format!("    {} {}", q(&c.name), c.data_type);
            if mssql_identity(c) {
                line.push_str(" IDENTITY(1,1)");
            }
            line.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
            if let Some(default) = c.default.as_deref().filter(|d| !d.trim().is_empty()) {
                line.push_str(&format!(" DEFAULT {default}"));
            }
            if let Some(check) = c.check.as_deref().filter(|d| !d.trim().is_empty()) {
                line.push_str(&format!(" CHECK {check}"));
            }
            line
        })
        .collect();
    let pk: Vec<String> = table
        .columns
        .iter()
        .filter(|c| c.primary_key)
        .map(|c| q(&c.name))
        .collect();
    if !pk.is_empty() {
        lines.push(format!(
            "    CONSTRAINT {} PRIMARY KEY ({})",
            q(&format!("PK_{}", table.name)),
            pk.join(", ")
        ));
    }
    format!(
        "CREATE TABLE {} (\n{}\n);",
        mssql_name(table.schema.as_deref(), &table.name),
        lines.join(",\n")
    )
}

/// Secondary indexes (the primary key's own index is part of `CREATE TABLE`).
fn mssql_index_sql(table: &crate::model::TableInfo) -> Vec<String> {
    let q = |s: &str| DbKind::SqlServer.quote_ident(s);
    let pk: Vec<&str> = table
        .columns
        .iter()
        .filter(|c| c.primary_key)
        .map(|c| c.name.as_str())
        .collect();
    table
        .indexes
        .iter()
        .filter(|index| {
            !(index.unique
                && index
                    .columns
                    .iter()
                    .map(String::as_str)
                    .eq(pk.iter().copied()))
        })
        .map(|index| {
            format!(
                "CREATE {}INDEX {} ON {} ({});",
                if index.unique { "UNIQUE " } else { "" },
                q(&index.name),
                mssql_name(table.schema.as_deref(), &table.name),
                index
                    .columns
                    .iter()
                    .map(|c| q(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect()
}

fn mssql_fk_sql(table: &crate::model::TableInfo) -> Vec<String> {
    let q = |s: &str| DbKind::SqlServer.quote_ident(s);
    let action = |label: &str, action: &str| {
        let action = action.replace('_', " ").to_ascii_uppercase();
        if action.is_empty() || action == "NO ACTION" {
            String::new()
        } else {
            format!(" ON {label} {action}")
        }
    };
    table
        .foreign_keys
        .iter()
        .map(|fk| {
            format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({}){}{};",
                mssql_name(table.schema.as_deref(), &table.name),
                q(&fk.name),
                fk.columns
                    .iter()
                    .map(|c| q(c))
                    .collect::<Vec<_>>()
                    .join(", "),
                mssql_name(fk.ref_schema.as_deref(), &fk.ref_table),
                fk.ref_columns
                    .iter()
                    .map(|c| q(c))
                    .collect::<Vec<_>>()
                    .join(", "),
                action("DELETE", &fk.on_delete),
                action("UPDATE", &fk.on_update),
            )
        })
        .collect()
}

/// Streams a table's rows into the script as `GO`-separated multi-row INSERTs.
struct InsertSink<'a> {
    out: &'a mut std::io::BufWriter<std::fs::File>,
    prefix: String,
    rows: Vec<String>,
}

impl InsertSink<'_> {
    fn flush_rows(&mut self) -> std::io::Result<()> {
        use std::io::Write;
        if self.rows.is_empty() {
            return Ok(());
        }
        writeln!(self.out, "{}\n{};\nGO", self.prefix, self.rows.join(",\n"))?;
        self.rows.clear();
        Ok(())
    }
}

impl crate::export::RowSink for InsertSink<'_> {
    fn begin(&mut self, _columns: &[crate::model::ColumnMeta]) -> std::io::Result<()> {
        Ok(())
    }
    fn write_row(&mut self, row: &[crate::value::Value]) -> std::io::Result<()> {
        let values: Vec<String> = row
            .iter()
            .map(|v| {
                crate::model::value_to_literal(v, DbKind::SqlServer)
                    .unwrap_or_else(|| "NULL".to_string())
            })
            .collect();
        self.rows.push(format!("({})", values.join(", ")));
        if self.rows.len() >= MSSQL_INSERT_ROWS {
            self.flush_rows()?;
        }
        Ok(())
    }
    fn finish(&mut self) -> std::io::Result<()> {
        self.flush_rows()
    }
}

/// Write a SQL Server script backup of the chosen tables (all when none are chosen) to a
/// local file: drop, create, data, then indexes and foreign keys — runnable in SSMS, and
/// restored by [`mssql_script_restore`].
async fn mssql_script_backup(
    job: &Job,
    opts: &BackupOptions,
    cancel: CancellationToken,
) -> Result<()> {
    use std::io::Write;
    let chosen: Vec<(Option<String>, String)> = if opts.tables.is_empty() {
        job.db
            .execute(
                "SELECT s.name, t.name FROM sys.tables t \
                 JOIN sys.schemas s ON s.schema_id = t.schema_id \
                 WHERE t.is_ms_shipped = 0 ORDER BY s.name, t.name",
            )
            .await?
            .rows
            .into_iter()
            .map(|row| (Some(row[0].display()), row[1].display()))
            .collect()
    } else {
        opts.tables.clone()
    };
    let mut tables = Vec::with_capacity(chosen.len());
    for (schema, name) in &chosen {
        let info = job
            .db
            .introspect_table(Some(schema.as_deref().unwrap_or("dbo")), name)
            .await?
            .ok_or_else(|| CoreError::Backup(format!("table {name} wasn't found")))?;
        tables.push(info);
    }

    let file = std::fs::File::create(&opts.path)?;
    let mut out = std::io::BufWriter::new(file);
    writeln!(
        out,
        "-- plusplus SQL Server script backup of {}\n-- {} table(s). Run in one session \
         (SSMS, sqlcmd, or plusplus Restore).\nSET NOCOUNT ON;\nGO",
        DbKind::SqlServer.quote_ident(&job.cfg.database),
        tables.len()
    )?;
    // Foreign keys first, so the chosen tables can be dropped in any order.
    for table in &tables {
        for fk in &table.foreign_keys {
            let fk_name = format!(
                "{}.{}",
                DbKind::SqlServer.quote_ident(table.schema.as_deref().unwrap_or("dbo")),
                DbKind::SqlServer.quote_ident(&fk.name)
            );
            writeln!(
                out,
                "IF OBJECT_ID(N'{}', N'F') IS NOT NULL ALTER TABLE {} DROP CONSTRAINT {};",
                fk_name.replace('\'', "''"),
                mssql_name(table.schema.as_deref(), &table.name),
                DbKind::SqlServer.quote_ident(&fk.name)
            )?;
        }
    }
    writeln!(out, "GO")?;
    for table in &tables {
        let name = mssql_name(table.schema.as_deref(), &table.name);
        writeln!(
            out,
            "IF OBJECT_ID(N'{}', N'U') IS NOT NULL DROP TABLE {name};",
            name.replace('\'', "''")
        )?;
    }
    writeln!(out, "GO")?;
    for table in &tables {
        writeln!(out, "{}\nGO", mssql_create_table_sql(table))?;
    }
    if !opts.schema_only {
        for table in &tables {
            if cancel.is_cancelled() {
                return Err(CoreError::Backup("Cancelled".into()));
            }
            let columns: Vec<&crate::model::ColumnInfo> = table
                .columns
                .iter()
                .filter(|c| mssql_insertable(c))
                .collect();
            if columns.is_empty() {
                continue;
            }
            let name = mssql_name(table.schema.as_deref(), &table.name);
            let list = columns
                .iter()
                .map(|c| DbKind::SqlServer.quote_ident(&c.name))
                .collect::<Vec<_>>()
                .join(", ");
            let identity = table.columns.iter().any(mssql_identity);
            if identity {
                writeln!(out, "SET IDENTITY_INSERT {name} ON;\nGO")?;
            }
            let mut sink = InsertSink {
                out: &mut out,
                prefix: format!("INSERT INTO {name} ({list}) VALUES"),
                rows: Vec::new(),
            };
            job.db
                .export_query_cancellable(
                    &format!("SELECT {list} FROM {name}"),
                    cancel.clone(),
                    &mut sink,
                )
                .await?;
            if identity {
                writeln!(out, "SET IDENTITY_INSERT {name} OFF;\nGO")?;
            }
        }
    }
    for table in &tables {
        for sql in mssql_index_sql(table) {
            writeln!(out, "{sql}\nGO")?;
        }
    }
    for table in &tables {
        for sql in mssql_fk_sql(table) {
            writeln!(out, "{sql}\nGO")?;
        }
    }
    out.flush()?;
    Ok(())
}

/// Split a script into its `GO`-separated batches (a line that is only `GO`).
fn go_batches(script: &str) -> Vec<String> {
    let mut batches = Vec::new();
    let mut current = String::new();
    for line in script.lines() {
        if line.trim().eq_ignore_ascii_case("go") {
            if !current.trim().is_empty() {
                batches.push(std::mem::take(&mut current));
            }
            current.clear();
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        batches.push(current);
    }
    batches
}

/// Run a SQL Server script on one connection in one transaction: every batch applies, or —
/// on the first error — none do (`XACT_ABORT` rolls it all back).
async fn mssql_script_restore(job: &Job, path: &Path) -> Result<()> {
    let script = std::fs::read_to_string(path)?;
    let batches = go_batches(&script);
    if batches.is_empty() {
        return Err(CoreError::Backup("the script is empty".into()));
    }
    job.db.execute_transaction(&batches).await.map(|_| ())
}

/// User tables of a SQLite schema (`main`, or an attached alias).
async fn sqlite_tables(db: &dyn Database, schema: &str) -> Result<Vec<String>> {
    let rows = db
        .execute(&format!(
            "SELECT name FROM {schema}.sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        ))
        .await?
        .rows;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.into_iter().next().map(|v| v.display()))
        .collect())
}

/// The names in `all` that aren't selected. `schema` is the schema `all` lives in, matched
/// against selections that carry one (SQLite selections have none).
fn unselected<'a>(
    all: &'a [String],
    selected: &'a [(Option<String>, String)],
    schema: Option<&'a str>,
) -> impl Iterator<Item = &'a String> + 'a {
    all.iter().filter(move |name| {
        !selected
            .iter()
            .any(|(s, t)| t == *name && (s.is_none() || schema.is_none() || s.as_deref() == schema))
    })
}

/// Drop every table the user didn't choose from the attached DuckDB copy `alias`.
async fn drop_unselected_duckdb(
    db: &dyn Database,
    alias: &str,
    selected: &[(Option<String>, String)],
) -> Result<()> {
    let rows = db
        .execute(&format!(
            "SELECT table_schema, table_name FROM information_schema.tables \
             WHERE table_catalog = '{alias}' AND table_type = 'BASE TABLE'"
        ))
        .await?
        .rows;
    for row in rows {
        let (schema, table) = (row[0].display(), row[1].display());
        let keep = selected
            .iter()
            .any(|(s, t)| *t == table && s.as_deref().is_none_or(|s| s == schema));
        if !keep {
            db.execute(&format!(
                "DROP TABLE {alias}.{}.{}",
                DbKind::DuckDb.quote_ident(&schema),
                DbKind::DuckDb.quote_ident(&table)
            ))
            .await?;
        }
    }
    Ok(())
}

/// The user tables inside a SQLite/DuckDB backup file, so a restore can say which of the
/// current tables the file doesn't have (a file restore replaces the whole database).
pub async fn file_tables(kind: DbKind, path: &Path) -> Result<Vec<String>> {
    check_database_file(kind, path)?;
    let mut cfg = ConnectionConfig::new(kind);
    match kind {
        DbKind::Sqlite => cfg.sqlite_path = path.display().to_string(),
        DbKind::DuckDb => cfg.duckdb_path = path.display().to_string(),
        _ => return Err(CoreError::Backup("not a file database".into())),
    }
    let db = crate::connect(&cfg, None, None).await?;
    if kind == DbKind::Sqlite {
        return sqlite_tables(&*db, "main").await;
    }
    let rows = db
        .execute(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_type = 'BASE TABLE' ORDER BY table_name",
        )
        .await?
        .rows;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.into_iter().next().map(|v| v.display()))
        .collect())
}

/// Restore `opts.path` into the database described by `job`. SQLite and DuckDB are
/// restored with [`restore_file`] instead, once the app has closed the database.
pub async fn restore(job: &Job, opts: &RestoreOptions, cancel: CancellationToken) -> Result<()> {
    let kind = job.cfg.kind;
    match Method::of(kind) {
        Method::Tool => {
            let tools = find_tools(kind).map_err(CoreError::Backup)?;
            let (endpoint, _tunnel) = job.endpoint().await?;
            let password = job.password.as_deref();
            let command = if kind == DbKind::Postgres {
                pg_restore_command(
                    &tools,
                    &endpoint,
                    password,
                    opts,
                    is_pg_archive(&opts.path)?,
                )
            } else {
                mysql_restore_command(&tools, &endpoint, password, opts)
            };
            run_tool(command, cancel).await
        }
        Method::Server if opts.format == DumpFormat::PlainSql => {
            mssql_script_restore(job, &opts.path).await
        }
        Method::Server => {
            let sql = mssql_restore_sql(&job.cfg.database, &opts.path.to_string_lossy());
            job.db.execute(&sql).await.map(|_| ())
        }
        Method::Builtin => Err(CoreError::Backup(
            "close the database, then restore its file".into(),
        )),
        Method::Unsupported => Err(CoreError::Backup(unsupported_reason(kind).into())),
    }
}

/// Whether a Postgres backup is a `pg_dump` archive (`PGDMP` header) rather than SQL.
pub fn is_pg_archive(path: &Path) -> Result<bool> {
    let mut header = [0u8; 5];
    let mut file = std::fs::File::open(path)?;
    let read = std::io::Read::read(&mut file, &mut header)?;
    Ok(read == 5 && &header == b"PGDMP")
}

/// Check `path` is a database file of `kind` before it replaces the real one.
fn check_database_file(kind: DbKind, path: &Path) -> Result<()> {
    let mut header = [0u8; 16];
    let mut file = std::fs::File::open(path)?;
    let read = std::io::Read::read(&mut file, &mut header)?;
    let ok = match kind {
        DbKind::Sqlite => read == 16 && &header == b"SQLite format 3\0",
        DbKind::DuckDb => read >= 12 && &header[8..12] == b"DUCK",
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(CoreError::Backup(format!(
            "{} isn't a {} database file.",
            path.display(),
            kind.label()
        )))
    }
}

/// Replace the SQLite/DuckDB file at `target` with the backup at `source`. The app must
/// have closed its connection first. The copy lands beside the target and is renamed over
/// it, so a failed copy never leaves a half-written database; stale journal/WAL files of
/// the old database are removed so they can't be replayed onto the restored one.
pub fn restore_file(kind: DbKind, source: &Path, target: &Path) -> Result<()> {
    check_database_file(kind, source)?;
    let staging = target.with_extension("plusplus-restore");
    std::fs::copy(source, &staging)?;
    let suffixes: &[&str] = if kind == DbKind::Sqlite {
        &["-wal", "-shm", "-journal"]
    } else {
        &[".wal"]
    };
    // Windows can't replace or delete a file another handle still has open, and a pool
    // closes its connections on background tasks — so right after the app drops the
    // database the old file can stay locked for a moment. Retry briefly rather than fail a
    // restore that would succeed a few milliseconds later. Elsewhere the first try is final.
    let attempts = if cfg!(windows) { 60 } else { 1 };
    let mut last = None;
    for attempt in 0..attempts {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut sidecars_gone = true;
        for suffix in suffixes {
            let mut sidecar = target.as_os_str().to_owned();
            sidecar.push(suffix);
            let sidecar = PathBuf::from(sidecar);
            // A missing sidecar is fine; one that is locked must not survive to be replayed
            // onto the restored database, so it counts as "not yet".
            match std::fs::remove_file(&sidecar) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => sidecars_gone = false,
                _ => {}
            }
        }
        if !sidecars_gone {
            last = Some(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the old journal files next to the database are still in use",
            ));
            continue;
        }
        match std::fs::rename(&staging, target) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    let _ = std::fs::remove_file(&staging);
    Err(last.map_or_else(
        || CoreError::Backup("restore failed".into()),
        CoreError::from,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> Endpoint {
        Endpoint {
            host: "db.example.com".into(),
            port: 5432,
            user: "app".into(),
            database: "sales".into(),
            ssl_mode: SslMode::Require,
            ssl_ca: String::new(),
            ssl_cert: String::new(),
            ssl_key: String::new(),
        }
    }

    fn tools(mariadb: bool) -> Tools {
        Tools {
            dump: "/bin/dump".into(),
            restore: "/bin/restore".into(),
            load: "/bin/load".into(),
            version: String::new(),
            mariadb,
        }
    }

    /// The password travels in the environment, never on the command line.
    #[test]
    fn pg_dump_keeps_the_password_off_the_command_line() {
        let opts = BackupOptions {
            path: "/tmp/sales.dump".into(),
            format: DumpFormat::Archive,
            schema_only: true,
            tables: vec![
                (Some("public".into()), "orders".into()),
                (Some("Sales".into()), "Line \"Items\"".into()),
            ],
        };
        let cmd = pg_dump_command(&tools(false), &endpoint(), Some("s3cr3t"), &opts);
        assert!(cmd.args.iter().all(|a| !a.contains("s3cr3t")));
        assert!(cmd.env.contains(&("PGPASSWORD".into(), "s3cr3t".into())));
        assert!(cmd.env.contains(&("PGSSLMODE".into(), "require".into())));
        for arg in [
            "--host=db.example.com",
            "--dbname=sales",
            "--format=custom",
            "--schema-only",
            "--file=/tmp/sales.dump",
            "--no-password",
            "--table=\"public\".\"orders\"",
            "--table=\"Sales\".\"Line \"\"Items\"\"\"",
        ] {
            assert!(cmd.args.contains(&arg.to_string()), "{arg}");
        }
    }

    #[test]
    fn pg_restore_picks_the_tool_by_file_kind() {
        let opts = RestoreOptions {
            path: "/tmp/sales.dump".into(),
            clean: true,
            format: DumpFormat::Archive,
        };
        let archive = pg_restore_command(&tools(false), &endpoint(), None, &opts, true);
        assert_eq!(archive.program, PathBuf::from("/bin/restore"));
        for arg in ["--clean", "--if-exists", "--no-owner", "/tmp/sales.dump"] {
            assert!(archive.args.contains(&arg.to_string()), "{arg}");
        }
        let script = pg_restore_command(&tools(false), &endpoint(), None, &opts, false);
        assert_eq!(script.program, PathBuf::from("/bin/load"));
        assert!(script.args.contains(&"--single-transaction".to_string()));
        assert!(script.args.contains(&"--set=ON_ERROR_STOP=1".to_string()));
        assert!(!script.args.contains(&"--clean".to_string()));
    }

    #[test]
    fn mysql_uses_an_option_file_for_the_password() {
        let opts = BackupOptions {
            path: "/tmp/sales.sql".into(),
            ..Default::default()
        };
        let cmd = mysql_dump_command(&tools(false), &endpoint(), Some("p\"a\\ss"), &opts);
        assert!(
            cmd.args[0].starts_with("--defaults-extra-file="),
            "must be first"
        );
        assert!(cmd.args.iter().all(|a| !a.contains("p\"a")));
        let (_, contents) = cmd.secret_file.as_ref().expect("option file");
        assert_eq!(contents, "[client]\npassword=\"p\\\"a\\\\ss\"\n");
        assert!(cmd.args.contains(&"--single-transaction".to_string()));
        assert!(cmd.args.contains(&"--ssl-mode=REQUIRED".to_string()));
        assert_eq!(cmd.args.last().map(String::as_str), Some("sales"));

        assert!(
            cmd.args.contains(&"--routines".to_string()),
            "whole database"
        );
        let maria = mysql_dump_command(&tools(true), &endpoint(), None, &opts);
        assert!(maria.args.contains(&"--ssl".to_string()));

        // Chosen tables follow the database name; routines stay out of a table backup.
        let some = BackupOptions {
            tables: vec![(None, "orders".into()), (None, "customers".into())],
            ..opts.clone()
        };
        let cmd = mysql_dump_command(&tools(false), &endpoint(), None, &some);
        let tail: Vec<&str> = cmd.args.iter().rev().take(3).map(String::as_str).collect();
        assert_eq!(tail, ["customers", "orders", "sales"]);
        assert!(!cmd.args.contains(&"--routines".to_string()));

        let restore = mysql_restore_command(
            &tools(false),
            &endpoint(),
            None,
            &RestoreOptions {
                path: "/tmp/sales.sql".into(),
                clean: false,
                format: DumpFormat::PlainSql,
            },
        );
        assert_eq!(restore.stdin, Some(PathBuf::from("/tmp/sales.sql")));
        assert_eq!(restore.program, PathBuf::from("/bin/load"));
    }

    #[test]
    fn sql_server_statements_quote_names_and_paths() {
        let backup = mssql_backup_sql("Sales]DB", "D:\\Backups\\o'neil.bak");
        assert_eq!(
            backup,
            "BACKUP DATABASE [Sales]]DB] TO DISK = N'D:\\Backups\\o''neil.bak' \
             WITH COPY_ONLY, INIT, CHECKSUM, NAME = N'plusplus backup';"
        );
        let restore = mssql_restore_sql("Sales", "D:\\b.bak");
        assert!(restore.starts_with("USE [master];"));
        assert!(restore.contains("SET SINGLE_USER WITH ROLLBACK IMMEDIATE"));
        assert!(restore.contains("RESTORE DATABASE [Sales] FROM DISK = N'D:\\b.bak' WITH REPLACE"));
        // Multi-user access comes back on both the failure and the success path.
        assert_eq!(restore.matches("SET MULTI_USER").count(), 2);
    }

    fn mssql_table() -> crate::model::TableInfo {
        use crate::model::{ColumnInfo, ForeignKeyInfo, IndexInfo, TableInfo};
        let col = |name: &str, data_type: &str, nullable: bool, pk: bool| ColumnInfo {
            name: name.into(),
            data_type: data_type.into(),
            nullable,
            primary_key: pk,
            default: None,
            check: None,
            comment: None,
            generated: false,
            max_length: None,
        };
        let mut id = col("id", "int", false, true);
        id.generated = true;
        let mut status = col("status", "nvarchar(20)", false, false);
        status.default = Some("(N'new')".into());
        TableInfo {
            schema: Some("sales".into()),
            name: "Orders".into(),
            columns: vec![
                id,
                col("customer_id", "int", true, false),
                status,
                col("amount", "decimal(18,2)", false, false),
                col("ver", "timestamp", false, false),
            ],
            indexes: vec![
                IndexInfo {
                    name: "PK__Orders".into(),
                    unique: true,
                    columns: vec!["id".into()],
                },
                IndexInfo {
                    name: "IX_Orders_customer".into(),
                    unique: false,
                    columns: vec!["customer_id".into()],
                },
            ],
            foreign_keys: vec![ForeignKeyInfo {
                name: "FK_Orders_Customers".into(),
                columns: vec!["customer_id".into()],
                ref_schema: Some("sales".into()),
                ref_table: "Customers".into(),
                ref_columns: vec!["id".into()],
                on_delete: "CASCADE".into(),
                on_update: "NO_ACTION".into(),
            }],
        }
    }

    #[test]
    fn sql_server_script_ddl_keeps_types_identity_and_keys() {
        let table = mssql_table();
        assert_eq!(
            mssql_create_table_sql(&table),
            "CREATE TABLE [sales].[Orders] (\n    [id] int IDENTITY(1,1) NOT NULL,\n    \
             [customer_id] int NULL,\n    [status] nvarchar(20) NOT NULL DEFAULT (N'new'),\n    \
             [amount] decimal(18,2) NOT NULL,\n    [ver] timestamp NOT NULL,\n    \
             CONSTRAINT [PK_Orders] PRIMARY KEY ([id])\n);"
        );
        // The PK's own index is part of CREATE TABLE; only the secondary one is added.
        assert_eq!(
            mssql_index_sql(&table),
            ["CREATE INDEX [IX_Orders_customer] ON [sales].[Orders] ([customer_id]);"]
        );
        assert_eq!(
            mssql_fk_sql(&table),
            [
                "ALTER TABLE [sales].[Orders] ADD CONSTRAINT [FK_Orders_Customers] FOREIGN KEY \
              ([customer_id]) REFERENCES [sales].[Customers] ([id]) ON DELETE CASCADE;"
            ]
        );
        assert!(
            !mssql_insertable(&table.columns[4]),
            "rowversion is server-filled"
        );
    }

    #[test]
    fn go_batches_split_on_go_lines_only() {
        let script = "SET NOCOUNT ON;\nGO\nINSERT INTO t VALUES (N'going');\n  go  \nSELECT 1\n";
        assert_eq!(
            go_batches(script),
            [
                "SET NOCOUNT ON;\n",
                "INSERT INTO t VALUES (N'going');\n",
                "SELECT 1\n"
            ]
        );
    }

    /// Inserts are batched at SQL Server's 1000-row limit and render Thai as N'…'.
    #[test]
    fn insert_sink_batches_rows_and_keeps_unicode() {
        use crate::export::RowSink;
        let path = std::env::temp_dir().join(format!("plusplus-sink-{}.sql", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        let mut out = std::io::BufWriter::new(file);
        {
            let mut sink = InsertSink {
                out: &mut out,
                prefix: "INSERT INTO [t] ([n], [s]) VALUES".into(),
                rows: Vec::new(),
            };
            for n in 0..1001 {
                sink.write_row(&[
                    crate::value::Value::Int(n),
                    crate::value::Value::Text("สวัสดี".into()),
                ])
                .unwrap();
            }
            sink.finish().unwrap();
        }
        drop(out);
        let script = std::fs::read_to_string(&path).unwrap();
        assert_eq!(script.matches("INSERT INTO").count(), 2, "1000 + 1");
        assert!(script.contains("(1000, N'สวัสดี');"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn methods_and_extensions_per_backend() {
        assert_eq!(Method::of(DbKind::Postgres), Method::Tool);
        assert_eq!(Method::of(DbKind::MariaDb), Method::Tool);
        assert_eq!(Method::of(DbKind::SqlServer), Method::Server);
        assert_eq!(Method::of(DbKind::Sqlite), Method::Builtin);
        assert_eq!(Method::of(DbKind::Cassandra), Method::Unsupported);
        assert_eq!(
            default_extension(DbKind::Postgres, DumpFormat::Archive),
            "dump"
        );
        assert_eq!(
            default_extension(DbKind::Postgres, DumpFormat::PlainSql),
            "sql"
        );
        assert_eq!(
            default_extension(DbKind::SqlServer, DumpFormat::Archive),
            "bak"
        );
        assert_eq!(
            default_extension(DbKind::SqlServer, DumpFormat::PlainSql),
            "sql"
        );
    }

    #[test]
    fn pg_major_reads_the_version() {
        assert_eq!(pg_major("pg_dump (PostgreSQL) 16.2"), 16);
        assert_eq!(pg_major("pg_dump (PostgreSQL) 9.6.24"), 9);
        assert_eq!(pg_major(""), 0);
    }

    #[test]
    fn restore_file_checks_the_header_and_swaps_atomically() {
        let dir = std::env::temp_dir().join(format!("plusplus-restore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("app.sqlite");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(dir.join("app.sqlite-wal"), b"stale").unwrap();

        let bogus = dir.join("notes.txt");
        std::fs::write(&bogus, b"hello").unwrap();
        assert!(restore_file(DbKind::Sqlite, &bogus, &target).is_err());
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"old",
            "untouched on refusal"
        );

        let backup = dir.join("backup.sqlite");
        let mut bytes = b"SQLite format 3\0".to_vec();
        bytes.extend_from_slice(&[7; 32]);
        std::fs::write(&backup, &bytes).unwrap();
        restore_file(DbKind::Sqlite, &backup, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert!(!dir.join("app.sqlite-wal").exists(), "stale WAL removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real SQLite round trip through the engine: back up, change the data, restore.
    #[tokio::test]
    async fn sqlite_backup_and_restore_round_trip() {
        let dir = std::env::temp_dir().join(format!("plusplus-sqlite-bk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("app.sqlite");
        let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
        cfg.sqlite_path = db_path.display().to_string();
        let db = crate::connect(&cfg, None, None).await.unwrap();
        db.execute("CREATE TABLE t (n INTEGER); INSERT INTO t VALUES (1);")
            .await
            .unwrap();

        let job = Job {
            cfg: cfg.clone(),
            password: None,
            ssh_secret: None,
            db: db.clone(),
        };

        // A table subset: the copy holds only the chosen table.
        db.execute("CREATE TABLE other (x INTEGER);").await.unwrap();
        let subset = dir.join("subset.sqlite");
        backup(
            &job,
            &BackupOptions {
                path: subset.clone(),
                tables: vec![(None, "t".into())],
                ..Default::default()
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(file_tables(DbKind::Sqlite, &subset).await.unwrap(), ["t"]);
        db.execute("DROP TABLE other;").await.unwrap();

        let backup_path = dir.join("app-backup.sqlite");
        backup(
            &job,
            &BackupOptions {
                path: backup_path.clone(),
                ..Default::default()
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        db.execute("INSERT INTO t VALUES (2);").await.unwrap();
        drop(job);
        drop(db);

        // As the app does: on a blocking thread, so this runtime stays free to run the pool's
        // background tasks that close the old file (Windows won't replace it until they do).
        {
            let (backup_path, db_path) = (backup_path.clone(), db_path.clone());
            tokio::task::spawn_blocking(move || {
                restore_file(DbKind::Sqlite, &backup_path, &db_path)
            })
            .await
            .unwrap()
            .unwrap();
        }
        let db = crate::connect(&cfg, None, None).await.unwrap();
        let rows = db.execute("SELECT count(*) FROM t").await.unwrap().rows;
        assert_eq!(
            rows[0][0].display(),
            "1",
            "the row added after the backup is gone"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live round trip against a real Postgres through the real `pg_dump`/`pg_restore`:
    /// back up, change the data, restore with `--clean`, and see the original rows. Run
    /// with `PLUSPLUS_PG_BACKUP_TEST=host:port:user:database` pointing at a scratch server.
    #[tokio::test]
    #[ignore = "needs a scratch Postgres server"]
    async fn postgres_backup_and_restore_live() {
        let Ok(spec) = std::env::var("PLUSPLUS_PG_BACKUP_TEST") else {
            return;
        };
        let parts: Vec<&str> = spec.split(':').collect();
        let mut cfg = ConnectionConfig::new(DbKind::Postgres);
        cfg.host = parts[0].into();
        cfg.port = parts[1].parse().unwrap();
        cfg.user = parts[2].into();
        cfg.database = parts[3].into();
        cfg.ssl_mode = SslMode::Disable;
        let db = crate::connect(&cfg, None, None).await.unwrap();
        db.execute("DROP TABLE IF EXISTS bk_t; CREATE TABLE bk_t (n int); INSERT INTO bk_t VALUES (1), (2);")
            .await
            .unwrap();
        let job = Job {
            cfg,
            password: None,
            ssh_secret: None,
            db: db.clone(),
        };
        let path = std::env::temp_dir().join(format!("plusplus-pg-{}.dump", std::process::id()));
        for format in [DumpFormat::Archive, DumpFormat::PlainSql] {
            backup(
                &job,
                &BackupOptions {
                    path: path.clone(),
                    format,
                    schema_only: false,
                    tables: Vec::new(),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
            assert_eq!(is_pg_archive(&path).unwrap(), format == DumpFormat::Archive);
            if format == DumpFormat::Archive {
                db.execute("DELETE FROM bk_t; INSERT INTO bk_t VALUES (99);")
                    .await
                    .unwrap();
            } else {
                // A plain script recreates the table, so it goes first.
                db.execute("DROP TABLE bk_t;").await.unwrap();
            }
            restore(
                &job,
                &RestoreOptions {
                    path: path.clone(),
                    clean: true,
                    format,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
            let rows = db
                .execute("SELECT string_agg(n::text, ',' ORDER BY n) FROM bk_t")
                .await
                .unwrap()
                .rows;
            assert_eq!(rows[0][0].display(), "1,2", "{format:?}");
        }

        // Chosen tables only: a serial table and a mixed-case name, nothing else.
        db.execute(
            "DROP TABLE IF EXISTS \"Bk Serial\", bk_other; \
             CREATE TABLE \"Bk Serial\" (id serial PRIMARY KEY, v text); \
             CREATE TABLE bk_other (x int);",
        )
        .await
        .unwrap();
        let sql_path = path.with_extension("sql");
        backup(
            &job,
            &BackupOptions {
                path: sql_path.clone(),
                format: DumpFormat::PlainSql,
                schema_only: false,
                tables: vec![
                    (Some("public".into()), "bk_t".into()),
                    (Some("public".into()), "Bk Serial".into()),
                ],
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let script = std::fs::read_to_string(&sql_path).unwrap();
        assert!(script.contains("CREATE TABLE public.bk_t"), "chosen table");
        assert!(
            script.contains("CREATE TABLE public.\"Bk Serial\""),
            "quoted name"
        );
        assert!(
            script.contains("CREATE SEQUENCE"),
            "its serial sequence comes along"
        );
        assert!(!script.contains("bk_other"), "unchosen table left out");
        let _ = std::fs::remove_file(&sql_path);
        let _ = std::fs::remove_file(&path);
    }

    /// DuckDB through the engine: `COPY FROM DATABASE` into an attached file, then a file
    /// swap back.
    #[tokio::test]
    async fn duckdb_backup_and_restore_round_trip() {
        let dir = std::env::temp_dir().join(format!("plusplus-duck-bk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("app.duckdb");
        let mut cfg = ConnectionConfig::new(DbKind::DuckDb);
        cfg.duckdb_path = db_path.display().to_string();
        let db = crate::connect(&cfg, None, None).await.unwrap();
        db.execute("CREATE TABLE t (n INTEGER)").await.unwrap();
        db.execute("INSERT INTO t VALUES (1)").await.unwrap();
        let job = Job {
            cfg: cfg.clone(),
            password: None,
            ssh_secret: None,
            db: db.clone(),
        };
        db.execute("CREATE TABLE other (x INTEGER)").await.unwrap();
        let subset = dir.join("subset.duckdb");
        backup(
            &job,
            &BackupOptions {
                path: subset.clone(),
                tables: vec![(Some("main".into()), "t".into())],
                ..Default::default()
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(file_tables(DbKind::DuckDb, &subset).await.unwrap(), ["t"]);
        db.execute("DROP TABLE other").await.unwrap();
        let backup_path = dir.join("app-backup.duckdb");
        backup(
            &job,
            &BackupOptions {
                path: backup_path.clone(),
                ..Default::default()
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        db.execute("INSERT INTO t VALUES (2)").await.unwrap();
        drop(job);
        drop(db);

        restore_file(DbKind::DuckDb, &backup_path, &db_path).unwrap();
        let db = crate::connect(&cfg, None, None).await.unwrap();
        let rows = db.execute("SELECT count(*) FROM t").await.unwrap().rows;
        assert_eq!(rows[0][0].display(), "1");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failing tool reports its stderr; a cancelled one is killed.
    #[cfg(unix)]
    #[tokio::test]
    async fn run_tool_reports_errors_and_cancels() {
        let failing = ToolCommand {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "echo 'pg_dump: error: boom' >&2; exit 1".into(),
            ],
            env: Vec::new(),
            stdin: None,
            secret_file: None,
        };
        let error = run_tool(failing, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "pg_dump: error: boom");

        let slow = ToolCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 30".into()],
            env: Vec::new(),
            stdin: None,
            secret_file: None,
        };
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            trigger.cancel();
        });
        let started = std::time::Instant::now();
        let error = run_tool(slow, cancel).await.unwrap_err();
        assert_eq!(error.to_string(), "Cancelled");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// The option file is private and gone once the tool exits.
    #[cfg(unix)]
    #[tokio::test]
    async fn option_file_is_private_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("plusplus-test-{}.cnf", std::process::id()));
        let probe = ToolCommand {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "stat -f %Lp '{0}' 2>/dev/null || stat -c %a '{0}'",
                    path.display()
                ),
            ],
            env: Vec::new(),
            stdin: None,
            secret_file: Some((path.clone(), "[client]\n".into())),
        };
        run_tool(probe, CancellationToken::new()).await.unwrap();
        assert!(!path.exists(), "removed after the run");
        let guard = write_secret_file(&path, "x").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        drop(guard);
        assert!(!path.exists());
    }
}
