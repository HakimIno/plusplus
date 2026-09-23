//! Schema editor state — no egui drawing here; all rendering is in panels.rs.

use dbcore::{
    build_add_column_sql, build_add_fk_sql, build_alter_column_sql, build_create_index_sql,
    build_create_routine_sql, build_create_table_sql, build_create_trigger_sql,
    build_create_view_sql, build_drop_column_sql, build_drop_fk_sql, build_drop_index_sql,
    build_drop_routine_sql, build_drop_trigger_sql, build_drop_view_sql, build_rename_column_sql,
    routine_supports_replace, select_body_after_as, view_supports_replace, ColumnDef, DbKind,
    FkAction, ForeignKeyDef, ForeignKeyInfo, IndexDef, ParamMode, RoutineBuild, RoutineInfo,
    RoutineKind, RoutineParam, TableInfo, TriggerBuild, TriggerEvent, TriggerInfo, TriggerLevel,
    TriggerTiming, ViewInfo,
};

// ─── Draft types (UI working copies) ────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ColumnDraft {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub default: String,
    /// Introspected column metadata. SQL Server exposes both as editable Structure cells;
    /// other providers keep them read-only until their dialect-specific DDL is implemented.
    pub check: String,
    pub comment: String,
    /// Only set for existing columns (ALTER TABLE context).
    pub original_name: Option<String>,
    /// Type/nullability as introspected, kept so an edit to them can be detected and turned
    /// into an `ALTER COLUMN`. `None` for newly added columns.
    pub original_type: Option<String>,
    pub original_nullable: Option<bool>,
    pub original_primary_key: Option<bool>,
    pub original_default: Option<String>,
    pub original_check: Option<String>,
    pub original_comment: Option<String>,
    /// Whether the column existed before editing (vs. being newly added).
    pub is_existing: bool,
    /// Mark for deletion (existing column will get DROP COLUMN).
    pub drop: bool,
}

impl ColumnDraft {
    pub fn new_empty() -> Self {
        Self {
            name: String::new(),
            data_type: "TEXT".into(),
            nullable: true,
            primary_key: false,
            default: String::new(),
            check: String::new(),
            comment: String::new(),
            original_name: None,
            original_type: None,
            original_nullable: None,
            original_primary_key: None,
            original_default: None,
            original_check: None,
            original_comment: None,
            is_existing: false,
            drop: false,
        }
    }

    pub fn from_existing(column: &dbcore::ColumnInfo) -> Self {
        Self {
            name: column.name.clone(),
            data_type: column.data_type.clone(),
            nullable: column.nullable,
            primary_key: column.primary_key,
            default: column.default.clone().unwrap_or_default(),
            check: column.check.clone().unwrap_or_default(),
            comment: column.comment.clone().unwrap_or_default(),
            original_name: Some(column.name.clone()),
            original_type: Some(column.data_type.clone()),
            original_nullable: Some(column.nullable),
            original_primary_key: Some(column.primary_key),
            original_default: Some(column.default.clone().unwrap_or_default()),
            original_check: Some(column.check.clone().unwrap_or_default()),
            original_comment: Some(column.comment.clone().unwrap_or_default()),
            is_existing: true,
            drop: false,
        }
    }

    /// True when an existing column's type, nullability, or (a newly entered) default differs
    /// from how it was introspected — i.e. it needs an `ALTER COLUMN`.
    pub fn is_altered(&self) -> bool {
        if !self.is_existing {
            return false;
        }
        let type_changed = self
            .original_type
            .as_deref()
            .is_some_and(|t| !t.eq_ignore_ascii_case(self.data_type.trim()));
        let null_changed = self.original_nullable.is_some_and(|n| n != self.nullable);
        let default_changed = self
            .original_default
            .as_deref()
            .is_some_and(|value| value.trim() != self.default.trim());
        type_changed || null_changed || default_changed
    }

    pub fn check_changed(&self) -> bool {
        self.is_existing
            && self.original_check.as_deref().unwrap_or_default().trim() != self.check.trim()
    }

    pub fn comment_changed(&self) -> bool {
        self.is_existing
            && self.original_comment.as_deref().unwrap_or_default().trim() != self.comment.trim()
    }

