//! Whole-database backup and restore: the dialog's state and the background job. The
//! mechanisms — client tools, server-side T-SQL, engine snapshots — live in
//! [`dbcore::backup`].

use super::*;
use dbcore::backup::{self as bk, DumpFormat, Method};

/// The open Backup / Restore dialog.
pub(super) struct BackupDialog {
    pub conn_id: String,
    pub conn_name: String,
    /// `user@host:port/db` or the file path, for the dialog header.
    pub target: String,
    pub kind: DbKind,
    /// The database's name (the file stem for SQLite/DuckDB). A restore is confirmed by
    /// typing it.
    pub database: String,
    pub restore: bool,
    pub method: Method,
    /// The client tools, for [`Method::Tool`]: found, or what to install.
    pub tools: Option<std::result::Result<bk::Tools, String>>,
    /// The local file to write/read.
    pub path: Option<std::path::PathBuf>,
    /// The file on the database server, for [`Method::Server`].
    pub server_path: String,
    pub format: DumpFormat,
    pub schema_only: bool,
    /// Drop existing objects before restoring (Postgres archives).
    pub clean: bool,
    /// The chosen restore file is a `pg_dump` archive (else a SQL script).
    pub archive: bool,
    pub confirm: String,
    pub read_only: bool,
    pub production: bool,
    pub running: Option<BackupRun>,
    /// The last run's result: a success summary or the error.
    pub outcome: Option<std::result::Result<String, String>>,
    /// Back up every table (else only `selected`).
    pub all_tables: bool,
    /// The connection's tables as `(schema, name)`, sorted.
    pub available: Vec<(Option<String>, String)>,
    pub selected: std::collections::BTreeSet<(Option<String>, String)>,
    /// Search box over `available`.
    pub table_filter: String,
    /// For a SQLite/DuckDB restore: the current tables the chosen file doesn't contain
    /// (they'd be lost, since the file replaces the whole database), or why it couldn't be
    /// read. `None` while unknown.
    pub restore_missing: Option<std::result::Result<Vec<String>, String>>,
}

pub(super) struct BackupRun {
    pub started: std::time::Instant,
    pub cancel: tokio_util::sync::CancellationToken,
    /// Only client tools can be stopped mid-way; a T-SQL BACKUP or a file swap can't.
    pub cancellable: bool,
}

impl BackupDialog {
    /// A SQL Server `.bak`, which lives on the server's disk (a SQL script is local).
    pub fn uses_server_path(&self) -> bool {
        self.method == Method::Server && self.format == DumpFormat::Archive
    }

    /// Why the Start button is disabled, or `None` when the job can run.
    pub fn blocker(&self) -> Option<String> {
        if self.running.is_some() {
            return Some("Already running".into());
        }
        if self.method == Method::Unsupported {
            return Some(bk::unsupported_reason(self.kind).into());
        }
        if let Some(Err(hint)) = &self.tools {
            return Some(hint.clone());
        }
        if self.database.trim().is_empty() {
            return Some("The connection has no database selected".into());
        }
        if self.restore && self.read_only {
            return Some("This connection is read-only".into());
        }
        let has_path = if self.uses_server_path() {
            !self.server_path.trim().is_empty()
        } else {
            self.path.is_some()
        };
        if !has_path {
            return Some(if self.restore {
                "Choose a backup file".into()
            } else {
                "Choose where to save the backup".into()
            });
        }
        if !self.restore && !self.all_tables && self.selected.is_empty() {
            return Some("Select at least one table".into());
        }
        if self.restore && self.confirm.trim() != self.database {
            return Some(format!("Type {} to confirm", self.database));
        }
        None
    }

    /// Tables can be chosen (not SQL Server, which backs up whole databases).
    pub fn can_choose_tables(&self) -> bool {
        !self.restore
            && (bk::supports_table_selection(self.kind)
                || (self.method == Method::Server && self.format == DumpFormat::PlainSql))
    }

