//! Introspected database catalog types.

use super::sql::keyword_positions;
use super::DbKind;
use serde::{Deserialize, Serialize};

/// A column as introspected from the schema.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    /// Introspected DEFAULT expression, kept verbatim for display and migration previews.
    pub default: Option<String>,
    /// Column-level CHECK expression when the backend exposes one.
    pub check: Option<String>,
    /// Column comment/description when supported by the backend.
    pub comment: Option<String>,
}

/// An index on a table.
#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub columns: Vec<String>,
}

/// A foreign key as introspected from the schema.
#[derive(Debug, Clone)]
pub struct ForeignKeyInfo {
    /// Constraint name. Empty for SQLite, which doesn't expose one.
    pub name: String,
    /// Referencing columns, in constraint order; pairs positionally with `ref_columns`.
    pub columns: Vec<String>,
    /// Schema of the referenced table, where the backend qualifies it.
    pub ref_schema: Option<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    /// Referential actions as reported by the backend (e.g. "CASCADE", "NO ACTION").
    pub on_delete: String,
    pub on_update: String,
}

impl ForeignKeyInfo {
    /// Human-readable `cols → ref_table(ref_cols)` summary for tree rows and tooltips.
    pub fn display(&self) -> String {
        format!(
            "{} → {}({})",
            self.columns.join(", "),
            self.ref_table,
            self.ref_columns.join(", ")
        )
    }
}

/// A table (or view) with its columns and indexes.
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// Schema/namespace the table lives in (e.g. `public` for Postgres). `None` for SQLite.
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

impl TableInfo {
    /// Fully-qualified, quote-safe name for use in generated SQL, quoted for `kind`
    /// (backticks on MySQL/MariaDB, ANSI double quotes elsewhere).
    pub fn qualified(&self, kind: DbKind) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(&self.name)),
            None => kind.quote_ident(&self.name),
        }
    }
}

/// A view as introspected from the schema. Like a table it has columns; it also carries
/// the `SELECT` body it was defined with.
#[derive(Debug, Clone)]
pub struct ViewInfo {
    /// Schema/namespace the view lives in. `None` for SQLite.
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    /// The view's defining query (the text after `AS`), as reported by the backend. Empty
    /// when the backend won't surface it (e.g. insufficient privileges).
    pub definition: String,
    /// Postgres materialized view. Always `false` on the other backends.
    pub materialized: bool,
}

impl ViewInfo {
    /// Fully-qualified, quote-safe name for use in generated SQL, quoted for `kind`.
    pub fn qualified(&self, kind: DbKind) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(&self.name)),
            None => kind.quote_ident(&self.name),
        }
    }
}

/// Whether a routine is a function (returns a value) or a procedure (called for effect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutineKind {
    Function,
    Procedure,
}

impl RoutineKind {
    pub fn label(self) -> &'static str {
        match self {
            RoutineKind::Function => "Function",
            RoutineKind::Procedure => "Procedure",
        }
    }

    /// SQL keyword for this routine kind (`FUNCTION` / `PROCEDURE`).
    pub fn keyword(self) -> &'static str {
        match self {
            RoutineKind::Function => "FUNCTION",
            RoutineKind::Procedure => "PROCEDURE",
        }
    }
}

/// Parameter-passing mode for a routine parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ParamMode {
    #[default]
    In,
    Out,
    InOut,
    Variadic,
}

impl ParamMode {
    pub const ALL: &'static [ParamMode] = &[
        ParamMode::In,
        ParamMode::Out,
        ParamMode::InOut,
        ParamMode::Variadic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ParamMode::In => "IN",
            ParamMode::Out => "OUT",
            ParamMode::InOut => "INOUT",
            ParamMode::Variadic => "VARIADIC",
        }
    }

    /// Parse a backend-reported parameter mode ("IN", "OUT", "INOUT", "IN OUT", "VARIADIC").
    pub fn from_keyword(s: &str) -> ParamMode {
        match s.trim().replace('_', " ").to_ascii_uppercase().as_str() {
            "OUT" => ParamMode::Out,
            "INOUT" | "IN OUT" => ParamMode::InOut,
            "VARIADIC" => ParamMode::Variadic,
            _ => ParamMode::In,
        }
    }
}