    pub fn to_def(&self) -> ColumnDef {
        ColumnDef {
            name: self.name.clone(),
            data_type: self.data_type.clone(),
            nullable: self.nullable,
            primary_key: self.primary_key,
            default: if self.default.trim().is_empty() {
                None
            } else {
                Some(self.default.clone())
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct IndexDraft {
    pub name: String,
    /// Space-separated column names (editable as a single string).
    pub columns_raw: String,
    pub unique: bool,
    original_name: Option<String>,
    original_columns_raw: Option<String>,
    original_unique: Option<bool>,
    pub is_existing: bool,
    pub drop: bool,
}

impl IndexDraft {
    pub fn new_empty() -> Self {
        Self {
            name: String::new(),
            columns_raw: String::new(),
            unique: false,
            original_name: None,
            original_columns_raw: None,
            original_unique: None,
            is_existing: false,
            drop: false,
        }
    }

    pub fn from_existing(name: &str, columns: &[String], unique: bool) -> Self {
        Self {
            name: name.into(),
            columns_raw: columns.join(", "),
            unique,
            original_name: Some(name.into()),
            original_columns_raw: Some(columns.join(", ")),
            original_unique: Some(unique),
            is_existing: true,
            drop: false,
        }
    }

    pub fn is_altered(&self) -> bool {
        self.is_existing
            && (self.original_name.as_deref() != Some(self.name.trim())
                || self.original_columns_raw.as_deref() != Some(self.columns_raw.trim())
                || self.original_unique != Some(self.unique))
    }

    fn original_name(&self) -> &str {
        self.original_name.as_deref().unwrap_or(&self.name)
    }

    pub fn columns(&self) -> Vec<String> {
        self.columns_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn to_def(&self) -> IndexDef {
        IndexDef {
            name: self.name.clone(),
            columns: self.columns(),
            unique: self.unique,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FkDraft {
    pub constraint_name: String,
    pub columns_raw: String,
    pub ref_table: String,
    /// Preserved for portable multi-schema designs; the compact UI edits the table name only.
    pub ref_schema: Option<String>,
    pub ref_columns_raw: String,
    pub on_delete: FkAction,
    pub is_existing: bool,
    pub drop: bool,
}

impl FkDraft {
    pub fn new_empty() -> Self {
        Self {
            constraint_name: String::new(),
            columns_raw: String::new(),
            ref_table: String::new(),
            ref_schema: None,
            ref_columns_raw: String::new(),
            on_delete: FkAction::NoAction,
            is_existing: false,
            drop: false,
        }
    }

    pub fn from_existing(fk: &ForeignKeyInfo) -> Self {
        Self {
            constraint_name: fk.name.clone(),
            columns_raw: fk.columns.join(", "),
            ref_table: fk.ref_table.clone(),
            ref_schema: fk.ref_schema.clone(),
            ref_columns_raw: fk.ref_columns.join(", "),
            // SET DEFAULT and other actions the editor doesn't offer display as NO ACTION;
            // existing FKs are only ever dropped wholesale, so this is cosmetic.
            on_delete: FkAction::from_rule(&fk.on_delete).unwrap_or_default(),
            is_existing: true,
            drop: false,
        }
    }

    fn split_cols(raw: &str) -> Vec<String> {
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn to_def(&self) -> ForeignKeyDef {
        ForeignKeyDef {
            name: self.constraint_name.clone(),
            columns: Self::split_cols(&self.columns_raw),
            ref_table: self.ref_table.clone(),
            ref_columns: Self::split_cols(&self.ref_columns_raw),
            on_delete: self.on_delete,
        }
    }
}

// ─── Editor mode ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaEditorMode {
    New,
    Edit,
    DesignNew,
    DesignEdit,
}

// ─── Tab ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaTab {
    Columns,
    Indexes,
    ForeignKeys,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaGridSelection {
    pub tab: SchemaTab,
    pub row: usize,
}

// ─── Main editor struct ──────────────────────────────────────────────────────

pub struct SchemaEditor {
    pub mode: SchemaEditorMode,
    pub table_name: String,
    pub schema_name: String,
    pub db_kind: DbKind,

    pub columns: Vec<ColumnDraft>,
    pub indexes: Vec<IndexDraft>,
    pub fks: Vec<FkDraft>,

    pub active_tab: SchemaTab,
    pub grid_selection: Option<SchemaGridSelection>,
    pub focus_selected_cell: bool,
    /// The single Structure data-type cell currently in direct text-edit mode.
    pub editing_type_row: Option<usize>,
    /// Live, case-insensitive filter for the embedded Structure column grid.
    pub column_filter: String,
    /// Original table name when editing, used to detect and build a rename.
    pub original_table_name: Option<String>,
}

fn sqlserver_string(value: &str) -> String {
    format!("N'{}'", value.replace('\'', "''"))
}

fn sqlserver_set_column_check(
    schema: &str,
    table: &str,
    column: &str,
    expression: &str,
) -> Vec<String> {
    let kind = DbKind::SqlServer;
    let table_ref = format!("{}.{}", kind.quote_ident(schema), kind.quote_ident(table));
    let dynamic_table_ref = table_ref.replace('\'', "''");
    let object_name = sqlserver_string(&table_ref);
    let column_value = sqlserver_string(column);
    let drop_existing = format!(
        "DECLARE @check_name sysname;\n\
         SELECT @check_name = cc.name\n\
         FROM sys.check_constraints cc\n\
         JOIN sys.columns c ON c.object_id = cc.parent_object_id AND c.column_id = cc.parent_column_id\n\
         WHERE cc.parent_object_id = OBJECT_ID({object_name}) AND c.name = {column_value};\n\
         IF @check_name IS NOT NULL\n\
           EXEC(N'ALTER TABLE {dynamic_table_ref} DROP CONSTRAINT ' + QUOTENAME(@check_name));"
    );
    let mut statements = vec![drop_existing];
    if !expression.trim().is_empty() {
        let constraint = format!("CK_{table}_{column}")
            .chars()
            .take(128)
            .collect::<String>();
        statements.push(format!(
            "ALTER TABLE {table_ref} ADD CONSTRAINT {} CHECK ({});",
            kind.quote_ident(&constraint),
            expression.trim()
        ));
    }
    statements
}

fn rename_table_sql(
    kind: DbKind,
    schema: Option<&str>,
    old_name: &str,
    new_name: &str,
) -> Result<String, String> {
    let old_ref = match schema {
        Some(schema) => format!(
            "{}.{}",
            kind.quote_ident(schema),
            kind.quote_ident(old_name)
        ),
        None => kind.quote_ident(old_name),
    };
    let new_ref = match schema {
        Some(schema) => format!(
            "{}.{}",
            kind.quote_ident(schema),
            kind.quote_ident(new_name)
        ),
        None => kind.quote_ident(new_name),
    };
    match kind {
        DbKind::SqlServer => Ok(format!(
            "EXEC sp_rename {}, {};",
            sqlserver_string(&old_ref),
            sqlserver_string(new_name)
        )),
        DbKind::MySql | DbKind::MariaDb => Ok(format!("RENAME TABLE {old_ref} TO {new_ref};")),
        DbKind::Cassandra | DbKind::ScyllaDb => {
            Err("Renaming an existing CQL table is not supported.".into())
        }
        _ => Ok(format!(
            "ALTER TABLE {old_ref} RENAME TO {};",
            kind.quote_ident(new_name)
        )),
    }
}

fn sqlserver_replace_primary_key(schema: &str, table: &str, columns: &[String]) -> Vec<String> {
    let kind = DbKind::SqlServer;
    let table_ref = format!("{}.{}", kind.quote_ident(schema), kind.quote_ident(table));
    let dynamic_table_ref = table_ref.replace('\'', "''");
    let object_name = sqlserver_string(&table_ref);
    let drop_existing = format!(
        "DECLARE @pk_name sysname;\n\
         SELECT @pk_name = kc.name\n\
         FROM sys.key_constraints kc\n\
         WHERE kc.parent_object_id = OBJECT_ID({object_name}) AND kc.type = 'PK';\n\
         IF @pk_name IS NOT NULL\n\
           EXEC(N'ALTER TABLE {dynamic_table_ref} DROP CONSTRAINT ' + QUOTENAME(@pk_name));"
    );
    let mut statements = vec![drop_existing];
    if !columns.is_empty() {
        let columns = columns
            .iter()
            .map(|column| kind.quote_ident(column))
            .collect::<Vec<_>>()
            .join(", ");
        statements.push(format!(
            "ALTER TABLE {table_ref} ADD PRIMARY KEY ({columns});"
        ));
    }
    statements
}

fn sqlserver_set_column_comment(schema: &str, table: &str, column: &str, comment: &str) -> String {
    let schema = sqlserver_string(schema);
    let table = sqlserver_string(table);
    let column = sqlserver_string(column);
    let exists = format!(
        "EXISTS (SELECT 1 FROM sys.extended_properties ep \
         JOIN sys.tables t ON t.object_id = ep.major_id \
         JOIN sys.schemas s ON s.schema_id = t.schema_id \
         JOIN sys.columns c ON c.object_id = t.object_id AND c.column_id = ep.minor_id \
         WHERE ep.name = N'MS_Description' AND s.name = {schema} \
           AND t.name = {table} AND c.name = {column})"
    );
    let levels = format!(
        "@name=N'MS_Description', @level0type=N'SCHEMA', @level0name={schema}, \
         @level1type=N'TABLE', @level1name={table}, @level2type=N'COLUMN', @level2name={column}"
    );
    if comment.trim().is_empty() {
        format!("IF {exists}\n  EXEC sys.sp_dropextendedproperty {levels};")
    } else {
        let value = sqlserver_string(comment.trim());
        format!(
            "IF {exists}\n\
               EXEC sys.sp_updateextendedproperty @name=N'MS_Description', @value={value}, \
                 @level0type=N'SCHEMA', @level0name={schema}, @level1type=N'TABLE', @level1name={table}, \
                 @level2type=N'COLUMN', @level2name={column};\n\
             ELSE\n\
               EXEC sys.sp_addextendedproperty @name=N'MS_Description', @value={value}, \
                 @level0type=N'SCHEMA', @level0name={schema}, @level1type=N'TABLE', @level1name={table}, \
                 @level2type=N'COLUMN', @level2name={column};"
        )
    }
}

impl SchemaEditor {
    /// Whether reloading would discard any table-definition edits.
    pub fn has_changes(&self) -> bool {
        if self.mode != SchemaEditorMode::Edit {
            return true;
        }
        self.original_table_name.as_deref() != Some(self.table_name.trim())
            || self.columns.iter().any(|column| {
                column.drop
                    || !column.is_existing
                    || column.is_altered()
                    || column.check_changed()
                    || column.comment_changed()
                    || column.original_primary_key.unwrap_or(false) != column.primary_key
                    || column.original_name.as_deref() != Some(column.name.trim())
            })
            || self
                .indexes
                .iter()
                .any(|index| index.drop || !index.is_existing || index.is_altered())
            || self
                .fks
                .iter()
                .any(|foreign_key| foreign_key.drop || !foreign_key.is_existing)
    }

    pub fn new_table(db_kind: DbKind, default_schema: Option<&str>) -> Self {
        Self {
            mode: SchemaEditorMode::New,
            table_name: String::new(),
            schema_name: default_schema.unwrap_or("").to_string(),
            db_kind,
            columns: vec![ColumnDraft::new_empty()],
            indexes: Vec::new(),
            fks: Vec::new(),
            active_tab: SchemaTab::Columns,
            grid_selection: None,
            focus_selected_cell: false,
            editing_type_row: None,
            column_filter: String::new(),
            original_table_name: None,
        }
    }

    pub fn edit_table(table: &TableInfo, db_kind: DbKind) -> Self {
        let columns = table
            .columns
            .iter()
            .map(ColumnDraft::from_existing)
            .collect();
        let indexes = table
            .indexes
            .iter()
            .map(|i| IndexDraft::from_existing(&i.name, &i.columns, i.unique))
            .collect();
        let fks = table
            .foreign_keys
            .iter()
            .map(FkDraft::from_existing)
            .collect();
        Self {
            mode: SchemaEditorMode::Edit,
            table_name: table.name.clone(),
            schema_name: table.schema.clone().unwrap_or_default(),
            db_kind,
            columns,
            indexes,
            fks,
            active_tab: SchemaTab::Columns,
            grid_selection: None,
            focus_selected_cell: false,
            editing_type_row: None,
            column_filter: String::new(),
            original_table_name: Some(table.name.clone()),
        }
    }

    pub fn design_table(
        table: Option<&dbcore::DesignTable>,
        db_kind: DbKind,
        default_schema: Option<&str>,
    ) -> Self {
        let Some(table) = table else {
            let mut editor = Self::new_table(db_kind, default_schema);
            editor.mode = SchemaEditorMode::DesignNew;
            return editor;
        };
        let columns = table
            .columns
            .iter()
            .map(|column| {
                let mut draft = ColumnDraft::from_existing(&dbcore::ColumnInfo {
                    name: column.name.clone(),
                    data_type: column.data_type.clone(),
                    nullable: column.nullable,
                    primary_key: column.primary_key,
                    default: column.default.clone(),
                    check: None,
                    comment: None,
                    generated: false,
                });
                draft.default = column.default.clone().unwrap_or_default();
                draft
            })
            .collect();
        let indexes = table
            .indexes
            .iter()
            .map(|index| IndexDraft::from_existing(&index.name, &index.columns, index.unique))
            .collect();
        let fks = table
            .foreign_keys
            .iter()
            .map(|fk| FkDraft {
                constraint_name: fk.name.clone(),
                columns_raw: fk.columns.join(", "),
                ref_table: fk.ref_table.clone(),
                ref_schema: fk.ref_schema.clone(),
                ref_columns_raw: fk.ref_columns.join(", "),
                on_delete: fk.on_delete,
                is_existing: true,
                drop: false,
            })
            .collect();
        Self {
            mode: SchemaEditorMode::DesignEdit,
            table_name: table.name.clone(),
            schema_name: table.schema.clone().unwrap_or_default(),
            db_kind,
            columns,
            indexes,
            fks,
            active_tab: SchemaTab::Columns,
            grid_selection: None,
            focus_selected_cell: false,
            editing_type_row: None,
            column_filter: String::new(),
            original_table_name: Some(table.name.clone()),
        }
    }

    /// Convert the portable table editor back into an ER design table. Cross-table reference
    /// validation is performed when the whole design is saved.
    pub fn to_design_table(&self) -> Result<dbcore::DesignTable, String> {
        let name = self.table_name.trim();
        if name.is_empty() {
            return Err("Table name is required.".into());
        }
        let columns: Vec<dbcore::DesignColumn> = self
            .columns
            .iter()
            .filter(|column| !column.drop)
            .map(|column| dbcore::DesignColumn {
                name: column.name.trim().to_string(),
                data_type: column.data_type.trim().to_string(),
                nullable: column.nullable,
                primary_key: column.primary_key,
                default: (!column.default.trim().is_empty())
                    .then(|| column.default.trim().to_string()),
            })
            .collect();
        if columns.is_empty()
            || columns
                .iter()
                .any(|column| column.name.is_empty() || column.data_type.is_empty())
        {
            return Err("Every table needs at least one named, typed column.".into());
        }
        Ok(dbcore::DesignTable {
            schema: (!self.schema_name.trim().is_empty())
                .then(|| self.schema_name.trim().to_string()),
            name: name.to_string(),
            columns,
            indexes: self
                .indexes
                .iter()
                .filter(|index| !index.drop)
                .map(|index| dbcore::DesignIndex {
                    name: index.name.trim().to_string(),
                    columns: index.columns(),
                    unique: index.unique,
                })
                .collect(),
            foreign_keys: self
                .fks
                .iter()
                .filter(|fk| !fk.drop)
                .map(|fk| dbcore::DesignForeignKey {
                    name: fk.constraint_name.trim().to_string(),
                    columns: FkDraft::split_cols(&fk.columns_raw),
                    ref_schema: fk.ref_schema.clone(),
                    ref_table: fk.ref_table.trim().to_string(),
                    ref_columns: FkDraft::split_cols(&fk.ref_columns_raw),
                    on_delete: fk.on_delete,
                })
                .collect(),
            layout_x: None,
            layout_y: None,
        })
    }

    fn schema(&self) -> Option<&str> {
        if self.schema_name.trim().is_empty() {
            None
        } else {
            Some(self.schema_name.trim())
        }
    }

    /// Validate and build DDL statements. Returns `Err(message)` if anything is invalid.
    pub fn build_ddl(&self) -> Result<Vec<String>, String> {
        let table = self.table_name.trim();
        if table.is_empty() {
            return Err("Table name is required.".into());
        }

        let mut stmts = Vec::new();

        match self.mode {
            SchemaEditorMode::New => {
                let active_cols: Vec<ColumnDef> = self
                    .columns
                    .iter()
                    .filter(|c| !c.drop)
                    .map(|c| c.to_def())
                    .collect();

                if active_cols.iter().all(|c| c.name.trim().is_empty()) {
                    return Err("At least one column is required.".into());
                }
                let unnamed: Vec<_> = active_cols
                    .iter()
                    .filter(|c| c.name.trim().is_empty())
                    .collect();
                if !unnamed.is_empty() {
                    return Err("All columns must have a name.".into());
                }

                let active_fks: Vec<ForeignKeyDef> = self
                    .fks
                    .iter()
                    .filter(|f| !f.drop)
                    .map(|f| f.to_def())
                    .collect();
                if self.db_kind == DbKind::DuckDb
                    && active_fks
                        .iter()
                        .any(|foreign_key| foreign_key.on_delete != FkAction::NoAction)
                {
                    return Err(
                        "DuckDB foreign keys support NO ACTION only; cascading actions are unavailable."
                            .into(),
                    );
                }

                stmts.push(build_create_table_sql(
                    self.db_kind,
                    self.schema(),
                    table,
                    &active_cols,
                    &active_fks,
                ));

                for idx in self.indexes.iter().filter(|i| !i.drop) {
                    if idx.name.trim().is_empty() {
                        return Err("All indexes must have a name.".into());
                    }
                    if idx.columns().is_empty() {
                        return Err(format!(
                            "Index '{}' must specify at least one column.",
                            idx.name
                        ));
                    }
                    stmts.push(build_create_index_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        &idx.to_def(),
                    ));
                }
            }

            SchemaEditorMode::Edit => {
                let original_table = self.original_table_name.as_deref().unwrap_or(table);
                if original_table != table {
                    stmts.push(rename_table_sql(
                        self.db_kind,
                        self.schema(),
                        original_table,
                        table,
                    )?);
                }

                let original_primary = self
                    .columns
                    .iter()
                    .filter(|column| {
                        column.is_existing && column.original_primary_key.unwrap_or(false)
                    })
                    .map(|column| {
                        column
                            .original_name
                            .clone()
                            .unwrap_or_else(|| column.name.clone())
                    })
                    .collect::<Vec<_>>();
                let current_primary = self
                    .columns
                    .iter()
                    .filter(|column| !column.drop && column.primary_key)
                    .map(|column| column.name.trim().to_string())
                    .collect::<Vec<_>>();
                let primary_changed = original_primary != current_primary;
                if primary_changed && self.db_kind != DbKind::SqlServer {
                    return Err(format!(
                        "Changing an existing primary key is not yet supported for {}.",
                        self.db_kind.label()
                    ));
                }
                if primary_changed {
                    // Drop the old constraint before a primary column is renamed or removed.
                    stmts.extend(sqlserver_replace_primary_key(
                        self.schema().unwrap_or("dbo"),
                        table,
                        &[],
                    ));
                }

                // Dropped columns
                for col in self.columns.iter().filter(|c| c.is_existing && c.drop) {
                    let orig = col.original_name.as_deref().unwrap_or(&col.name);
                    stmts.push(build_drop_column_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        orig,
                    ));
                }

                // Renamed columns
                for col in self.columns.iter().filter(|c| c.is_existing && !c.drop) {
                    let orig = col.original_name.as_deref().unwrap_or(&col.name);
                    if orig != col.name.trim() && !col.name.trim().is_empty() {
                        stmts.push(build_rename_column_sql(
                            self.db_kind,
                            self.schema(),
                            table,
                            orig,
                            col.name.trim(),
                        ));
                    }
                }

                // Altered existing columns (type / nullability / default). Runs after rename so
                // it references the column's current name (`to_def` uses `col.name`).
                for col in self
                    .columns
                    .iter()
                    .filter(|c| c.is_existing && !c.drop && c.is_altered())
                {
                    if self.db_kind == DbKind::Sqlite {
                        return Err(
                            "SQLite cannot change a column's type or nullability in place; \
                             recreate the table instead."
                                .into(),
                        );
                    }
                    stmts.extend(build_alter_column_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        &col.to_def(),
                    ));
                }

                // New columns
                for col in self.columns.iter().filter(|c| !c.is_existing && !c.drop) {
                    if col.name.trim().is_empty() {
                        return Err("New columns must have a name.".into());
                    }
                    let mut definition = col.to_def();
                    if primary_changed {
                        // The table-level constraint is added once all new columns exist.
                        definition.primary_key = false;
                    }
                    stmts.push(build_add_column_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        &definition,
                    ));
                }

                if primary_changed && !current_primary.is_empty() {
                    stmts.push(
                        sqlserver_replace_primary_key(
                            self.schema().unwrap_or("dbo"),
                            table,
                            &current_primary,
                        )
                        .pop()
                        .expect("a non-empty primary key emits an ADD statement"),
                    );
                }

                // SQL Server exposes column checks and comments as separate schema objects:
                // CHECK constraints and MS_Description extended properties.
                for col in self.columns.iter().filter(|column| !column.drop) {
                    let check_changed =
                        col.check_changed() || (!col.is_existing && !col.check.trim().is_empty());
                    let comment_changed = col.comment_changed()
                        || (!col.is_existing && !col.comment.trim().is_empty());
                    if (check_changed || comment_changed) && self.db_kind != DbKind::SqlServer {
                        return Err(format!(
                            "Editing column checks and comments is not yet supported for {}.",
                            self.db_kind.label()
                        ));
                    }
                    if check_changed {
                        stmts.extend(sqlserver_set_column_check(
                            self.schema().unwrap_or("dbo"),
                            table,
                            col.name.trim(),
                            &col.check,
                        ));
                    }
                    if comment_changed {
                        stmts.push(sqlserver_set_column_comment(
                            self.schema().unwrap_or("dbo"),
                            table,
                            col.name.trim(),
                            &col.comment,
                        ));
                    }
                }

                // Existing index edits are applied portably as drop + recreate; this also makes
                // renaming and changing the indexed columns work across every supported backend.
                for idx in self
                    .indexes
                    .iter()
                    .filter(|i| i.is_existing && (i.drop || i.is_altered()))
                {
                    stmts.push(build_drop_index_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        idx.original_name(),
                    ));
                }

                // New and edited indexes
                for idx in self
                    .indexes
                    .iter()
                    .filter(|i| !i.drop && (!i.is_existing || i.is_altered()))
                {
                    if idx.name.trim().is_empty() {
                        return Err("New indexes must have a name.".into());
                    }
                    if idx.columns().is_empty() {
                        return Err(format!(
                            "Index '{}' must specify at least one column.",
                            idx.name
                        ));
                    }
                    stmts.push(build_create_index_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        &idx.to_def(),
                    ));
                }

                // Dropped foreign keys
                for fk in self.fks.iter().filter(|f| f.is_existing && f.drop) {
                    if matches!(self.db_kind, DbKind::Sqlite | DbKind::DuckDb) {
                        return Err(format!(
                            "{} cannot drop a foreign key from an existing table.",
                            self.db_kind.label()
                        ));
                    }
                    if fk.constraint_name.trim().is_empty() {
                        return Err("Cannot drop a foreign key without a constraint name.".into());
                    }
                    stmts.push(build_drop_fk_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        fk.constraint_name.trim(),
                    ));
                }

                // New foreign keys
                for fk in self.fks.iter().filter(|f| !f.is_existing && !f.drop) {
                    if matches!(self.db_kind, DbKind::Sqlite | DbKind::DuckDb) {
                        return Err(format!(
                            "{} cannot add a foreign key to an existing table.",
                            self.db_kind.label()
                        ));
                    }
                    let def = fk.to_def();
                    if def.columns.is_empty() || def.ref_table.trim().is_empty() {
                        return Err(
                            "New foreign keys must specify columns and a referenced table.".into(),
                        );
                    }
                    stmts.push(build_add_fk_sql(self.db_kind, self.schema(), table, &def));
                }
            }
            SchemaEditorMode::DesignNew | SchemaEditorMode::DesignEdit => {
                return Err("Save ER table changes from the diagram editor.".into());
            }
        }

        if stmts.is_empty() {
            return Err("No changes to apply.".into());
        }

        Ok(stmts)
    }
}

/// Render the current introspected table as portable, dialect-specific creation DDL.
/// This is a read-only definition preview; unlike [`SchemaEditor::build_ddl`] it describes
/// the whole table instead of only the edits pending against an existing table.
pub fn build_table_definition_ddl(table: &TableInfo, db_kind: DbKind) -> Vec<String> {
    let columns = table
        .columns
        .iter()
        .map(ColumnDraft::from_existing)
        .map(|column| column.to_def())
        .collect::<Vec<_>>();
    let foreign_keys = table
        .foreign_keys
        .iter()
        .map(FkDraft::from_existing)
        .map(|foreign_key| foreign_key.to_def())
        .collect::<Vec<_>>();

    let mut statements = vec![build_create_table_sql(
        db_kind,
        table.schema.as_deref(),
        &table.name,
        &columns,
        &foreign_keys,
    )];
    statements.extend(table.indexes.iter().map(|index| {
        build_create_index_sql(
            db_kind,
            table.schema.as_deref(),
            &table.name,
            &IndexDef {
                name: index.name.clone(),
                columns: index.columns.clone(),
                unique: index.unique,
            },
        )
    }));
    statements
}

// ─── Object editors (views, triggers, routines) ──────────────────────────────

/// Whether an object editor is creating a new object or editing an existing one. Distinct
/// from [`SchemaEditorMode`], which is specific to the table editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectMode {
    Create,
    Edit,
}

