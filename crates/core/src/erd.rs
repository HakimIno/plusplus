//! Portable ER designs and dialect-aware forward engineering.
//!
//! An [`ErDesign`] deliberately contains no connection id.  It can be exported from one
//! database, edited, saved as JSON, then rendered as DDL for any supported backend.

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use crate::{DbKind, FkAction, SchemaTree};

pub const FILE_EXTENSION: &str = "plusplus-er.json";
pub const CURRENT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErDesign {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub tables: Vec<DesignTable>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesignTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
    #[serde(default)]
    pub columns: Vec<DesignColumn>,
    #[serde(default)]
    pub indexes: Vec<DesignIndex>,
    #[serde(default)]
    pub foreign_keys: Vec<DesignForeignKey>,
    /// Optional canvas position. Kept backend-agnostic so hand-arranged layouts survive import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_y: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesignColumn {
    pub name: String,
    /// Portable SQL type. Common native spellings are normalised during forward engineering.
    pub data_type: String,
    #[serde(default = "default_true")]
    pub nullable: bool,
    #[serde(default)]
    pub primary_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesignIndex {
    pub name: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesignForeignKey {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_schema: Option<String>,
    pub ref_table: String,
    #[serde(default)]
    pub ref_columns: Vec<String>,
    #[serde(default)]
    pub on_delete: FkAction,
}

fn default_true() -> bool {
    true
}

impl ErDesign {
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            version: CURRENT_VERSION,
            name: name.into(),
            tables: Vec::new(),
        }
    }

    /// Capture the portable subset of an introspected schema. Views, routines and triggers
    /// are intentionally excluded: the ER canvas models tables and their relationships.
    pub fn from_schema(schema: &SchemaTree) -> Self {
        Self {
            version: CURRENT_VERSION,
            name: schema.database_name.clone(),
            tables: schema
                .tables
                .iter()
                .map(|table| {
                    let primary_key: Vec<&str> = table
                        .columns
                        .iter()
                        .filter(|column| column.primary_key)
                        .map(|column| column.name.as_str())
                        .collect();
                    DesignTable {
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                        columns: table
                            .columns
                            .iter()
                            .map(|column| DesignColumn {
                                name: column.name.clone(),
                                data_type: portable_type(&column.data_type),
                                nullable: column.nullable,
                                primary_key: column.primary_key,
                                // Column defaults are not part of the current introspection model.
                                default: None,
                            })
                            .collect(),
                        indexes: table
                            .indexes
                            .iter()
                            // Engines expose the index backing a PRIMARY KEY as an ordinary
                            // index too. CREATE TABLE already recreates it, so exporting it would
                            // produce a duplicate-name/duplicate-key failure on the target.
                            .filter(|index| {
                                !(index.unique
                                    && !primary_key.is_empty()
                                    && index.columns.len() == primary_key.len()
                                    && index
                                        .columns
                                        .iter()
                                        .map(String::as_str)
                                        .eq(primary_key.iter().copied()))
                            })
                            .map(|index| DesignIndex {
                                name: index.name.clone(),
                                columns: index.columns.clone(),
                                unique: index.unique,
                            })
                            .collect(),
                        foreign_keys: table
                            .foreign_keys
                            .iter()
                            .map(|fk| DesignForeignKey {
                                name: fk.name.clone(),
                                columns: fk.columns.clone(),
                                ref_schema: fk.ref_schema.clone(),
                                ref_table: fk.ref_table.clone(),
                                ref_columns: fk.ref_columns.clone(),
                                on_delete: FkAction::from_rule(&fk.on_delete).unwrap_or_default(),
                            })
                            .collect(),
                        layout_x: None,
                        layout_y: None,
                    }
                })
                .collect(),
        }
    }

    pub fn to_json_pretty(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let design: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if design.version != CURRENT_VERSION {
            return Err(format!(
                "unsupported ER design version {} (this app supports version {CURRENT_VERSION})",
                design.version
            ));
        }
        design.validate()?;
        Ok(design)
    }

    /// Validate references before a preview is generated, so a bad design never becomes a
    /// half-applied database migration.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Design name is required.".into());
        }

        let mut table_keys = HashSet::new();
        for table in &self.tables {
            if table.name.trim().is_empty() {
                return Err("Every table must have a name.".into());
            }
            let key = table_key(table.schema.as_deref(), &table.name);
            if !table_keys.insert(key) {
                return Err(format!("Duplicate table '{}'.", qualified_name(table)));
            }
            if table.columns.is_empty() {
                return Err(format!(
                    "Table '{}' needs at least one column.",
                    qualified_name(table)
                ));
            }
            let mut columns = HashSet::new();
            for column in &table.columns {
                if column.name.trim().is_empty() || column.data_type.trim().is_empty() {
                    return Err(format!(
                        "Every column in '{}' needs a name and type.",
                        qualified_name(table)
                    ));
                }
                if !columns.insert(column.name.trim().to_ascii_lowercase()) {
                    return Err(format!(
                        "Duplicate column '{}' in '{}'.",
                        column.name,
                        qualified_name(table)
                    ));
                }
            }
            for index in &table.indexes {
                if index.name.trim().is_empty() || index.columns.is_empty() {
                    return Err(format!(
                        "Every index in '{}' needs a name and at least one column.",
                        qualified_name(table)
                    ));
                }
                ensure_columns_exist(table, &index.columns, "index")?;
            }
            let mut index_names = HashSet::new();
            for index in &table.indexes {
                if !index_names.insert(index.name.trim().to_ascii_lowercase()) {
                    return Err(format!(
                        "Duplicate index '{}' in '{}'.",
                        index.name,
                        qualified_name(table)
                    ));
                }
            }
        }

        for table in &self.tables {
            for fk in &table.foreign_keys {
                if fk.columns.is_empty() || fk.columns.len() != fk.ref_columns.len() {
                    return Err(format!(
                        "Foreign key '{}' in '{}' must have the same non-zero number of source and target columns.",
                        fk.name,
                        qualified_name(table)
                    ));
                }
                ensure_columns_exist(table, &fk.columns, "foreign key")?;
                let target = self
                    .resolve_table(fk.ref_schema.as_deref(), &fk.ref_table)
                    .ok_or_else(|| {
                        format!(
                            "Foreign key '{}' in '{}' references missing or ambiguous table '{}'.",
                            fk.name,
                            qualified_name(table),
                            fk.ref_table
                        )
                    })?;
                ensure_columns_exist(target, &fk.ref_columns, "foreign key target")?;
            }
        }
        Ok(())
    }

    fn resolve_table(&self, schema: Option<&str>, name: &str) -> Option<&DesignTable> {
        if let Some(schema) = schema {
            return self.tables.iter().find(|table| {
                table.name.eq_ignore_ascii_case(name)
                    && table
                        .schema
                        .as_deref()
                        .unwrap_or("")
                        .eq_ignore_ascii_case(schema)
            });
        }
        let mut matches = self
            .tables
            .iter()
            .filter(|table| table.name.eq_ignore_ascii_case(name));
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    /// Generate a create-only migration for `kind`. If a design uses one namespace, it is
    /// remapped to `target_schema`; this is what makes a `public` design usable on `dbo`, or
    /// on an unqualified MySQL/SQLite database. Multi-schema designs retain their namespaces.
    pub fn forward_ddl(
        &self,
        kind: DbKind,
        target_schema: Option<&str>,
    ) -> Result<Vec<String>, String> {
        self.validate()?;
        let source_schemas: BTreeSet<&str> = self
            .tables
            .iter()
            .filter_map(|table| table.schema.as_deref())
            .collect();
        let remap_single = source_schemas.len() <= 1;
        let schema_for = |schema: Option<&str>| -> Option<String> {
            if kind == DbKind::Sqlite {
                None
            } else if remap_single {
                target_schema.map(str::to_string)
            } else {
                schema.map(str::to_string)
            }
        };

        let mut statements = Vec::new();
        for table in &self.tables {
            statements.push(create_table_sql(
                kind,
                schema_for(table.schema.as_deref()).as_deref(),
                table,
                kind == DbKind::Sqlite,
                &schema_for,
            )?);
        }

        for table in &self.tables {
            let schema = schema_for(table.schema.as_deref());
            for index in &table.indexes {
                statements.push(create_index_sql(kind, schema.as_deref(), table, index));
            }
        }

        // SQLite cannot ADD CONSTRAINT, so its FKs are emitted inline above. Other engines
        // get a two-phase migration, allowing forward references and cyclic relationships.
        if kind != DbKind::Sqlite {
            for table in &self.tables {
                let schema = schema_for(table.schema.as_deref());
                for fk in &table.foreign_keys {
                    statements.push(add_fk_sql(kind, schema.as_deref(), table, fk, &schema_for));
                }
            }
        }
        Ok(statements)
    }
}