    /// The tables to pass to the backup: empty for all.
    pub fn chosen_tables(&self) -> Vec<(Option<String>, String)> {
        if self.all_tables || !self.can_choose_tables() {
            Vec::new()
        } else {
            self.selected.iter().cloned().collect()
        }
    }

    /// `sales-2026-09-30-1415.dump`, or `sales-orders-…` for a single chosen table.
    pub fn default_file_name(&self) -> String {
        let base = if self.database.is_empty() {
            "backup".to_string()
        } else {
            self.database.clone()
        };
        let chosen = self.chosen_tables();
        let stem = match chosen.as_slice() {
            [(_, table)] => format!("{base}-{table}"),
            _ => base,
        };
        format!(
            "{}-{}.{}",
            stem,
            chrono::Local::now().format("%Y-%m-%d-%H%M"),
            bk::default_extension(self.kind, self.format)
        )
    }
}

/// The database a SQLite/DuckDB connection stands for: its file name without extension.
fn file_stem(path: &str) -> String {
    std::path::Path::new(path.trim())
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl DbGuiApp {
    /// Export from a table's context menu through the existing SQL backup flow.
    pub(super) fn open_table_dump_dialog(&mut self, conn_id: &str, table: &TableInfo) {
        let Some(conn_idx) = self.connections.iter().position(|c| c.id == conn_id) else {
            self.error = Some("Connect to this table's database first.".into());
            return;
        };
        self.open_backup_dialog(conn_idx, false);
        let Some(dialog) = self.backup_dialog.as_mut().filter(|d| d.conn_id == conn_id) else {
            return;
        };
        dialog.format = DumpFormat::PlainSql;
        dialog.all_tables = false;
        dialog.selected.clear();
        dialog
            .selected
            .insert((table.schema.clone(), table.name.clone()));
    }

    /// Open the Backup (or Restore) dialog for the saved connection `conn_idx`. The
    /// connection must be live: the server-side and engine methods run on it, and a live
    /// connection proves the saved credentials work before a long job starts.
    pub(super) fn open_backup_dialog(&mut self, conn_idx: usize, restore: bool) {
        let Some(cfg) = self.connections.get(conn_idx).cloned() else {
            return;
        };
        if !self
            .active_connections
            .iter()
            .any(|c| c.config_id == cfg.id)
        {
            self.error = Some(format!("Connect to {} first.", cfg.name));
            return;
        }
        let kind = cfg.kind;
        let method = Method::of(kind);
        let database = match kind {
            DbKind::Sqlite => file_stem(&cfg.sqlite_path),
            DbKind::DuckDb => file_stem(&cfg.duckdb_path),
            _ => cfg.database.clone(),
        };
        let mut dialog = BackupDialog {
            conn_id: cfg.id.clone(),
            conn_name: cfg.name.clone(),
            target: cfg.target_summary(),
            kind,
            database,
            restore,
            method,
            tools: (method == Method::Tool).then(|| bk::find_tools(kind)),
            path: None,
            server_path: String::new(),
            // SQL Server starts on the local script: no server-disk access needed, and
            // tables can be chosen. Postgres starts on its compact archive.
            format: if method == Method::Server {
                DumpFormat::PlainSql
            } else {
                DumpFormat::default()
            },
            schema_only: false,
            clean: false,
            archive: false,
            confirm: String::new(),
            read_only: cfg.is_read_only(),
            production: cfg.is_production(),
            running: None,
            outcome: None,
            all_tables: true,
            available: Vec::new(),
            selected: Default::default(),
            table_filter: String::new(),
            restore_missing: None,
        };
        if let Some(active) = self
            .active_connections
            .iter()
            .find(|c| c.config_id == cfg.id)
        {
            dialog.available = active
                .schema
                .tables
                .iter()
                .map(|t| (t.schema.clone(), t.name.clone()))
                .collect();
            dialog.available.sort();
        }
        // Opened from a table of this connection: that table starts ticked, ready for
        // "Selected tables".
        if let Some(source) = self
            .tabs
            .get(self.active_query_tab)
            .filter(|tab| tab.conn_id.as_deref() == Some(cfg.id.as_str()))
            .and_then(|tab| tab.edits.source.as_ref())
        {
            let key = (source.schema.clone(), source.table.clone());
            if dialog.available.contains(&key) {
                dialog.selected.insert(key);
            }
        }
        if method == Method::Server {
            // For the .bak option: a bare file name until the server says where its
            // backups live.
            let bak = DumpFormat::Archive;
            dialog.server_path = format!(
                "{}-{}.{}",
                dialog.database,
                chrono::Local::now().format("%Y-%m-%d-%H%M"),
                bk::default_extension(kind, bak)
            );
            self.fetch_mssql_backup_dir(&cfg.id);
        }
        self.backup_dialog = Some(dialog);
    }

    /// Ask SQL Server for its default backup folder; answered by `BackupDefaultDir`.
    fn fetch_mssql_backup_dir(&self, conn_id: &str) {
        let Some(db) = self
            .active_connections
            .iter()
            .find(|c| c.config_id == conn_id)
            .map(|c| c.db.clone())
        else {
            return;
        };
        let tx = self.tx.clone();
        let conn_id = conn_id.to_string();
        self.rt.spawn(async move {
            let dir = db
                .execute(bk::MSSQL_DEFAULT_BACKUP_DIR_SQL)
                .await
                .ok()
                .and_then(|result| result.rows.into_iter().next())
                .and_then(|row| row.into_iter().next())
                .filter(|value| !value.is_null())
                .map(|value| value.display());
            if let Some(dir) = dir.filter(|d| !d.trim().is_empty()) {
                let _ = tx.send(AppMessage::BackupDefaultDir { conn_id, dir });
            }
        });
    }

    /// Put the server's default folder in front of a still-bare file name.
    pub(super) fn apply_mssql_backup_dir(&mut self, conn_id: &str, dir: &str) {
        let Some(dialog) = self
            .backup_dialog
            .as_mut()
            .filter(|d| d.conn_id == conn_id && !d.restore)
        else {
            return;
        };
        let name = dialog.server_path.trim();
        if name.contains(['/', '\\']) {
            return; // the user already typed a full path
        }
        let separator = if dir.contains('\\') { '\\' } else { '/' };
        let dir = dir.trim_end_matches(['/', '\\']);
        dialog.server_path = format!("{dir}{separator}{name}");
    }

    /// Pick the local file: where to save a backup, or which backup to restore.
    pub(super) fn choose_backup_file(&mut self) {
        let Some(dialog) = self.backup_dialog.as_mut() else {
            return;
        };
        let kind = dialog.kind;
        if dialog.restore {
            let extensions: &[&str] = match kind {
                DbKind::Postgres => &["dump", "backup", "sql"],
                DbKind::MySql | DbKind::MariaDb => &["sql"],
                DbKind::Sqlite => &["sqlite", "sqlite3", "db"],
                DbKind::DuckDb => &["duckdb", "db"],
                DbKind::SqlServer => &["sql"],
                _ => &[],
            };
            let mut picker = rfd::FileDialog::new().set_title("Restore from backup");
            if !extensions.is_empty() {
                picker = picker.add_filter("Backup", extensions);
            }
            if let Some(path) = picker.pick_file() {
                dialog.archive =
                    kind == DbKind::Postgres && bk::is_pg_archive(&path).unwrap_or(false);
                dialog.path = Some(path.clone());
                dialog.outcome = None;
                dialog.restore_missing = None;
                if dialog.method == Method::Builtin {
                    // Read which tables the file holds, to warn about the ones it'd drop.
                    let (tx, conn_id) = (self.tx.clone(), dialog.conn_id.clone());
                    self.rt.spawn(async move {
                        let result = bk::file_tables(kind, &path)
                            .await
                            .map_err(|e| e.to_string());
                        let _ = tx.send(AppMessage::BackupFileTables { conn_id, result });
                    });
                }
            }
        } else {
            let extension = bk::default_extension(kind, dialog.format);
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Save backup")
                .set_file_name(dialog.default_file_name())
                .add_filter("Backup", &[extension])
                .save_file()
            {
                dialog.path = Some(path);
                dialog.outcome = None;
            }
        }
    }

    /// The chosen SQLite/DuckDB restore file's tables arrived: note which current tables it
    /// lacks.
    pub(super) fn apply_backup_file_tables(
        &mut self,
        conn_id: &str,
        result: std::result::Result<Vec<String>, String>,
    ) {
        let current: Vec<String> = self
            .active_connections
            .iter()
            .find(|c| c.config_id == conn_id)
            .map(|c| c.schema.tables.iter().map(|t| t.name.clone()).collect())
            .unwrap_or_default();
        let Some(dialog) = self
            .backup_dialog
            .as_mut()
            .filter(|d| d.conn_id == conn_id && d.restore)
        else {
            return;
        };
        dialog.restore_missing = Some(result.map(|in_file| {
            current
                .into_iter()
                .filter(|table| !in_file.contains(table))
                .collect()
        }));
    }

    /// Start the backup/restore described by the dialog, on the background runtime.
    pub(super) fn start_backup_job(&mut self) {
        let Some(dialog) = self.backup_dialog.as_ref() else {
            return;
        };
        if dialog.blocker().is_some() {
            return;
        }
        let conn_id = dialog.conn_id.clone();
        let Some(cfg) = self.connections.iter().find(|c| c.id == conn_id).cloned() else {
            return;
        };
        let Some(db) = self
            .active_connections
            .iter()
            .find(|c| c.config_id == conn_id)
            .map(|c| c.db.clone())
        else {
            if let Some(dialog) = self.backup_dialog.as_mut() {
                dialog.outcome =
                    Some(Err("The connection closed. Reconnect and try again.".into()));
            }
            return;
        };
        let (kind, method, restore) = (dialog.kind, dialog.method, dialog.restore);
        let path = if dialog.uses_server_path() {
            std::path::PathBuf::from(dialog.server_path.trim())
        } else {
            dialog.path.clone().unwrap_or_default()
        };
        let summary = format!(
            "{} {} {} {}",
            if restore { "RESTORE" } else { "BACKUP" },
            dialog.database,
            if restore { "FROM" } else { "TO" },
            path.display()
        );
        let (format, schema_only, clean) = (dialog.format, dialog.schema_only, dialog.clean);
        let tables = dialog.chosen_tables();
        let cancel = tokio_util::sync::CancellationToken::new();
        let tx = self.tx.clone();
        let started = std::time::Instant::now();
        let finish = move |result: std::result::Result<(), String>| AppMessage::BackupFinished {
            conn_id: conn_id.clone(),
            restore,
            summary: summary.clone(),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            result,
        };

        let cancellable = if restore && method == Method::Builtin {
            // SQLite/DuckDB: close the database, swap in the backup file, reconnect (on
            // `BackupFinished`). Nothing may hold the old file open while it's replaced.
            let target = std::path::PathBuf::from(if kind == DbKind::Sqlite {
                cfg.sqlite_path.trim()
            } else {
                cfg.duckdb_path.trim()
            });
            drop(db);
            self.disconnect_conn(&cfg.id);
            self.rt.spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    bk::restore_file(kind, &path, &target).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                let _ = tx.send(finish(result));
            });
            false
        } else {
            let password = if kind.is_server() {
                dbcore::secrets::get_password(&cfg.id).ok().flatten()
            } else {
                None
            };
            let ssh_secret = if cfg.ssh_enabled && kind.is_server() {
                dbcore::secrets::get_ssh_secret(&cfg.id).ok().flatten()
            } else {
                None
            };
            let job = bk::Job {
                cfg,
                password,
                ssh_secret,
                db,
            };
            let token = cancel.clone();
            self.rt.spawn(async move {
                let result = if restore {
                    bk::restore(
                        &job,
                        &bk::RestoreOptions {
                            path,
                            clean,
                            format,
                        },
                        token,
                    )
                    .await
                } else {
                    bk::backup(
                        &job,
                        &bk::BackupOptions {
                            path,
                            format,
                            schema_only,
                            tables,
                        },
                        token,
                    )
                    .await
                };
                let _ = tx.send(finish(result.map_err(|e| e.to_string())));
            });
            // Client tools and script generation stop on request; a T-SQL BACKUP/RESTORE
            // or a script restore (one transaction) runs to its end.
            method == Method::Tool
                || (method == Method::Server && format == DumpFormat::PlainSql && !restore)
        };
        if let Some(dialog) = self.backup_dialog.as_mut() {
            dialog.outcome = None;
            dialog.running = Some(BackupRun {
                started,
                cancel,
                cancellable,
            });
        }
        self.status_msg = if restore {
            "Restoring database…".into()
        } else {
            "Backing up database…".into()
        };
    }

    pub(super) fn cancel_backup_job(&mut self) {
        if let Some(run) = self
            .backup_dialog
            .as_ref()
            .and_then(|d| d.running.as_ref())
            .filter(|run| run.cancellable)
        {
            run.cancel.cancel();
        }
    }

    /// A job finished: audit it, show the outcome, and after a restore reconnect so every
    /// tab and the schema see the restored database (SQL Server's pool also left it for
    /// `master`; SQLite/DuckDB were closed for the file swap).
    pub(super) fn finish_backup_job(
        &mut self,
        conn_id: &str,
        restore: bool,
        summary: &str,
        elapsed_ms: f64,
        result: std::result::Result<(), String>,
    ) {
        let action = if restore {
            dbcore::audit::AuditAction::Restore
        } else {
            dbcore::audit::AuditAction::Backup
        };
        let production = self
            .connections
            .iter()
            .find(|c| c.id == conn_id)
            .is_some_and(|c| c.is_production());
        if restore && production && !self.audit_enabled && !cfg!(test) {
            // Replacing a production database is never unaudited, like Guardian decisions.
            let (conn_name, target) = self
                .connections
                .iter()
                .find(|c| c.id == conn_id)
                .map(|c| (c.name.clone(), c.target_summary()))
                .unwrap_or_default();
            let _ = dbcore::audit::append(&dbcore::audit::AuditEntry {
                at: dbcore::history::now_rfc3339(),
                action,
                conn_id: conn_id.to_string(),
                conn_name,
                target,
                sql: summary.to_string(),
                ok: result.is_ok(),
                error: result.as_ref().err().cloned(),
                details: None,
                rows: None,
                elapsed_ms,
            });
        } else {
            self.record_audit(
                action,
                conn_id,
                summary,
                result.is_ok(),
                result.as_ref().err().cloned(),
                None,
                elapsed_ms,
            );
        }

        let seconds = elapsed_ms / 1000.0;
        let dialog = self.backup_dialog.as_mut().filter(|d| d.conn_id == conn_id);
        let outcome = match &result {
            Ok(()) => {
                let text = match &dialog {
                    Some(d) if restore => format!("Restored {} in {seconds:.1}s.", d.database),
                    Some(d) if d.uses_server_path() => format!(
                        "Saved on the server as {} in {seconds:.1}s.",
                        d.server_path.trim()
                    ),
                    Some(d) => {
                        let size = d
                            .path
                            .as_ref()
                            .and_then(|p| std::fs::metadata(p).ok())
                            .map(|m| crate::results::value_viewer::format_bytes(m.len()))
                            .unwrap_or_default();
                        format!("Backup saved ({size}) in {seconds:.1}s.")
                    }
                    None => "Done.".into(),
                };
                self.status_msg = text.clone();
                self.error = None;
                Ok(text)
            }
            Err(error) => {
                self.status_msg = if restore {
                    "Restore failed".into()
                } else {
                    "Backup failed".into()
                };
                Err(error.clone())
            }
        };
        if let Some(dialog) = dialog {
            dialog.running = None;
            dialog.outcome = Some(outcome);
            if restore && dialog.outcome.as_ref().is_some_and(|o| o.is_ok()) {
                dialog.confirm.clear();
            }
        }
        if restore {
            if let Some(idx) = self.connections.iter().position(|c| c.id == conn_id) {
                self.start_connect(idx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(kind: DbKind, restore: bool) -> BackupDialog {
        BackupDialog {
            conn_id: "c".into(),
            conn_name: "Sales".into(),
            target: String::new(),
            kind,
            database: "sales".into(),
            restore,
            method: Method::of(kind),
            tools: None,
            path: None,
            server_path: String::new(),
            format: DumpFormat::default(),
            schema_only: false,
            clean: false,
            archive: false,
            confirm: String::new(),
            read_only: false,
            production: false,
            running: None,
            outcome: None,
            all_tables: true,
            available: vec![(None, "orders".into()), (None, "customers".into())],
            selected: Default::default(),
            table_filter: String::new(),
            restore_missing: None,
        }
    }

    /// Choosing tables: at least one, SQL Server can't, and one table names the file.
    #[test]
    fn table_selection_rules() {
        let mut d = dialog(DbKind::Postgres, false);
        d.path = Some("/tmp/x.dump".into());
        assert!(d.chosen_tables().is_empty(), "all tables by default");
        d.all_tables = false;
        assert_eq!(d.blocker().as_deref(), Some("Select at least one table"));
        d.selected.insert((None, "orders".into()));
        assert_eq!(d.blocker(), None);
        assert_eq!(d.chosen_tables(), [(None, "orders".to_string())]);
        assert!(d.default_file_name().starts_with("sales-orders-"));

        // A SQL Server .bak is whole-database; its SQL script can take chosen tables and is
        // written on this computer.
        let mut d = dialog(DbKind::SqlServer, false);
        d.all_tables = false;
        d.selected.insert((Some("dbo".into()), "orders".into()));
        assert!(d.uses_server_path() && !d.can_choose_tables());
        assert!(d.chosen_tables().is_empty(), ".bak is whole-database");
        d.format = DumpFormat::PlainSql;
        assert!(!d.uses_server_path() && d.can_choose_tables());
        assert_eq!(
            d.chosen_tables(),
            [(Some("dbo".into()), "orders".to_string())]
        );
        assert_eq!(
            d.blocker().as_deref(),
            Some("Choose where to save the backup"),
            "a local file, not a server path"
        );
        assert!(d.default_file_name().ends_with(".sql"));
    }

    #[test]
    fn table_export_opens_a_local_sql_dump_for_that_table() {
        let dir =
            std::env::temp_dir().join(format!("plusplus-table-dump-dialog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut app, idx) = app_with_sqlite(&dir);
        app.connections[idx].kind = DbKind::SqlServer;
        app.connections[idx].database = "sales".into();
        let table = TableInfo {
            schema: Some("dbo".into()),
            name: "orders".into(),
            columns: Vec::new(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        };
        app.active_connections[0].schema.tables.push(table.clone());

        app.open_table_dump_dialog("sqlite-conn", &table);
        let dialog = app.backup_dialog.as_ref().unwrap();
        assert_eq!(dialog.format, DumpFormat::PlainSql);
        assert!(!dialog.uses_server_path());
        assert_eq!(
            dialog.chosen_tables(),
            [(Some("dbo".into()), "orders".into())]
        );
        assert!(dialog.default_file_name().ends_with(".sql"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A restore runs only with a file, a writable connection and the typed name.
    #[test]
    fn restore_needs_a_file_write_access_and_the_typed_name() {
        let mut d = dialog(DbKind::Sqlite, true);
        assert_eq!(d.blocker().as_deref(), Some("Choose a backup file"));
        d.path = Some("/tmp/sales.sqlite".into());
        assert_eq!(d.blocker().as_deref(), Some("Type sales to confirm"));
        d.confirm = "Sales".into();
        assert!(d.blocker().is_some(), "case-sensitive");
        d.confirm = " sales ".into();
        assert_eq!(d.blocker(), None);
        d.read_only = true;
        assert_eq!(d.blocker().as_deref(), Some("This connection is read-only"));
    }

    #[test]
    fn backup_blockers_cover_tools_server_paths_and_unsupported() {
        let mut d = dialog(DbKind::Postgres, false);
        d.tools = Some(Err("pg_dump and pg_restore weren't found".into()));
        d.path = Some("/tmp/x.dump".into());
        assert!(d.blocker().unwrap().contains("weren't found"));

        let mut d = dialog(DbKind::SqlServer, false);
        assert_eq!(
            d.blocker().as_deref(),
            Some("Choose where to save the backup")
        );
        d.server_path = "D:\\Backups\\sales.bak".into();
        assert_eq!(d.blocker(), None, "no local file needed");

        let d = dialog(DbKind::Cassandra, false);
        assert!(d.blocker().unwrap().contains("nodetool"));
    }

    #[test]
    fn default_names_carry_the_database_and_extension() {
        let d = dialog(DbKind::Postgres, false);
        let name = d.default_file_name();
        assert!(
            name.starts_with("sales-") && name.ends_with(".dump"),
            "{name}"
        );
        let d = dialog(DbKind::SqlServer, false);
        assert!(d.default_file_name().ends_with(".bak"));
    }

    fn app_with_sqlite(dir: &std::path::Path) -> (DbGuiApp, usize) {
        let mut app = DbGuiApp::construct();
        app.connections.clear();
        app.active_connections.clear();
        let mut cfg = dbcore::ConnectionConfig::new(DbKind::Sqlite);
        cfg.id = "sqlite-conn".into();
        cfg.name = "Local".into();
        cfg.sqlite_path = dir.join("app.sqlite").display().to_string();
        let db = app
            .rt
            .block_on(dbcore::connect(&cfg, None, None))
            .expect("sqlite opens");
        app.rt
            .block_on(db.execute("CREATE TABLE t (n INTEGER); INSERT INTO t VALUES (1);"))
            .unwrap();
        app.connections.push(cfg);
        app.active_connections.push(ActiveConnection {
            config_id: "sqlite-conn".into(),
            name: "Local".into(),
            db,
            databases: Vec::new(),
            schema: SchemaTree::default(),
        });
        (app, 0)
    }

    fn wait_for_outcome(app: &mut DbGuiApp) {
        let ctx = egui::Context::default();
        for _ in 0..200 {
            app.poll_messages(&ctx);
            if app
                .backup_dialog
                .as_ref()
                .is_some_and(|d| d.outcome.is_some())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("the job never finished");
    }

    /// The whole path through the app: open the dialog on a live connection, back up,
    /// and see the outcome; the dialog needs a live connection to open at all.
    #[test]
    fn sqlite_backup_runs_through_the_dialog() {
        let dir = std::env::temp_dir().join(format!("plusplus-app-backup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut app, idx) = app_with_sqlite(&dir);

        app.open_backup_dialog(idx, false);
        let target = dir.join("app-backup.sqlite");
        {
            let dialog = app.backup_dialog.as_mut().expect("dialog opens");
            assert_eq!(dialog.database, "app");
            dialog.path = Some(target.clone());
        }
        app.start_backup_job();
        assert!(app.backup_dialog.as_ref().unwrap().running.is_some());
        wait_for_outcome(&mut app);
        let outcome = app.backup_dialog.as_ref().unwrap().outcome.clone().unwrap();
        assert!(
            outcome
                .as_ref()
                .is_ok_and(|t| t.starts_with("Backup saved")),
            "{outcome:?}"
        );
        assert!(target.exists());

        app.active_connections.clear();
        app.backup_dialog = None;
        app.open_backup_dialog(idx, false);
        assert!(app.backup_dialog.is_none(), "needs a live connection");
        assert!(app
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Connect to Local"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn render_dialog(dialog: BackupDialog, name: &str) {
        let mut app = DbGuiApp::construct();
        app.show_welcome = false;
        app.connections.clear();
        app.backup_dialog = Some(dialog);
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 640.0))
            .with_pixels_per_point(2.0)
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                app.draw(ui, None);
            });
        harness.run_steps(6);
        harness.snapshot(name);
    }

    /// Screenshot generator (ignored): a Postgres backup with its tool found.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_backup_dialog() {
        let mut d = dialog(DbKind::Postgres, false);
        d.conn_name = "Sales DB".into();
        d.target = "app@db.internal:5432/sales".into();
        d.tools = Some(Ok(bk::Tools {
            dump: "/opt/homebrew/opt/libpq/bin/pg_dump".into(),
            restore: "/opt/homebrew/opt/libpq/bin/pg_restore".into(),
            load: "/opt/homebrew/opt/libpq/bin/psql".into(),
            version: "pg_dump (PostgreSQL) 16.4".into(),
            mariadb: false,
        }));
        d.path = Some("/Users/me/Backups/sales-2026-09-30-1415.dump".into());
        d.all_tables = false;
        d.available = [
            "customers",
            "invoices",
            "invoice_lines",
            "orders",
            "products",
            "stock_moves",
        ]
        .iter()
        .map(|t| (Some("public".to_string()), t.to_string()))
        .collect();
        d.selected.insert((Some("public".into()), "orders".into()));
        d.selected
            .insert((Some("public".into()), "invoices".into()));
        render_dialog(d, "backup_dialog");
    }

    /// Screenshot generator (ignored): a SQL Server script backup of chosen tables.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_mssql_script_backup_dialog() {
        let mut d = dialog(DbKind::SqlServer, false);
        d.conn_name = "UNICRON".into();
        d.target = "sa@10.0.0.12:1433/sales".into();
        d.format = DumpFormat::PlainSql;
        d.path = Some("/Users/me/Backups/sales-2026-09-30-1415.sql".into());
        d.all_tables = false;
        d.available = ["ac_ms_account_group", "ac_ms_account_group1", "hr_ms_major"]
            .iter()
            .map(|t| (Some("dbo".to_string()), t.to_string()))
            .collect();
        d.selected
            .insert((Some("dbo".into()), "hr_ms_major".into()));
        render_dialog(d, "mssql_script_backup_dialog");
    }

    /// Screenshot generator (ignored): a production SQL Server restore, half confirmed.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_restore_dialog() {
        let mut d = dialog(DbKind::SqlServer, true);
        d.conn_name = "UNICRON".into();
        d.target = "sa@10.0.0.12:1433/sales".into();
        d.production = true;
        d.server_path = "D:\\SQLBackups\\sales-2026-09-30-1415.bak".into();
        d.confirm = "sal".into();
        render_dialog(d, "restore_dialog");
    }

    /// The server's default folder prefixes a bare file name, but never a typed path.
    #[test]
    fn mssql_default_dir_prefixes_only_a_bare_name() {
        let mut app = DbGuiApp::construct();
        let mut d = dialog(DbKind::SqlServer, false);
        d.server_path = "sales.bak".into();
        app.backup_dialog = Some(d);
        app.apply_mssql_backup_dir("c", "D:\\SQLBackups\\");
        assert_eq!(
            app.backup_dialog.as_ref().unwrap().server_path,
            "D:\\SQLBackups\\sales.bak"
        );
        app.apply_mssql_backup_dir("c", "/var/opt/mssql/data");
        assert_eq!(
            app.backup_dialog.as_ref().unwrap().server_path,
            "D:\\SQLBackups\\sales.bak",
            "a full path is left alone"
        );
    }
}