/// A single routine parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutineParam {
    pub name: String,
    pub data_type: String,
    pub mode: ParamMode,
    /// Optional default expression, rendered verbatim. `None` when the parameter has none.
    pub default: Option<String>,
}

/// A stored function or procedure as introspected from the schema.
#[derive(Debug, Clone)]
pub struct RoutineInfo {
    /// Schema/namespace the routine lives in. `None` for SQLite (which has no routines).
    pub schema: Option<String>,
    pub name: String,
    pub kind: RoutineKind,
    pub params: Vec<RoutineParam>,
    /// Return type for functions; `None` for procedures.
    pub return_type: Option<String>,
    /// Implementation language (Postgres: "plpgsql"/"sql"; often empty elsewhere).
    pub language: String,
    /// The routine body / full definition as reported by the backend. May be empty when the
    /// backend won't surface it (e.g. insufficient privileges).
    pub body: String,
}

impl RoutineInfo {
    /// Compact `name(mode arg type, …) → ret` signature for tree rows and tooltips.
    pub fn signature(&self) -> String {
        let params = self
            .params
            .iter()
            .map(|p| {
                let mode = if p.mode == ParamMode::In {
                    String::new()
                } else {
                    format!("{} ", p.mode.label())
                };
                format!("{mode}{} {}", p.name, p.data_type)
                    .trim()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(", ");
        match &self.return_type {
            Some(ret) => format!("{}({params}) → {ret}", self.name),
            None => format!("{}({params})", self.name),
        }
    }

    /// Fully-qualified, quote-safe name for generated SQL, quoted for `kind`.
    pub fn qualified(&self, kind: DbKind) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(&self.name)),
            None => kind.quote_ident(&self.name),
        }
    }
}

/// When a trigger fires relative to the triggering statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerTiming {
    #[default]
    Before,
    After,
    InsteadOf,
}

impl TriggerTiming {
    pub const ALL: &'static [TriggerTiming] = &[
        TriggerTiming::Before,
        TriggerTiming::After,
        TriggerTiming::InsteadOf,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriggerTiming::Before => "BEFORE",
            TriggerTiming::After => "AFTER",
            TriggerTiming::InsteadOf => "INSTEAD OF",
        }
    }

    pub fn from_keyword(s: &str) -> Option<TriggerTiming> {
        match s.trim().replace('_', " ").to_ascii_uppercase().as_str() {
            "BEFORE" => Some(TriggerTiming::Before),
            "AFTER" => Some(TriggerTiming::After),
            "INSTEAD OF" => Some(TriggerTiming::InsteadOf),
            _ => None,
        }
    }
}

/// A data-modification event a trigger can fire on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerEvent {
    Insert,
    Update,
    Delete,
}

impl TriggerEvent {
    pub const ALL: &'static [TriggerEvent] = &[
        TriggerEvent::Insert,
        TriggerEvent::Update,
        TriggerEvent::Delete,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriggerEvent::Insert => "INSERT",
            TriggerEvent::Update => "UPDATE",
            TriggerEvent::Delete => "DELETE",
        }
    }

    pub fn from_keyword(s: &str) -> Option<TriggerEvent> {
        match s.trim().to_ascii_uppercase().as_str() {
            "INSERT" => Some(TriggerEvent::Insert),
            "UPDATE" => Some(TriggerEvent::Update),
            "DELETE" => Some(TriggerEvent::Delete),
            _ => None,
        }
    }
}

/// Whether a trigger fires once per affected row or once per statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerLevel {
    #[default]
    Row,
    Statement,
}

impl TriggerLevel {
    pub fn label(self) -> &'static str {
        match self {
            TriggerLevel::Row => "FOR EACH ROW",
            TriggerLevel::Statement => "FOR EACH STATEMENT",
        }
    }

    /// The bare granularity keyword (`ROW` / `STATEMENT`) following `FOR EACH`.
    pub fn sql(self) -> &'static str {
        match self {
            TriggerLevel::Row => "ROW",
            TriggerLevel::Statement => "STATEMENT",
        }
    }
}