fn ensure_columns_exist(table: &DesignTable, columns: &[String], role: &str) -> Result<(), String> {
    for column in columns {
        if !table
            .columns
            .iter()
            .any(|candidate| candidate.name.eq_ignore_ascii_case(column.trim()))
        {
            return Err(format!(
                "Unknown {role} column '{}' in '{}'.",
                column,
                qualified_name(table)
            ));
        }
    }
    Ok(())
}

fn table_key(schema: Option<&str>, table: &str) -> String {
    format!(
        "{}\0{}",
        schema.unwrap_or("").to_ascii_lowercase(),
        table.trim().to_ascii_lowercase()
    )
}

fn qualified_name(table: &DesignTable) -> String {
    table
        .schema
        .as_deref()
        .map(|schema| format!("{schema}.{}", table.name))
        .unwrap_or_else(|| table.name.clone())
}

fn table_ref(kind: DbKind, schema: Option<&str>, table: &str) -> String {
    schema
        .map(|schema| format!("{}.{}", kind.quote_ident(schema), kind.quote_ident(table)))
        .unwrap_or_else(|| kind.quote_ident(table))
}

fn create_table_sql<F>(
    kind: DbKind,
    schema: Option<&str>,
    table: &DesignTable,
    inline_fks: bool,
    schema_for: &F,
) -> Result<String, String>
where
    F: Fn(Option<&str>) -> Option<String>,
{
    let pk_count = table
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .count();
    let mut definitions = Vec::new();
    for column in &table.columns {
        let mut parts = vec![
            kind.quote_ident(column.name.trim()),
            dialect_type(kind, &column.data_type)?,
        ];
        if !column.nullable {
            parts.push("NOT NULL".into());
        }
        if let Some(default) = column
            .default
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            parts.push(format!("DEFAULT {default}"));
        }
        if column.primary_key && pk_count == 1 {
            parts.push("PRIMARY KEY".into());
        }
        definitions.push(parts.join(" "));
    }
    if pk_count > 1 {
        let columns = table
            .columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| kind.quote_ident(&column.name))
            .collect::<Vec<_>>()
            .join(", ");
        definitions.push(format!("PRIMARY KEY ({columns})"));
    }
    if inline_fks {
        definitions.extend(
            table
                .foreign_keys
                .iter()
                .map(|fk| fk_clause(kind, fk, schema_for)),
        );
    }
    let engine = matches!(kind, DbKind::MySql | DbKind::MariaDb)
        .then_some(" ENGINE=InnoDB")
        .unwrap_or("");
    Ok(format!(
        "CREATE TABLE {} (\n    {}\n){engine};",
        table_ref(kind, schema, &table.name),
        definitions.join(",\n    ")
    ))
}