/// The schema object being edited in a tab's central panel. One per-tab slot serves every
/// object editor, so switching tabs never leaves a stale editor on screen. The table editor
/// keeps its own [`SchemaEditor`] type unchanged; the others are added here.
pub enum ObjectEditor {
    Table(SchemaEditor),
    View(ViewEditor),
    Trigger(TriggerEditor),
    Routine(RoutineEditor),
}

impl ObjectEditor {
    /// Validate and build the DDL for whichever object this editor holds.
    pub fn build_ddl(&self) -> Result<Vec<String>, String> {
        match self {
            ObjectEditor::Table(e) => e.build_ddl(),
            ObjectEditor::View(e) => e.build_ddl(),
            ObjectEditor::Trigger(e) => e.build_ddl(),
            ObjectEditor::Routine(e) => e.build_ddl(),
        }
    }
}

/// Working copy of a view being created or edited.
pub struct ViewEditor {
    pub mode: ObjectMode,
    pub name: String,
    pub schema_name: String,
    /// Postgres materialized view. Only offered on Postgres.
    pub materialized: bool,
    /// The defining `SELECT` (the text after `AS`).
    pub select_body: String,
    pub db_kind: DbKind,
    /// In Edit mode, the view's `(name, materialized)` as introspected — needed to DROP the
    /// old object when a rename or a drop-then-create is required.
    pub original: Option<(String, bool)>,
}

