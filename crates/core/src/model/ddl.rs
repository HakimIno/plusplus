//! Dialect-aware DDL input types and statement builders.

use super::{
    DbKind, ParamMode, RoutineKind, RoutineParam, TriggerEvent, TriggerLevel, TriggerTiming,
};
use serde::{Deserialize, Serialize};

// ─── DDL types ───────────────────────────────────────────────────────────────

/// Column definition for DDL operations (CREATE TABLE / ALTER TABLE ADD COLUMN).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    /// Optional DEFAULT expression rendered verbatim (e.g. `"'hello'"`, `"0"`, `"NOW()"`).
    pub default: Option<String>,
}

/// Index definition for DDL operations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

/// ON DELETE / ON UPDATE referential action for a foreign key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FkAction {
    #[default]
    NoAction,
    Cascade,
    SetNull,
    Restrict,
}

impl FkAction {
    pub const ALL: &'static [FkAction] = &[
        FkAction::NoAction,
        FkAction::Cascade,
        FkAction::SetNull,
        FkAction::Restrict,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FkAction::NoAction => "NO ACTION",
            FkAction::Cascade => "CASCADE",
            FkAction::SetNull => "SET NULL",
            FkAction::Restrict => "RESTRICT",
        }
    }

    /// Parse a backend-reported referential action ("CASCADE", "SET NULL", "SET_NULL", …).
    /// Unknown actions (e.g. SET DEFAULT, which the editor doesn't offer) map to `None`.
    pub fn from_rule(rule: &str) -> Option<FkAction> {
        match rule.trim().replace('_', " ").to_ascii_uppercase().as_str() {
            "NO ACTION" => Some(FkAction::NoAction),
            "CASCADE" => Some(FkAction::Cascade),
            "SET NULL" => Some(FkAction::SetNull),
            "RESTRICT" => Some(FkAction::Restrict),
            _ => None,
        }
    }
}

/// Foreign key constraint definition (used inside CREATE TABLE or as ADD CONSTRAINT).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForeignKeyDef {
    /// Constraint name — use an empty string to omit the `CONSTRAINT` clause.
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    pub on_delete: FkAction,
}

// ─── DDL builder helpers ─────────────────────────────────────────────────────

fn ddl_table_ref(kind: DbKind, schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(table)),
        None => kind.quote_ident(table),
    }
}

/// Render one column definition for use in CREATE TABLE.
/// `inline_pk` emits `PRIMARY KEY` inline; set to `false` for multi-column PK tables
/// (which use a trailing table-level `PRIMARY KEY (a, b)` clause instead).
fn col_def_sql(kind: DbKind, col: &ColumnDef, inline_pk: bool) -> String {
    let mut parts = vec![kind.quote_ident(&col.name), col.data_type.clone()];
    // CQL columns have no nullability and no per-column DEFAULT — every non-key column is
    // implicitly nullable and unset means null. Emitting NOT NULL/DEFAULT produces a
    // statement the server rejects, so those clauses are skipped entirely for CQL.
    if !kind.is_cql() {
        if !col.nullable {
            parts.push("NOT NULL".into());
        }
        if let Some(def) = &col.default {
            let d = def.trim();
            if !d.is_empty() {
                parts.push(format!("DEFAULT {d}"));
            }
        }
    }
    if col.primary_key && inline_pk {
        parts.push("PRIMARY KEY".into());
    }
    parts.join(" ")
}