fn create_index_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &DesignTable,
    index: &DesignIndex,
) -> String {
    let unique = if index.unique { "UNIQUE " } else { "" };
    let columns = index
        .columns
        .iter()
        .map(|column| kind.quote_ident(column))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE {unique}INDEX {} ON {} ({columns});",
        kind.quote_ident(&index.name),
        table_ref(kind, schema, &table.name)
    )
}

fn add_fk_sql<F>(
    kind: DbKind,
    schema: Option<&str>,
    table: &DesignTable,
    fk: &DesignForeignKey,
    schema_for: &F,
) -> String
where
    F: Fn(Option<&str>) -> Option<String>,
{
    format!(
        "ALTER TABLE {} ADD {};",
        table_ref(kind, schema, &table.name),
        fk_clause(kind, fk, schema_for)
    )
}

fn fk_clause<F>(kind: DbKind, fk: &DesignForeignKey, schema_for: &F) -> String
where
    F: Fn(Option<&str>) -> Option<String>,
{
    let constraint = if fk.name.trim().is_empty() {
        String::new()
    } else {
        format!("CONSTRAINT {} ", kind.quote_ident(fk.name.trim()))
    };
    let columns = fk
        .columns
        .iter()
        .map(|column| kind.quote_ident(column))
        .collect::<Vec<_>>()
        .join(", ");
    let ref_columns = fk
        .ref_columns
        .iter()
        .map(|column| kind.quote_ident(column))
        .collect::<Vec<_>>()
        .join(", ");
    let ref_schema = schema_for(fk.ref_schema.as_deref());
    format!(
        "{constraint}FOREIGN KEY ({columns}) REFERENCES {} ({ref_columns}) ON DELETE {}",
        table_ref(kind, ref_schema.as_deref(), &fk.ref_table),
        fk.on_delete.label()
    )
}