impl ViewEditor {
    pub fn new_view(db_kind: DbKind, default_schema: Option<&str>) -> Self {
        Self {
            mode: ObjectMode::Create,
            name: String::new(),
            schema_name: default_schema.unwrap_or("").to_string(),
            materialized: false,
            select_body: "SELECT ".to_string(),
            db_kind,
            original: None,
        }
    }

    pub fn edit_view(view: &ViewInfo, db_kind: DbKind) -> Self {
        Self {
            mode: ObjectMode::Edit,
            name: view.name.clone(),
            schema_name: view.schema.clone().unwrap_or_default(),
            materialized: view.materialized,
            select_body: if view.definition.trim().is_empty() {
                "SELECT ".to_string()
            } else {
                view.definition.clone()
            },
            db_kind,
            original: Some((view.name.clone(), view.materialized)),
        }
    }

    fn schema(&self) -> Option<&str> {
        let s = self.schema_name.trim();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// Validate and build DDL. An edit becomes a single `CREATE OR REPLACE` where the dialect
    /// allows it; otherwise (SQLite, a materialized view, or a rename) it's a drop-then-create,
    /// both statements running in the preview's single transaction.
    pub fn build_ddl(&self) -> Result<Vec<String>, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("View name is required.".into());
        }
        if self.select_body.trim().is_empty() {
            return Err("The view's SELECT statement is required.".into());
        }