fn fk_clause_sql(kind: DbKind, fk: &ForeignKeyDef) -> String {
    let cols = fk
        .columns
        .iter()
        .map(|c| kind.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let ref_t = kind.quote_ident(&fk.ref_table);
    let ref_c = fk
        .ref_columns
        .iter()
        .map(|c| kind.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let constraint = if fk.name.trim().is_empty() {
        String::new()
    } else {
        format!("CONSTRAINT {} ", kind.quote_ident(fk.name.trim()))
    };
    format!(
        "{constraint}FOREIGN KEY ({cols}) REFERENCES {ref_t} ({ref_c}) ON DELETE {}",
        fk.on_delete.label()
    )
}

// ─── DDL builders ────────────────────────────────────────────────────────────

/// Build a `CREATE TABLE` statement with column definitions and optional foreign keys.
pub fn build_create_table_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    columns: &[ColumnDef],
    fks: &[ForeignKeyDef],
) -> String {
    let pk_count = columns.iter().filter(|c| c.primary_key).count();
    let inline_pk = pk_count == 1;
    let mut defs: Vec<String> = columns
        .iter()
        .map(|c| col_def_sql(kind, c, inline_pk))
        .collect();
    if pk_count > 1 {
        let pk_cols = columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| kind.quote_ident(&c.name))
            .collect::<Vec<_>>()
            .join(", ");
        defs.push(format!("PRIMARY KEY ({pk_cols})"));
    }
    for fk in fks {
        defs.push(fk_clause_sql(kind, fk));
    }
    let body = defs.join(",\n    ");
    let engine = match kind {
        DbKind::MySql | DbKind::MariaDb => " ENGINE=InnoDB",
        _ => "",
    };
    format!(
        "CREATE TABLE {} (\n    {body}\n){engine};",
        ddl_table_ref(kind, schema, table)
    )
}

/// Build a `DROP TABLE` statement.
pub fn build_drop_table_sql(kind: DbKind, schema: Option<&str>, table: &str) -> String {
    format!("DROP TABLE {};", ddl_table_ref(kind, schema, table))
}

/// Build the statement that empties a table of all rows, keeping its structure.
/// SQLite has no `TRUNCATE`; it falls back to an unfiltered `DELETE`.
pub fn build_truncate_table_sql(kind: DbKind, schema: Option<&str>, table: &str) -> String {
    let tref = ddl_table_ref(kind, schema, table);
    match kind {
        DbKind::Sqlite => format!("DELETE FROM {tref};"),
        _ => format!("TRUNCATE TABLE {tref};"),
    }
}

/// Build the statement(s) that copy an existing table's structure and data into a new
/// table named `new_table` (in the same schema). Dialects diverge on how much structure
/// survives:
/// - Postgres/MySQL/MariaDB clone the full definition, then bulk-insert the rows, so
///   constraints and indexes are preserved.
/// - SQLite (`CREATE TABLE … AS SELECT`) and SQL Server (`SELECT … INTO`) copy columns and
///   data only — indexes, keys, and constraints are not carried over.
/// - CQL has neither `CREATE TABLE … LIKE` nor `INSERT … SELECT`, so cloning is not
///   possible as a statement sequence; returns an empty vec (callers hide the action).
pub fn build_clone_table_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    new_table: &str,
) -> Vec<String> {
    let src = ddl_table_ref(kind, schema, table);
    let dst = ddl_table_ref(kind, schema, new_table);
    match kind {
        DbKind::Postgres => vec![
            format!("CREATE TABLE {dst} (LIKE {src} INCLUDING ALL);"),
            format!("INSERT INTO {dst} SELECT * FROM {src};"),
        ],
        DbKind::MySql | DbKind::MariaDb => vec![
            format!("CREATE TABLE {dst} LIKE {src};"),
            format!("INSERT INTO {dst} SELECT * FROM {src};"),
        ],
        DbKind::SqlServer => vec![format!("SELECT * INTO {dst} FROM {src};")],
        DbKind::Sqlite => vec![format!("CREATE TABLE {dst} AS SELECT * FROM {src};")],
        DbKind::Cassandra | DbKind::ScyllaDb => Vec::new(),
    }
}

/// Build an `ALTER TABLE … ADD [CONSTRAINT] FOREIGN KEY` statement.
/// Not supported by SQLite (which requires a table rebuild); callers must not emit it there.
pub fn build_add_fk_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    fk: &ForeignKeyDef,
) -> String {
    format!(
        "ALTER TABLE {} ADD {};",
        ddl_table_ref(kind, schema, table),
        fk_clause_sql(kind, fk)
    )
}

/// Build the statement dropping a foreign key constraint (dialect-aware).
/// Not supported by SQLite (which requires a table rebuild); callers must not emit it there.
pub fn build_drop_fk_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    constraint: &str,
) -> String {
    let verb = match kind {
        // MySQL/MariaDB use DROP FOREIGN KEY; DROP CONSTRAINT only exists from MySQL 8.0.19.
        DbKind::MySql | DbKind::MariaDb => "DROP FOREIGN KEY",
        _ => "DROP CONSTRAINT",
    };
    format!(
        "ALTER TABLE {} {verb} {};",
        ddl_table_ref(kind, schema, table),
        kind.quote_ident(constraint)
    )
}