/// Convert common backend-native spellings to the portable vocabulary stored in an ER file.
pub fn portable_type(native: &str) -> String {
    let (base, args) = split_type(native);
    let canonical = match base.as_str() {
        "int2" | "smallint" | "tinyint" => "SMALLINT",
        "int" | "int4" | "integer" | "serial" => "INTEGER",
        "int8" | "bigint" | "bigserial" => "BIGINT",
        "real" | "float4" => "REAL",
        "float" | "float8" | "double" | "double precision" => "DOUBLE",
        "decimal" | "numeric" | "money" => "DECIMAL",
        "bool" | "boolean" | "bit" => "BOOLEAN",
        "char" | "character" => "CHAR",
        "varchar" | "character varying" | "nvarchar" | "nchar" => "VARCHAR",
        "text" | "tinytext" | "mediumtext" | "longtext" | "clob" => "TEXT",
        "date" => "DATE",
        "time" | "time without time zone" | "time with time zone" => "TIME",
        "timestamp"
        | "timestamp without time zone"
        | "timestamp with time zone"
        | "datetime"
        | "datetime2"
        | "smalldatetime" => "TIMESTAMP",
        "blob" | "tinyblob" | "mediumblob" | "longblob" | "bytea" | "binary" | "varbinary"
        | "image" => "BINARY",
        "uuid" | "uniqueidentifier" => "UUID",
        "json" | "jsonb" => "JSON",
        _ => return native.trim().to_ascii_uppercase(),
    };
    match (canonical, args) {
        ("BOOLEAN", _)
        | ("TEXT", _)
        | ("DATE", _)
        | ("TIME", _)
        | ("TIMESTAMP", _)
        | ("BINARY", _)
        | ("UUID", _)
        | ("JSON", _) => canonical.to_string(),
        (_, Some(args)) => format!("{canonical}({args})"),
        _ => canonical.to_string(),
    }
}