        let mut stmts = Vec::new();
        if let (ObjectMode::Edit, Some((orig_name, orig_mat))) = (self.mode, &self.original) {
            let renamed = orig_name != name;
            let mat_changed = *orig_mat != self.materialized;
            if renamed || mat_changed || !view_supports_replace(self.db_kind, self.materialized) {
                stmts.push(build_drop_view_sql(
                    self.db_kind,
                    self.schema(),
                    orig_name,
                    *orig_mat,
                ));
            }
        }
        // Use an in-place replace only in Edit mode when we didn't already drop the old view.
        let or_replace = self.mode == ObjectMode::Edit && stmts.is_empty();
        stmts.push(build_create_view_sql(
            self.db_kind,
            self.schema(),
            name,
            &self.select_body,
            self.materialized,
            or_replace,
        ));
        Ok(stmts)
    }
}

/// Working copy of a trigger being created or edited. Fields the active dialect can't express
/// are hidden by the form (e.g. `level`/`when_condition` for MySQL), so this single struct
/// drives all four backends.
pub struct TriggerEditor {
    pub mode: ObjectMode,
    pub name: String,
    pub schema_name: String,
    pub table: String,
    pub timing: TriggerTiming,
    pub events: Vec<TriggerEvent>,
    pub level: TriggerLevel,
    pub when_condition: String,
    pub body: String,
    /// Postgres only: execute an existing function (the `body` field holds its name) instead
    /// of generating one from an inline PL/pgSQL body.
    pub pg_existing_function: bool,
    pub db_kind: DbKind,
    pub original: Option<(String, String)>, // (name, table) when editing — for DROP
    /// Table names available to attach to, for the target combo. Captured at open time.
    pub tables: Vec<String>,
}