/// Build an `ALTER TABLE … ADD COLUMN` statement.
pub fn build_add_column_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    col: &ColumnDef,
) -> String {
    format!(
        "ALTER TABLE {} ADD COLUMN {};",
        ddl_table_ref(kind, schema, table),
        col_def_sql(kind, col, false)
    )
}

/// Build an `ALTER TABLE … DROP COLUMN` statement.
pub fn build_drop_column_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    col_name: &str,
) -> String {
    format!(
        "ALTER TABLE {} DROP COLUMN {};",
        ddl_table_ref(kind, schema, table),
        kind.quote_ident(col_name)
    )
}

/// Build `ALTER TABLE … ALTER/MODIFY COLUMN` statement(s) to change an existing column's
/// type and nullability (and, when a non-empty `default` is given, its DEFAULT).
///
/// Dialects diverge enough that this returns a list: Postgres needs one statement per aspect,
/// while MySQL restates the whole column in a single `MODIFY`. SQLite can't alter a column in
/// place — callers must guard against it (it isn't handled here). `primary_key` is ignored:
/// changing a table's primary key is a separate constraint operation, not a column alter.
pub fn build_alter_column_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    col: &ColumnDef,
) -> Vec<String> {
    let tref = ddl_table_ref(kind, schema, table);
    let c = kind.quote_ident(&col.name);
    let ty = col.data_type.trim();
    let default = col
        .default
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());

    match kind {
        DbKind::Postgres => {
            let mut out = vec![
                format!("ALTER TABLE {tref} ALTER COLUMN {c} TYPE {ty};"),
                format!(
                    "ALTER TABLE {tref} ALTER COLUMN {c} {};",
                    if col.nullable {
                        "DROP NOT NULL"
                    } else {
                        "SET NOT NULL"
                    }
                ),
            ];
            if let Some(d) = default {
                out.push(format!(
                    "ALTER TABLE {tref} ALTER COLUMN {c} SET DEFAULT {d};"
                ));
            }
            out
        }
        DbKind::MySql | DbKind::MariaDb => {
            let null = if col.nullable { "NULL" } else { "NOT NULL" };
            let def = default.map(|d| format!(" DEFAULT {d}")).unwrap_or_default();
            vec![format!(
                "ALTER TABLE {tref} MODIFY COLUMN {c} {ty} {null}{def};"
            )]
        }
        DbKind::SqlServer => {
            let null = if col.nullable { "NULL" } else { "NOT NULL" };
            let mut out = vec![format!("ALTER TABLE {tref} ALTER COLUMN {c} {ty} {null};")];
            if let Some(d) = default {
                // SQL Server attaches a DEFAULT through a (here unnamed) constraint.
                out.push(format!("ALTER TABLE {tref} ADD DEFAULT {d} FOR {c};"));
            }
            out
        }
        // SQLite has no in-place column alter; Cassandra/ScyllaDB dropped ALTER TYPE and
        // have no nullability or defaults. The caller refuses these before reaching here.
        DbKind::Sqlite | DbKind::Cassandra | DbKind::ScyllaDb => Vec::new(),
    }
}

/// Build an `ALTER TABLE … RENAME COLUMN` (or `sp_rename` for SQL Server).
pub fn build_rename_column_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    old_name: &str,
    new_name: &str,
) -> String {
    match kind {
        DbKind::SqlServer => {
            let qualified = format!("{}.{}.{}", schema.unwrap_or("dbo"), table, old_name);
            format!("EXEC sp_rename '{qualified}', '{new_name}', 'COLUMN';")
        }
        // CQL spells it without the COLUMN keyword (and only allows renaming key columns —
        // the server rejects the rest with a clear error).
        DbKind::Cassandra | DbKind::ScyllaDb => format!(
            "ALTER TABLE {} RENAME {} TO {};",
            ddl_table_ref(kind, schema, table),
            kind.quote_ident(old_name),
            kind.quote_ident(new_name)
        ),
        _ => format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {};",
            ddl_table_ref(kind, schema, table),
            kind.quote_ident(old_name),
            kind.quote_ident(new_name)
        ),
    }
}