/// A trigger as introspected from the schema.
#[derive(Debug, Clone)]
pub struct TriggerInfo {
    /// Schema/namespace the trigger lives in. `None` for SQLite.
    pub schema: Option<String>,
    pub name: String,
    /// Table the trigger is attached to.
    pub table: String,
    pub timing: TriggerTiming,
    /// Events the trigger fires on, in declaration order (MySQL allows only one).
    pub events: Vec<TriggerEvent>,
    pub level: TriggerLevel,
    /// `WHEN (...)` guard condition, if any.
    pub when_condition: Option<String>,
    /// The action body — an inline statement block, or for Postgres an
    /// `EXECUTE FUNCTION fn(...)` clause. For SQLite this is the full stored `CREATE TRIGGER`
    /// text, the only form the backend exposes.
    pub action: String,
}

impl TriggerInfo {
    /// Human-readable `BEFORE INSERT ON table` summary for tree rows and tooltips.
    pub fn display(&self) -> String {
        let events = self
            .events
            .iter()
            .map(|e| e.label())
            .collect::<Vec<_>>()
            .join(" OR ");
        format!("{} {} ON {}", self.timing.label(), events, self.table)
    }
}

/// Best-effort extraction of `(timing, events, level, when_condition)` from a full
/// `CREATE TRIGGER` definition — as returned by `pg_get_triggerdef`, SQL Server's
/// `OBJECT_DEFINITION`, or SQLite's `sqlite_master.sql`. Only the *header* (everything
/// before the action clause: `WHEN` / `EXECUTE` / `BEGIN` / `AS`) is scanned, so DML
/// keywords inside the trigger body are never mistaken for the trigger's own events.
/// Reuses [`keyword_positions`], which already skips string literals, comments, and quoted
/// identifiers. Callers that know their dialect's firing granularity (SQLite = row,
/// SQL Server = statement) may override the returned `level`.
pub fn parse_trigger_header(
    def: &str,
) -> (
    TriggerTiming,
    Vec<TriggerEvent>,
    TriggerLevel,
    Option<String>,
) {
    let first = |kw: &str| keyword_positions(def, kw).first().copied();
    let when_at = first("WHEN");
    let action_at = ["EXECUTE", "BEGIN", "AS"]
        .iter()
        .filter_map(|kw| first(kw))
        .min();
    let header_end = [when_at, action_at]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(def.len());
    let header = &def[..header_end];

    let has = |kw: &str| !keyword_positions(header, kw).is_empty();
    let timing = if has("INSTEAD") {
        TriggerTiming::InsteadOf
    } else if has("AFTER") {
        TriggerTiming::After
    } else {
        TriggerTiming::Before
    };
    let events = TriggerEvent::ALL
        .iter()
        .copied()
        .filter(|e| has(e.label()))
        .collect();
    let level = if has("STATEMENT") {
        TriggerLevel::Statement
    } else {
        TriggerLevel::Row
    };
    let when_condition = when_at.and_then(|w| {
        let stop = action_at.unwrap_or(def.len());
        let cond = def.get(w + "WHEN".len()..stop)?.trim();
        (!cond.is_empty()).then(|| cond.to_string())
    });
    (timing, events, level, when_condition)
}

/// Extract the defining `SELECT` from a `CREATE VIEW … AS <select>` statement: the trimmed
/// text after the first top-level `AS` keyword, or the whole input when there's no such
/// separator. Normalises SQL Server's and SQLite's full `CREATE VIEW` text down to the query
/// body (Postgres and MySQL already report only the body). Skips literals/comments via
/// [`keyword_positions`].
pub fn select_body_after_as(create_sql: &str) -> String {
    match keyword_positions(create_sql, "AS").first() {
        Some(&pos) => create_sql[pos + "AS".len()..].trim().to_string(),
        None => create_sql.trim().to_string(),
    }
}

/// The full introspected schema of a connected database.
#[derive(Debug, Clone, Default)]
pub struct SchemaTree {
    pub database_name: String,
    pub tables: Vec<TableInfo>,
    pub views: Vec<ViewInfo>,
    pub routines: Vec<RoutineInfo>,
    pub triggers: Vec<TriggerInfo>,
}
