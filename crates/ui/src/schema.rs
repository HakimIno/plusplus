//! Schema editor state — no egui drawing here; all rendering is in panels.rs.

use dbcore::{
    build_add_column_sql, build_add_fk_sql, build_alter_column_sql, build_create_index_sql,
    build_create_table_sql, build_drop_column_sql, build_drop_fk_sql, build_drop_index_sql,
    build_rename_column_sql,
    ColumnDef, DbKind, FkAction, ForeignKeyDef, ForeignKeyInfo, IndexDef, TableInfo,
};

// ─── Draft types (UI working copies) ────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ColumnDraft {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub default: String,
    /// Only set for existing columns (ALTER TABLE context).
    pub original_name: Option<String>,
    /// Type/nullability as introspected, kept so an edit to them can be detected and turned
    /// into an `ALTER COLUMN`. `None` for newly added columns.
    pub original_type: Option<String>,
    pub original_nullable: Option<bool>,
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
            original_name: None,
            original_type: None,
            original_nullable: None,
            is_existing: false,
            drop: false,
        }
    }

    pub fn from_existing(name: &str, data_type: &str, nullable: bool, primary_key: bool) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable,
            primary_key,
            default: String::new(),
            original_name: Some(name.into()),
            original_type: Some(data_type.into()),
            original_nullable: Some(nullable),
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
        let default_set = !self.default.trim().is_empty();
        type_changed || null_changed || default_set
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
    pub is_existing: bool,
    pub drop: bool,
}

impl IndexDraft {
    pub fn new_empty() -> Self {
        Self {
            name: String::new(),
            columns_raw: String::new(),
            unique: false,
            is_existing: false,
            drop: false,
        }
    }

    pub fn from_existing(name: &str, columns: &[String], unique: bool) -> Self {
        Self {
            name: name.into(),
            columns_raw: columns.join(", "),
            unique,
            is_existing: true,
            drop: false,
        }
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
    NewTable,
    EditTable,
}

// ─── Tab ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaTab {
    Columns,
    Indexes,
    ForeignKeys,
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
    /// Original table name when editing (used for future rename support).
    #[allow(dead_code)]
    pub original_table_name: Option<String>,
}

impl SchemaEditor {
    pub fn new_table(db_kind: DbKind, default_schema: Option<&str>) -> Self {
        Self {
            mode: SchemaEditorMode::NewTable,
            table_name: String::new(),
            schema_name: default_schema.unwrap_or("").to_string(),
            db_kind,
            columns: vec![ColumnDraft::new_empty()],
            indexes: Vec::new(),
            fks: Vec::new(),
            active_tab: SchemaTab::Columns,
            original_table_name: None,
        }
    }

    pub fn edit_table(table: &TableInfo, db_kind: DbKind) -> Self {
        let columns = table
            .columns
            .iter()
            .map(|c| {
                ColumnDraft::from_existing(&c.name, &c.data_type, c.nullable, c.primary_key)
            })
            .collect();
        let indexes = table
            .indexes
            .iter()
            .map(|i| IndexDraft::from_existing(&i.name, &i.columns, i.unique))
            .collect();
        let fks = table.foreign_keys.iter().map(FkDraft::from_existing).collect();
        Self {
            mode: SchemaEditorMode::EditTable,
            table_name: table.name.clone(),
            schema_name: table.schema.clone().unwrap_or_default(),
            db_kind,
            columns,
            indexes,
            fks,
            active_tab: SchemaTab::Columns,
            original_table_name: Some(table.name.clone()),
        }
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
            SchemaEditorMode::NewTable => {
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

                let active_fks: Vec<ForeignKeyDef> =
                    self.fks.iter().filter(|f| !f.drop).map(|f| f.to_def()).collect();

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
                        return Err(format!("Index '{}' must specify at least one column.", idx.name));
                    }
                    stmts.push(build_create_index_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        &idx.to_def(),
                    ));
                }
            }

            SchemaEditorMode::EditTable => {
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
                for col in self
                    .columns
                    .iter()
                    .filter(|c| c.is_existing && !c.drop)
                {
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
                    stmts.push(build_add_column_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        &col.to_def(),
                    ));
                }

                // Dropped indexes
                for idx in self.indexes.iter().filter(|i| i.is_existing && i.drop) {
                    stmts.push(build_drop_index_sql(
                        self.db_kind,
                        self.schema(),
                        table,
                        &idx.name,
                    ));
                }

                // New indexes
                for idx in self.indexes.iter().filter(|i| !i.is_existing && !i.drop) {
                    if idx.name.trim().is_empty() {
                        return Err("New indexes must have a name.".into());
                    }
                    if idx.columns().is_empty() {
                        return Err(format!("Index '{}' must specify at least one column.", idx.name));
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
                    if self.db_kind == DbKind::Sqlite {
                        return Err(
                            "SQLite cannot drop a foreign key from an existing table.".into()
                        );
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
                    if self.db_kind == DbKind::Sqlite {
                        return Err(
                            "SQLite cannot add a foreign key to an existing table.".into()
                        );
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
        }

        if stmts.is_empty() {
            return Err("No changes to apply.".into());
        }

        Ok(stmts)
    }
}