/// Build a `CREATE [UNIQUE] INDEX` statement (dialect-aware).
pub fn build_create_index_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    idx: &IndexDef,
) -> String {
    let unique = if idx.unique { "UNIQUE " } else { "" };
    let cols = idx
        .columns
        .iter()
        .map(|c| kind.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    // MySQL/MariaDB don't support schema-qualified index names in CREATE INDEX; CQL
    // likewise names the index bare (it lands in the table's keyspace).
    let idx_ref = match kind {
        DbKind::MySql | DbKind::MariaDb | DbKind::Cassandra | DbKind::ScyllaDb => {
            kind.quote_ident(&idx.name)
        }
        _ => match schema {
            Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(&idx.name)),
            None => kind.quote_ident(&idx.name),
        },
    };
    format!(
        "CREATE {unique}INDEX {idx_ref} ON {} ({cols});",
        ddl_table_ref(kind, schema, table)
    )
}

/// Build a `DROP INDEX` statement (MySQL/SQL Server require `ON table`; others don't).
pub fn build_drop_index_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    idx_name: &str,
) -> String {
    let q = kind.quote_ident(idx_name);
    match kind {
        DbKind::MySql | DbKind::MariaDb | DbKind::SqlServer => {
            format!("DROP INDEX {q} ON {};", ddl_table_ref(kind, schema, table))
        }
        // CQL: qualify with the keyspace (the schema slot), since the connection may not
        // have a keyspace USEd while browsing across keyspaces.
        DbKind::Cassandra | DbKind::ScyllaDb => match schema {
            Some(s) => format!("DROP INDEX {}.{q};", kind.quote_ident(s)),
            None => format!("DROP INDEX {q};"),
        },
        _ => format!("DROP INDEX {q};"),
    }
}

// ─── View DDL builders ───────────────────────────────────────────────────────

/// The in-place "replace" keyword for `CREATE … VIEW`, or `""` when the dialect has none.
/// Postgres/MySQL use `OR REPLACE`; SQL Server uses `OR ALTER` (2016+); SQLite has no such
/// form (callers drop-then-create). Postgres can't `OR REPLACE` a materialized view, so
/// `materialized` suppresses it there too.
fn view_replace_kw(kind: DbKind, materialized: bool) -> &'static str {
    if materialized {
        return "";
    }
    match kind {
        DbKind::Postgres | DbKind::MySql | DbKind::MariaDb => "OR REPLACE ",
        DbKind::SqlServer => "OR ALTER ",
        // SQLite: no such form. CQL: plain views don't exist at all (materialized views
        // have their own syntax); the view editor is not offered there.
        DbKind::Sqlite | DbKind::Cassandra | DbKind::ScyllaDb => "",
    }
}

/// Build a `CREATE [OR REPLACE] [MATERIALIZED] VIEW … AS <select>` statement.
///
/// `or_replace` requests an in-place redefinition where the dialect supports one (see
/// [`view_supports_replace`]); when it doesn't, the caller must drop the view first.
/// `materialized` is Postgres-only and ignored on the other backends.
pub fn build_create_view_sql(
    kind: DbKind,
    schema: Option<&str>,
    name: &str,
    select_body: &str,
    materialized: bool,
    or_replace: bool,
) -> String {
    let vref = ddl_table_ref(kind, schema, name);
    let body = select_body.trim().trim_end_matches(';').trim_end();
    let mat = if materialized && kind == DbKind::Postgres {
        "MATERIALIZED "
    } else {
        ""
    };
    let replace = if or_replace {
        view_replace_kw(kind, materialized)
    } else {
        ""
    };
    format!("CREATE {replace}{mat}VIEW {vref} AS\n{body};")
}

/// Build a `DROP [MATERIALIZED] VIEW` statement. `materialized` is Postgres-only.
pub fn build_drop_view_sql(
    kind: DbKind,
    schema: Option<&str>,
    name: &str,
    materialized: bool,
) -> String {
    let mat = if materialized && kind == DbKind::Postgres {
        "MATERIALIZED "
    } else {
        ""
    };
    format!("DROP {mat}VIEW {};", ddl_table_ref(kind, schema, name))
}

/// Whether `kind` can redefine a view in place (so an edit is a single statement rather than
/// a drop-then-create). False for SQLite, and for Postgres materialized views.
pub fn view_supports_replace(kind: DbKind, materialized: bool) -> bool {
    !view_replace_kw(kind, materialized).is_empty()
}