impl TriggerEditor {
    pub fn new_trigger(db_kind: DbKind, default_schema: Option<&str>, tables: Vec<String>) -> Self {
        // Default to a timing the dialect actually supports.
        let timing = match db_kind {
            DbKind::SqlServer => TriggerTiming::After,
            _ => TriggerTiming::Before,
        };
        Self {
            mode: ObjectMode::Create,
            name: String::new(),
            schema_name: default_schema.unwrap_or("").to_string(),
            table: tables.first().cloned().unwrap_or_default(),
            timing,
            events: vec![TriggerEvent::Insert],
            level: TriggerLevel::Row,
            when_condition: String::new(),
            body: String::new(),
            pg_existing_function: false,
            db_kind,
            original: None,
            tables,
        }
    }

    pub fn edit_trigger(trg: &TriggerInfo, db_kind: DbKind, tables: Vec<String>) -> Self {
        // The body to pre-fill differs by dialect: MySQL's `action` is already the statement
        // body; SQLite/SQL Server expose the full CREATE, so peel out the inner body; Postgres
        // keeps its logic in a separate function, so edit in "existing function" mode.
        let (pg_existing, body) = match db_kind {
            DbKind::Postgres => (true, extract_pg_trigger_fn(&trg.action)),
            DbKind::Sqlite => (false, extract_between_begin_end(&trg.action)),
            DbKind::SqlServer => (false, select_body_after_as(&trg.action)),
            _ => (false, trg.action.clone()),
        };
        Self {
            mode: ObjectMode::Edit,
            name: trg.name.clone(),
            schema_name: trg.schema.clone().unwrap_or_default(),
            table: trg.table.clone(),
            timing: trg.timing,
            events: if trg.events.is_empty() {
                vec![TriggerEvent::Insert]
            } else {
                trg.events.clone()
            },
            level: trg.level,
            when_condition: trg.when_condition.clone().unwrap_or_default(),
            body,
            pg_existing_function: pg_existing,
            db_kind,
            original: Some((trg.name.clone(), trg.table.clone())),
            tables,
        }
    }

    fn schema(&self) -> Option<&str> {
        let s = self.schema_name.trim();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    pub fn has_event(&self, e: TriggerEvent) -> bool {
        self.events.contains(&e)
    }

    /// Add or remove `e`, keeping `events` in canonical INSERT/UPDATE/DELETE order.
    pub fn set_event(&mut self, e: TriggerEvent, on: bool) {
        self.events.retain(|x| *x != e);
        if on {
            self.events.push(e);
        }
        self.events
            .sort_by_key(|e| TriggerEvent::ALL.iter().position(|x| x == e).unwrap_or(0));
    }

    pub fn build_ddl(&self) -> Result<Vec<String>, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Trigger name is required.".into());
        }
        if self.table.trim().is_empty() {
            return Err("Select the table the trigger fires on.".into());
        }
        let when = {
            let w = self.when_condition.trim();
            if w.is_empty() {
                None
            } else {
                Some(w)
            }
        };
        let build = TriggerBuild {
            schema: self.schema(),
            name,
            table: self.table.trim(),
            timing: self.timing,
            events: &self.events,
            level: self.level,
            when_condition: when,
            body: &self.body,
            pg_existing_function: self.pg_existing_function && self.db_kind == DbKind::Postgres,
        };
        let mut stmts = Vec::new();
        // Triggers have no portable in-place replace, so an edit is drop-then-create.
        if let (ObjectMode::Edit, Some((orig_name, orig_table))) = (self.mode, &self.original) {
            stmts.push(build_drop_trigger_sql(
                self.db_kind,
                self.schema(),
                orig_name,
                orig_table,
            ));
        }
        stmts.extend(build_create_trigger_sql(self.db_kind, &build)?);
        Ok(stmts)
    }
}