/// Render one portable type for a target engine. Unknown types are rejected instead of
/// silently producing target-specific DDL that only works on the source database.
pub fn dialect_type(kind: DbKind, portable: &str) -> Result<String, String> {
    let canonical = portable_type(portable);
    let (base, args) = split_type(&canonical);
    let args = args.as_deref();
    let rendered = match (kind, base.as_str()) {
        (DbKind::Sqlite, "smallint" | "integer" | "bigint" | "boolean") => "INTEGER".into(),
        (DbKind::Sqlite, "real" | "double") => "REAL".into(),
        (DbKind::Sqlite, "decimal") => "NUMERIC".into(),
        (DbKind::Sqlite, "binary") => "BLOB".into(),
        (DbKind::Sqlite, "char" | "varchar" | "text" | "date" | "time" | "timestamp"
            | "uuid" | "json") => "TEXT".into(),

        (DbKind::Postgres, "smallint") => "SMALLINT".into(),
        (DbKind::Postgres, "integer") => "INTEGER".into(),
        (DbKind::Postgres, "bigint") => "BIGINT".into(),
        (DbKind::Postgres, "real") => "REAL".into(),
        (DbKind::Postgres, "double") => "DOUBLE PRECISION".into(),
        (DbKind::Postgres, "decimal") => with_args("DECIMAL", args),
        (DbKind::Postgres, "boolean") => "BOOLEAN".into(),
        (DbKind::Postgres, "char") => with_args("CHAR", args),
        (DbKind::Postgres, "varchar") => with_args("VARCHAR", args),
        (DbKind::Postgres, "text") => "TEXT".into(),
        (DbKind::Postgres, "date") => "DATE".into(),
        (DbKind::Postgres, "time") => "TIME".into(),
        (DbKind::Postgres, "timestamp") => "TIMESTAMP".into(),
        (DbKind::Postgres, "binary") => "BYTEA".into(),
        (DbKind::Postgres, "uuid") => "UUID".into(),
        (DbKind::Postgres, "json") => "JSONB".into(),

        (DbKind::MySql | DbKind::MariaDb, "smallint") => "SMALLINT".into(),
        (DbKind::MySql | DbKind::MariaDb, "integer") => "INT".into(),
        (DbKind::MySql | DbKind::MariaDb, "bigint") => "BIGINT".into(),
        (DbKind::MySql | DbKind::MariaDb, "real" | "double") => "DOUBLE".into(),
        (DbKind::MySql | DbKind::MariaDb, "decimal") => with_args("DECIMAL", args),
        (DbKind::MySql | DbKind::MariaDb, "boolean") => "BOOLEAN".into(),
        (DbKind::MySql | DbKind::MariaDb, "char") => with_args("CHAR", args),
        (DbKind::MySql | DbKind::MariaDb, "varchar") => with_args_default("VARCHAR", args, "255"),
        (DbKind::MySql | DbKind::MariaDb, "text") => "TEXT".into(),
        (DbKind::MySql | DbKind::MariaDb, "date") => "DATE".into(),
        (DbKind::MySql | DbKind::MariaDb, "time") => "TIME".into(),
        (DbKind::MySql | DbKind::MariaDb, "timestamp") => "DATETIME".into(),
        (DbKind::MySql | DbKind::MariaDb, "binary") => "BLOB".into(),
        (DbKind::MySql | DbKind::MariaDb, "uuid") => "CHAR(36)".into(),
        (DbKind::MySql | DbKind::MariaDb, "json") => "JSON".into(),

        (DbKind::SqlServer, "smallint") => "SMALLINT".into(),
        (DbKind::SqlServer, "integer") => "INT".into(),
        (DbKind::SqlServer, "bigint") => "BIGINT".into(),
        (DbKind::SqlServer, "real") => "REAL".into(),
        (DbKind::SqlServer, "double") => "FLOAT".into(),
        (DbKind::SqlServer, "decimal") => with_args("DECIMAL", args),
        (DbKind::SqlServer, "boolean") => "BIT".into(),
        (DbKind::SqlServer, "char") => with_args("NCHAR", args),
        (DbKind::SqlServer, "varchar") => with_args_default("NVARCHAR", args, "255"),
        (DbKind::SqlServer, "text" | "json") => "NVARCHAR(MAX)".into(),
        (DbKind::SqlServer, "date") => "DATE".into(),
        (DbKind::SqlServer, "time") => "TIME".into(),
        (DbKind::SqlServer, "timestamp") => "DATETIME2".into(),
        (DbKind::SqlServer, "binary") => "VARBINARY(MAX)".into(),
        (DbKind::SqlServer, "uuid") => "UNIQUEIDENTIFIER".into(),
        (_, _) => {
            return Err(format!(
                "Unsupported portable type '{portable}'. Use SMALLINT, INTEGER, BIGINT, REAL, DOUBLE, DECIMAL(p,s), BOOLEAN, CHAR(n), VARCHAR(n), TEXT, DATE, TIME, TIMESTAMP, BINARY, UUID, or JSON."
            ))
        }
    };
    Ok(rendered)
}

fn with_args(base: &str, args: Option<&str>) -> String {
    args.map(|args| format!("{base}({args})"))
        .unwrap_or_else(|| base.to_string())
}

fn with_args_default(base: &str, args: Option<&str>, default: &str) -> String {
    format!("{base}({})", args.unwrap_or(default))
}