// ─── Trigger DDL builders ────────────────────────────────────────────────────

/// Inputs for [`build_create_trigger_sql`]. Bundled into a struct because a trigger carries
/// far more dialect-sensitive parts than the other objects.
pub struct TriggerBuild<'a> {
    pub schema: Option<&'a str>,
    pub name: &'a str,
    pub table: &'a str,
    pub timing: TriggerTiming,
    pub events: &'a [TriggerEvent],
    pub level: TriggerLevel,
    pub when_condition: Option<&'a str>,
    /// The trigger action. For MySQL/SQLite/SQL Server this is the statement body. For
    /// Postgres it is either the name of an existing trigger function (when
    /// `pg_existing_function`) or a PL/pgSQL body wrapped in a generated `RETURNS trigger`
    /// function.
    pub body: &'a str,
    /// Postgres only: treat `body` as the name of an existing function to `EXECUTE`.
    pub pg_existing_function: bool,
}

/// Build the statement(s) creating a trigger. Postgres returns two — a backing function plus
/// the trigger — when generating a function from an inline body; the other dialects return
/// one. `Err` is returned for requests a dialect can't express (a `BEFORE` trigger on SQL
/// Server, multiple events on MySQL, an empty body, …).
pub fn build_create_trigger_sql(kind: DbKind, t: &TriggerBuild) -> Result<Vec<String>, String> {
    let name = t.name.trim();
    if name.is_empty() {
        return Err("Trigger name is required.".into());
    }
    if t.table.trim().is_empty() {
        return Err("Trigger requires a target table.".into());
    }
    if t.events.is_empty() {
        return Err("Select at least one event (INSERT / UPDATE / DELETE).".into());
    }
    let body = t.body.trim();
    let tref = ddl_table_ref(kind, t.schema, t.table);
    let nm = kind.quote_ident(name);
    let when = t.when_condition.map(str::trim).filter(|w| !w.is_empty());

    match kind {
        DbKind::Postgres => {
            let events = t
                .events
                .iter()
                .map(|e| e.label())
                .collect::<Vec<_>>()
                .join(" OR ");
            let when_clause = when.map(|w| format!("\nWHEN ({w})")).unwrap_or_default();
            let mut out = Vec::new();
            let call = if t.pg_existing_function {
                if body.is_empty() {
                    return Err("Enter the trigger function to execute.".into());
                }
                if body.ends_with(')') {
                    body.to_string()
                } else {
                    format!("{body}()")
                }
            } else {
                if body.is_empty() {
                    return Err("Enter the trigger function body.".into());
                }
                let fn_ref = match t.schema {
                    Some(s) => format!(
                        "{}.{}",
                        kind.quote_ident(s),
                        kind.quote_ident(&format!("{name}_trigfn"))
                    ),
                    None => kind.quote_ident(&format!("{name}_trigfn")),
                };
                out.push(format!(
                    "CREATE OR REPLACE FUNCTION {fn_ref}()\n\
                     RETURNS trigger LANGUAGE plpgsql AS $$\n{body}\n$$;"
                ));
                format!("{fn_ref}()")
            };
            out.push(format!(
                "CREATE TRIGGER {nm} {} {events} ON {tref}\n\
                 FOR EACH {}{when_clause}\nEXECUTE FUNCTION {call};",
                t.timing.label(),
                t.level.sql(),
            ));
            Ok(out)
        }
        DbKind::MySql | DbKind::MariaDb => {
            if t.timing == TriggerTiming::InsteadOf {
                return Err("MySQL/MariaDB have no INSTEAD OF triggers.".into());
            }
            if t.events.len() != 1 {
                return Err("A MySQL/MariaDB trigger fires on exactly one event.".into());
            }
            if body.is_empty() {
                return Err("Enter the trigger body.".into());
            }
            Ok(vec![format!(
                "CREATE TRIGGER {nm} {} {} ON {tref}\nFOR EACH ROW\n{body};",
                t.timing.label(),
                t.events[0].label(),
            )])
        }
        DbKind::Sqlite => {
            if t.events.len() != 1 {
                return Err("A SQLite trigger fires on a single event.".into());
            }
            if body.is_empty() {
                return Err("Enter the trigger body.".into());
            }
            let when_clause = when.map(|w| format!("\nWHEN ({w})")).unwrap_or_default();
            Ok(vec![format!(
                "CREATE TRIGGER {nm} {} {} ON {tref}\n\
                 FOR EACH ROW{when_clause}\nBEGIN\n{body}\nEND;",
                t.timing.label(),
                t.events[0].label(),
            )])
        }
        DbKind::SqlServer => {
            if t.timing == TriggerTiming::Before {
                return Err("SQL Server has no BEFORE triggers; use AFTER or INSTEAD OF.".into());
            }
            if body.is_empty() {
                return Err("Enter the trigger body.".into());
            }
            let events = t
                .events
                .iter()
                .map(|e| e.label())
                .collect::<Vec<_>>()
                .join(", ");
            Ok(vec![format!(
                "CREATE TRIGGER {nm} ON {tref}\n{} {events}\nAS\n{body};",
                t.timing.label(),
            )])
        }
        DbKind::Cassandra | DbKind::ScyllaDb => {
            Err("Cassandra/ScyllaDB have no triggers in CQL.".into())
        }
    }
}