/// Pull the executed function name out of a Postgres `CREATE TRIGGER … EXECUTE FUNCTION fn(…)`
/// (or the older `EXECUTE PROCEDURE`) definition, best-effort. Empty when not found.
fn extract_pg_trigger_fn(action: &str) -> String {
    let upper = action.to_ascii_uppercase();
    for kw in ["EXECUTE FUNCTION", "EXECUTE PROCEDURE"] {
        if let Some(pos) = upper.find(kw) {
            let rest = action[pos + kw.len()..].trim();
            let end = rest
                .find('(')
                .or_else(|| rest.find(';'))
                .unwrap_or(rest.len());
            return rest[..end].trim().to_string();
        }
    }
    String::new()
}

/// Extract the inner statements of a `BEGIN … END` block, best-effort. Falls back to the whole
/// input when no block is found.
fn extract_between_begin_end(sql: &str) -> String {
    let upper = sql.to_ascii_uppercase();
    if let (Some(b), Some(e)) = (upper.find("BEGIN"), upper.rfind("END")) {
        if e > b + "BEGIN".len() {
            return sql[b + "BEGIN".len()..e].trim().to_string();
        }
    }
    sql.trim().to_string()
}

/// An editable routine parameter row.
#[derive(Clone)]
pub struct ParamDraft {
    pub name: String,
    pub data_type: String,
    pub mode: ParamMode,
    pub default: String,
}

impl ParamDraft {
    pub fn new_empty() -> Self {
        Self {
            name: String::new(),
            data_type: String::new(),
            mode: ParamMode::In,
            default: String::new(),
        }
    }

    fn to_param(&self) -> RoutineParam {
        RoutineParam {
            name: self.name.clone(),
            data_type: self.data_type.clone(),
            mode: self.mode,
            default: {
                let d = self.default.trim();
                (!d.is_empty()).then(|| d.to_string())
            },
        }
    }
}

/// Working copy of a function or procedure being created or edited.
pub struct RoutineEditor {
    pub mode: ObjectMode,
    pub kind: RoutineKind,
    pub name: String,
    pub schema_name: String,
    pub params: Vec<ParamDraft>,
    /// Return type (functions only).
    pub return_type: String,
    /// Implementation language (Postgres: plpgsql/sql).
    pub language: String,
    pub body: String,
    pub db_kind: DbKind,
    /// In Edit mode, the original `(name, kind, params)` — needed to DROP on a drop-then-create.
    pub original: Option<(String, RoutineKind, Vec<RoutineParam>)>,
}

impl RoutineEditor {
    pub fn new_routine(db_kind: DbKind, kind: RoutineKind, default_schema: Option<&str>) -> Self {
        Self {
            mode: ObjectMode::Create,
            kind,
            name: String::new(),
            schema_name: default_schema.unwrap_or("").to_string(),
            params: Vec::new(),
            return_type: String::new(),
            language: if db_kind == DbKind::Postgres {
                "plpgsql".to_string()
            } else {
                String::new()
            },
            body: String::new(),
            db_kind,
            original: None,
        }
    }

    pub fn edit_routine(info: &RoutineInfo, db_kind: DbKind) -> Self {
        Self {
            mode: ObjectMode::Edit,
            kind: info.kind,
            name: info.name.clone(),
            schema_name: info.schema.clone().unwrap_or_default(),
            params: info
                .params
                .iter()
                .map(|p| ParamDraft {
                    name: p.name.clone(),
                    data_type: p.data_type.clone(),
                    mode: p.mode,
                    default: p.default.clone().unwrap_or_default(),
                })
                .collect(),
            return_type: info.return_type.clone().unwrap_or_default(),
            language: info.language.clone(),
            body: extract_routine_body(info, db_kind),
            db_kind,
            original: Some((info.name.clone(), info.kind, info.params.clone())),
        }
    }

    fn schema(&self) -> Option<&str> {
        let s = self.schema_name.trim();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    pub fn build_ddl(&self) -> Result<Vec<String>, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Routine name is required.".into());
        }
        let params: Vec<RoutineParam> = self
            .params
            .iter()
            .filter(|p| !p.name.trim().is_empty())
            .map(ParamDraft::to_param)
            .collect();
        let ret = self.return_type.trim();
        let build = RoutineBuild {
            schema: self.schema(),
            name,
            kind: self.kind,
            params: &params,
            return_type: (!ret.is_empty()).then_some(ret),
            language: &self.language,
            body: &self.body,
        };
        let supports_replace = routine_supports_replace(self.db_kind);
        let mut stmts = Vec::new();
        // Drop the old routine when the dialect can't replace in place, or when a rename /
        // kind change means an in-place replace would leave the old one behind.
        if let (ObjectMode::Edit, Some((orig_name, orig_kind, orig_params))) =
            (self.mode, &self.original)
        {
            let renamed = orig_name != name || *orig_kind != self.kind;
            if renamed || !supports_replace {
                stmts.push(build_drop_routine_sql(
                    self.db_kind,
                    self.schema(),
                    orig_name,
                    *orig_kind,
                    orig_params,
                ));
            }
        }
        let or_replace = self.mode == ObjectMode::Edit && supports_replace && stmts.is_empty();
        stmts.extend(build_create_routine_sql(self.db_kind, &build, or_replace)?);
        Ok(stmts)
    }
}

/// Best-effort extraction of a routine's editable body from its introspected definition: the
/// dollar-quoted body on Postgres, the text after `AS` on SQL Server, and MySQL's
/// already-inner `ROUTINE_DEFINITION` unchanged.
fn extract_routine_body(info: &RoutineInfo, db_kind: DbKind) -> String {
    match db_kind {
        DbKind::Postgres => extract_dollar_quoted(&info.body).unwrap_or_else(|| info.body.clone()),
        DbKind::SqlServer => select_body_after_as(&info.body),
        _ => info.body.clone(),
    }
}

/// Extract the text inside the first matched `$tag$ … $tag$` dollar-quote of `def`.
fn extract_dollar_quoted(def: &str) -> Option<String> {
    let open = def.find('$')?;
    let rest = &def[open..];
    let tag_len = rest[1..].find('$')? + 2; // through the closing `$` of the opening tag
    let tag = &rest[..tag_len];
    let after = &rest[tag_len..];
    let close = after.find(tag)?;
    Some(after[..close].trim().to_string())
}

#[cfg(test)]
mod object_editor_tests {
    use super::*;
    use dbcore::{ColumnInfo, IndexInfo, ViewInfo};

    fn view(schema: Option<&str>, materialized: bool) -> ViewInfo {
        ViewInfo {
            schema: schema.map(str::to_string),
            name: "v".into(),
            columns: Vec::new(),
            definition: "SELECT 1".into(),
            materialized,
        }
    }