fn split_type(input: &str) -> (String, Option<String>) {
    let trimmed = input.trim();
    if let Some(open) = trimmed.find('(') {
        if trimmed.ends_with(')') {
            return (
                trimmed[..open].trim().to_ascii_lowercase(),
                Some(trimmed[open + 1..trimmed.len() - 1].trim().to_string()),
            );
        }
    }
    (trimmed.to_ascii_lowercase(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design() -> ErDesign {
        ErDesign {
            version: CURRENT_VERSION,
            name: "shop".into(),
            tables: vec![
                DesignTable {
                    schema: Some("public".into()),
                    name: "users".into(),
                    columns: vec![DesignColumn {
                        name: "id".into(),
                        data_type: "BIGINT".into(),
                        nullable: false,
                        primary_key: true,
                        default: None,
                    }],
                    indexes: vec![],
                    foreign_keys: vec![],
                    layout_x: Some(12.0),
                    layout_y: Some(34.0),
                },
                DesignTable {
                    schema: Some("public".into()),
                    name: "orders".into(),
                    columns: vec![
                        DesignColumn {
                            name: "id".into(),
                            data_type: "UUID".into(),
                            nullable: false,
                            primary_key: true,
                            default: None,
                        },
                        DesignColumn {
                            name: "user_id".into(),
                            data_type: "BIGINT".into(),
                            nullable: false,
                            primary_key: false,
                            default: None,
                        },
                    ],
                    indexes: vec![DesignIndex {
                        name: "orders_user".into(),
                        columns: vec!["user_id".into()],
                        unique: false,
                    }],
                    foreign_keys: vec![DesignForeignKey {
                        name: "orders_user_fk".into(),
                        columns: vec!["user_id".into()],
                        ref_schema: Some("public".into()),
                        ref_table: "users".into(),
                        ref_columns: vec!["id".into()],
                        on_delete: FkAction::Cascade,
                    }],
                    layout_x: None,
                    layout_y: None,
                },
            ],
        }
    }

    #[test]
    fn json_round_trip_is_versioned_and_validated() {
        let json = design().to_json_pretty().unwrap();
        assert_eq!(ErDesign::from_json(&json).unwrap(), design());
    }

    #[test]
    fn postgres_remaps_single_namespace_and_adds_fks_after_tables() {
        let ddl = design()
            .forward_ddl(DbKind::Postgres, Some("tenant_a"))
            .unwrap();
        assert!(ddl[0].contains("\"tenant_a\".\"users\""));
        assert!(ddl[1].contains("\"tenant_a\".\"orders\""));
        assert!(ddl.last().unwrap().starts_with("ALTER TABLE"));
        assert!(ddl
            .last()
            .unwrap()
            .contains("REFERENCES \"tenant_a\".\"users\""));
    }

    #[test]
    fn sqlite_inlines_fks_and_maps_types() {
        let ddl = design().forward_ddl(DbKind::Sqlite, None).unwrap();
        assert_eq!(ddl.len(), 3); // two tables and one index; no ALTER TABLE
        assert!(ddl[1].contains("\"id\" TEXT NOT NULL PRIMARY KEY"));
        assert!(ddl[1].contains("FOREIGN KEY (\"user_id\")"));
    }

    #[test]
    fn invalid_fk_is_rejected_before_ddl() {
        let mut bad = design();
        bad.tables[1].foreign_keys[0].ref_columns = vec!["missing".into()];
        assert!(bad
            .forward_ddl(DbKind::MySql, None)
            .unwrap_err()
            .contains("missing"));
    }

    #[test]
    fn portable_types_cover_all_targets() {
        assert_eq!(dialect_type(DbKind::Postgres, "json").unwrap(), "JSONB");
        assert_eq!(
            dialect_type(DbKind::MySql, "varchar(80)").unwrap(),
            "VARCHAR(80)"
        );
        assert_eq!(
            dialect_type(DbKind::SqlServer, "uuid").unwrap(),
            "UNIQUEIDENTIFIER"
        );
        assert_eq!(
            dialect_type(DbKind::Sqlite, "decimal(12,2)").unwrap(),
            "NUMERIC"
        );
    }
}