/// Build a `DROP TRIGGER` statement. Postgres needs the owning `table`; the others ignore it.
pub fn build_drop_trigger_sql(
    kind: DbKind,
    schema: Option<&str>,
    name: &str,
    table: &str,
) -> String {
    let nm = kind.quote_ident(name);
    match kind {
        DbKind::Postgres => format!(
            "DROP TRIGGER {nm} ON {};",
            ddl_table_ref(kind, schema, table)
        ),
        // MySQL allows database-qualifying the trigger; SQL Server allows schema-qualifying it.
        DbKind::MySql | DbKind::MariaDb | DbKind::SqlServer => match schema {
            Some(s) => format!("DROP TRIGGER {}.{nm};", kind.quote_ident(s)),
            None => format!("DROP TRIGGER {nm};"),
        },
        // CQL has no CREATE TRIGGER, but DROP TRIGGER exists for Java triggers loaded
        // server-side; emit the plain form should one ever appear in the tree.
        DbKind::Sqlite | DbKind::Cassandra | DbKind::ScyllaDb => format!("DROP TRIGGER {nm};"),
    }
}

// ─── Routine (function / procedure) DDL builders ─────────────────────────────

/// Inputs for [`build_create_routine_sql`].
pub struct RoutineBuild<'a> {
    pub schema: Option<&'a str>,
    pub name: &'a str,
    pub kind: RoutineKind,
    pub params: &'a [RoutineParam],
    /// Return type — required for functions, ignored for procedures.
    pub return_type: Option<&'a str>,
    /// Postgres: "plpgsql" / "sql". Ignored by the other backends.
    pub language: &'a str,
    pub body: &'a str,
}