    #[test]
    fn editing_an_existing_index_drops_then_recreates_it() {
        let table = TableInfo {
            schema: None,
            name: "items".into(),
            columns: vec![ColumnInfo {
                name: "sku".into(),
                data_type: "TEXT".into(),
                nullable: false,
                primary_key: false,
                default: None,
                check: None,
                comment: None,
                generated: false,
            }],
            indexes: vec![IndexInfo {
                name: "idx_items_sku".into(),
                unique: false,
                columns: vec!["sku".into()],
            }],
            foreign_keys: Vec::new(),
        };
        let mut editor = SchemaEditor::edit_table(&table, DbKind::Sqlite);
        editor.indexes[0].unique = true;

        let sql = editor.build_ddl().unwrap();
        assert_eq!(sql.len(), 2);
        assert!(sql[0].starts_with("DROP INDEX"));
        assert!(sql[1].starts_with("CREATE UNIQUE INDEX"));
    }

    #[test]
    fn table_definition_ddl_uses_the_selected_dialect_and_includes_indexes() {
        let table = TableInfo {
            schema: Some("dbo".into()),
            name: "items".into(),
            columns: vec![ColumnInfo {
                name: "sku".into(),
                data_type: "varchar(15)".into(),
                nullable: false,
                primary_key: true,
                default: None,
                check: None,
                comment: None,
                generated: false,
            }],
            indexes: vec![IndexInfo {
                name: "idx_items_sku".into(),
                unique: true,
                columns: vec!["sku".into()],
            }],
            foreign_keys: Vec::new(),
        };

        let sql = build_table_definition_ddl(&table, DbKind::SqlServer);
        assert_eq!(sql.len(), 2);
        assert!(sql[0].starts_with("CREATE TABLE [dbo].[items]"));
        assert!(sql[0].contains("[sku] varchar(15) NOT NULL PRIMARY KEY"));
        assert!(sql[1].starts_with("CREATE UNIQUE INDEX [idx_items_sku]"));
    }

    #[test]
    fn sqlserver_column_check_and_comment_changes_build_real_ddl() {
        let table = TableInfo {
            schema: Some("dbo".into()),
            name: "items".into(),
            columns: vec![ColumnInfo {
                name: "qty".into(),
                data_type: "int".into(),
                nullable: false,
                primary_key: false,
                default: None,
                check: Some("[qty] > 0".into()),
                comment: Some("Old description".into()),
                generated: false,
            }],
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        };
        let mut editor = SchemaEditor::edit_table(&table, DbKind::SqlServer);
        editor.columns[0].check = "[qty] >= 0".into();
        editor.columns[0].comment = "Available quantity".into();

        assert!(editor.has_changes());
        let sql = editor.build_ddl().unwrap();
        assert!(sql
            .iter()
            .any(|stmt| stmt.contains("sys.check_constraints")));
        assert!(sql
            .iter()
            .any(|stmt| stmt.contains("ADD CONSTRAINT [CK_items_qty]")));
        assert!(sql
            .iter()
            .any(|stmt| stmt.contains("sp_updateextendedproperty")));
    }

    #[test]
    fn sqlserver_table_name_and_primary_key_changes_build_real_ddl() {
        let table = TableInfo {
            schema: Some("dbo".into()),
            name: "items".into(),
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    data_type: "int".into(),
                    nullable: false,
                    primary_key: true,
                    default: None,
                    check: None,
                    comment: None,
                    generated: false,
                },
                ColumnInfo {
                    name: "sku".into(),
                    data_type: "varchar(15)".into(),
                    nullable: false,
                    primary_key: false,
                    default: None,
                    check: None,
                    comment: None,
                    generated: false,
                },
            ],
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        };
        let mut editor = SchemaEditor::edit_table(&table, DbKind::SqlServer);
        editor.table_name = "inventory".into();
        editor.columns[0].primary_key = false;
        editor.columns[1].primary_key = true;

        assert!(editor.has_changes());
        let sql = editor.build_ddl().unwrap();
        assert_eq!(sql.len(), 3);
        assert!(sql[0].contains("sp_rename"));
        assert!(sql[0].contains("[dbo].[items]"));
        assert!(sql[1].contains("DROP CONSTRAINT"));
        assert!(sql[1].contains("[dbo].[inventory]"));
        assert_eq!(
            sql[2],
            "ALTER TABLE [dbo].[inventory] ADD PRIMARY KEY ([sku]);"
        );
    }

    #[test]
    fn view_edit_sqlite_drops_then_creates() {
        let mut e = ViewEditor::edit_view(&view(None, false), DbKind::Sqlite);
        e.select_body = "SELECT 2".into();
        let sql = e.build_ddl().unwrap();
        assert_eq!(sql.len(), 2);
        assert!(sql[0].starts_with("DROP VIEW"));
        assert!(sql[1].starts_with("CREATE VIEW"));
    }

    #[test]
    fn view_edit_postgres_replaces_in_place() {
        let e = ViewEditor::edit_view(&view(Some("public"), false), DbKind::Postgres);
        let sql = e.build_ddl().unwrap();
        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("CREATE OR REPLACE VIEW"));
    }

    #[test]
    fn trigger_postgres_emits_function_and_trigger() {
        let mut e = TriggerEditor::new_trigger(DbKind::Postgres, Some("public"), vec!["t".into()]);
        e.name = "trg".into();
        e.table = "t".into();
        e.body = "BEGIN RETURN NEW; END;".into();
        let sql = e.build_ddl().unwrap();
        assert_eq!(sql.len(), 2);
        assert!(sql[0].contains("CREATE OR REPLACE FUNCTION"));
        assert!(sql[1].contains("CREATE TRIGGER"));
    }

    #[test]
    fn routine_edit_mysql_drops_then_creates() {
        let info = RoutineInfo {
            schema: None,
            name: "p".into(),
            kind: RoutineKind::Procedure,
            params: Vec::new(),
            return_type: None,
            language: String::new(),
            body: "BEGIN END".into(),
        };
        let mut e = RoutineEditor::edit_routine(&info, DbKind::MySql);
        e.body = "BEGIN SELECT 1; END".into();
        let sql = e.build_ddl().unwrap();
        assert_eq!(sql.len(), 2);
        assert!(sql[0].starts_with("DROP PROCEDURE"));
        assert!(sql[1].starts_with("CREATE PROCEDURE"));
    }

    #[test]
    fn routine_function_needs_return_type() {
        let mut e = RoutineEditor::new_routine(DbKind::Postgres, RoutineKind::Function, None);
        e.name = "f".into();
        e.body = "SELECT 1".into();
        assert!(e.build_ddl().is_err());
        e.return_type = "integer".into();
        assert!(e.build_ddl().is_ok());
    }
}