/// Format a routine's parameter list (comma-separated, no surrounding parentheses) for `kind`.
/// MySQL functions take no mode keyword; SQL Server prefixes `@` and uses `OUTPUT`.
fn routine_params_sql(kind: DbKind, is_function: bool, params: &[RoutineParam]) -> String {
    params
        .iter()
        .map(|p| {
            let ty = p.data_type.trim();
            let nm = p.name.trim();
            let default = p
                .default
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty());
            match kind {
                DbKind::Postgres => {
                    let mode = if p.mode == ParamMode::In {
                        String::new()
                    } else {
                        format!("{} ", p.mode.label())
                    };
                    let def = default.map(|d| format!(" DEFAULT {d}")).unwrap_or_default();
                    format!("{mode}{nm} {ty}{def}")
                }
                DbKind::MySql | DbKind::MariaDb => {
                    // Functions take no mode keyword; procedures may.
                    let mode = if is_function || p.mode == ParamMode::In {
                        String::new()
                    } else {
                        format!("{} ", p.mode.label())
                    };
                    format!("{mode}{nm} {ty}")
                }
                DbKind::SqlServer => {
                    let at = if nm.starts_with('@') {
                        nm.to_string()
                    } else {
                        format!("@{nm}")
                    };
                    let def = default.map(|d| format!(" = {d}")).unwrap_or_default();
                    let out = if matches!(p.mode, ParamMode::Out | ParamMode::InOut) {
                        " OUTPUT"
                    } else {
                        ""
                    };
                    format!("{at} {ty}{def}{out}")
                }
                DbKind::Sqlite | DbKind::Cassandra | DbKind::ScyllaDb => String::new(),
            }
        })
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build a `CREATE [OR REPLACE] FUNCTION|PROCEDURE` statement. SQLite has no routines and
/// returns `Err`. `or_replace` uses `OR REPLACE` on Postgres / `OR ALTER` on SQL Server; the
/// MySQL family has no portable inline replace, so the caller drops first (see the editor).
pub fn build_create_routine_sql(
    kind: DbKind,
    r: &RoutineBuild,
    or_replace: bool,
) -> Result<Vec<String>, String> {
    if kind == DbKind::Sqlite {
        return Err("SQLite has no stored functions or procedures.".into());
    }
    if kind.is_cql() {
        // CQL UDFs exist but are Java/JS snippets, disabled by default server-side, and
        // shaped nothing like this editor; introspection shows them read-only instead.
        return Err("Creating Cassandra/ScyllaDB user-defined functions is not supported.".into());
    }
    let name = r.name.trim();
    if name.is_empty() {
        return Err("Routine name is required.".into());
    }
    let body = r.body.trim();
    if body.is_empty() {
        return Err("Routine body is required.".into());
    }
    let is_fn = r.kind == RoutineKind::Function;
    let ret = r.return_type.map(str::trim).filter(|s| !s.is_empty());
    if is_fn && ret.is_none() {
        return Err("A function needs a return type.".into());
    }

    let rref = ddl_table_ref(kind, r.schema, name);
    let plist = routine_params_sql(kind, is_fn, r.params);
    let kw = r.kind.keyword();

    let sql = match kind {
        DbKind::Postgres => {
            let repl = if or_replace { "OR REPLACE " } else { "" };
            let lang = {
                let l = r.language.trim();
                if l.is_empty() {
                    "plpgsql"
                } else {
                    l
                }
            };
            let returns = ret.map(|t| format!(" RETURNS {t}")).unwrap_or_default();
            let returns = if is_fn { returns } else { String::new() };
            format!(
                "CREATE {repl}{kw} {rref}({plist}){returns}\nLANGUAGE {lang} AS $$\n{body}\n$$;"
            )
        }
        DbKind::MySql | DbKind::MariaDb => {
            let returns = if is_fn {
                format!(" RETURNS {}", ret.unwrap())
            } else {
                String::new()
            };
            format!("CREATE {kw} {rref}({plist}){returns}\n{body}")
        }
        DbKind::SqlServer => {
            let repl = if or_replace { "OR ALTER " } else { "" };
            if is_fn {
                format!(
                    "CREATE {repl}FUNCTION {rref}({plist})\nRETURNS {}\nAS\n{body};",
                    ret.unwrap()
                )
            } else {
                // SQL Server procedures list parameters without parentheses.
                let params = if plist.is_empty() {
                    String::new()
                } else {
                    format!(" {plist}")
                };
                format!("CREATE {repl}PROCEDURE {rref}{params}\nAS\n{body};")
            }
        }
        DbKind::Sqlite | DbKind::Cassandra | DbKind::ScyllaDb => unreachable!("guarded above"),
    };
    Ok(vec![sql])
}

/// Build a `DROP FUNCTION|PROCEDURE` statement. Postgres disambiguates overloads by argument
/// types (OUT parameters excluded); the others drop by name alone.
pub fn build_drop_routine_sql(
    kind: DbKind,
    schema: Option<&str>,
    name: &str,
    routine_kind: RoutineKind,
    params: &[RoutineParam],
) -> String {
    let kw = routine_kind.keyword();
    let rref = ddl_table_ref(kind, schema, name);
    match kind {
        DbKind::Postgres => {
            let types = params
                .iter()
                .filter(|p| p.mode != ParamMode::Out)
                .map(|p| p.data_type.trim().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("DROP {kw} {rref}({types});")
        }
        _ => format!("DROP {kw} {rref};"),
    }
}

/// Whether `kind` can redefine a routine in place (an edit is a single statement, not a
/// drop-then-create). True for Postgres (`OR REPLACE`) and SQL Server (`OR ALTER`).
pub fn routine_supports_replace(kind: DbKind) -> bool {
    matches!(kind, DbKind::Postgres | DbKind::SqlServer)
}
