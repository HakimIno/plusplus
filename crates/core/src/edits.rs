//! GUI-independent planning for staged row edits. Planning never executes SQL.
//!
//! Row indices refer to the supplied result snapshot, not to a remote API's row identity.
//! Callers must bind a plan to that snapshot and connection before exposing commit.

use std::collections::{HashMap, HashSet};

use crate::{
    build_delete_sql, build_insert_sql, build_update_sql, DbKind, EditorKind, QueryResult, Value,
};

pub const NEW_ROW_BASE: usize = 1 << 48;

pub fn is_new_row(row: usize) -> bool {
    row >= NEW_ROW_BASE
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditSource {
    pub schema: Option<String>,
    pub table: String,
    /// Empty means the result is read-only.
    pub pk_cols: Vec<String>,
}

impl EditSource {
    pub fn editable(&self) -> bool {
        !self.pk_cols.is_empty()
    }
}

/// Recognize editable SQL using the complete statement, not just the first clause after
/// FROM. Keep the existing narrow projection/table grammar used by the pager. Unsupported
/// dialect syntax remains read-only; parsing failure is never permission to edit rows.
pub fn editable_select_target(kind: DbKind, sql: &str) -> Option<(Option<String>, String)> {
    use sqlparser::ast::{GroupByExpr, SetExpr, Statement};
    use sqlparser::parser::Parser;

    let target = crate::simple_select_target(sql)?;
    let dialect = crate::syntax::dialect_for(Some(kind));
    let statements = Parser::new(dialect.as_ref())
        .with_recursion_limit(128)
        .try_with_sql(sql)
        .ok()?
        .parse_statements()
        .ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    let plain_group = matches!(&select.group_by, GroupByExpr::Expressions(exprs, modifiers) if exprs.is_empty() && modifiers.is_empty());
    let row_preserving = query.with.is_none()
        && query.for_clause.is_none()
        && query.format_clause.is_none()
        && query.pipe_operators.is_empty()
        && select.from.len() == 1
        && select.from[0].joins.is_empty()
        && select.distinct.is_none()
        && select.into.is_none()
        && select.lateral_views.is_empty()
        && plain_group
        && select.having.is_none()
        && select.qualify.is_none()
        && select.value_table_mode.is_none();
    row_preserving.then_some(target)
}

/// Borrow the staging maps: large text/BLOB values are not copied during planning.
pub struct EditBatch<'a> {
    pub source: &'a EditSource,
    pub result: &'a QueryResult,
    pub cells: &'a HashMap<usize, HashMap<usize, Value>>,
    pub deleted: &'a HashSet<usize>,
    pub new_rows: usize,
    pub generated_columns: &'a [usize],
}

#[derive(Debug, PartialEq, Eq)]
pub struct CommitPlan {
    /// Stable row and column order makes previews reproducible and comparable.
    pub statements: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("Cannot save: the result has no primary key.")]
    NoPrimaryKey,
    #[error("Cannot save: primary key columns are missing or ambiguous in the result.")]
    PrimaryKeyColumns,
    #[error("Cannot save: row {0} is no longer in the result. Preview the edits again.")]
    InvalidRow(usize),
    #[error("Cannot save: column {0} is no longer in the result.")]
    InvalidColumn(usize),
    #[error("Cannot save: row {row}, column {column} holds a value invalid for its type.")]
    InvalidValue { row: usize, column: usize },
    #[error("Cannot add row: primary key \"{0}\" is required.")]
    MissingPrimaryKey(String),
    #[error("Cannot add row: duplicate primary key.")]
    DuplicatePrimaryKey,
    #[error("Cannot save: a value can't be written.")]
    UnwritableValue,
}

// Borrowed keys preserve Value's type-sensitive equality, including -0.0 == 0.0.
// This is a local duplicate check, not an emulation of database collations/coercions.
#[derive(PartialEq, Eq, Hash)]
enum KeyPart<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Float(u64),
    Text(&'a str),
    Bytes(&'a [u8]),
}

fn key_part(value: &Value) -> Result<KeyPart<'_>, EditError> {
    Ok(match value {
        Value::Null => KeyPart::Null,
        Value::Bool(v) => KeyPart::Bool(*v),
        Value::Int(v) => KeyPart::Int(*v),
        Value::Float(v) if v.is_finite() => KeyPart::Float(if *v == 0.0 { 0 } else { v.to_bits() }),
        Value::Float(_) => return Err(EditError::UnwritableValue),
        Value::Text(v) => KeyPart::Text(v),
        Value::Bytes(v) => KeyPart::Bytes(v),
    })
}

pub fn plan_edits(kind: DbKind, batch: EditBatch<'_>) -> Result<CommitPlan, EditError> {
    let EditBatch {
        source,
        result,
        cells,
        deleted,
        new_rows,
        generated_columns,
    } = batch;
    if !source.editable() {
        return Err(EditError::NoPrimaryKey);
    }
    let mut columns = HashMap::with_capacity(result.columns.len());
    for (index, column) in result.columns.iter().enumerate() {
        if columns.insert(column.name.as_str(), index).is_some() {
            return Err(EditError::PrimaryKeyColumns);
        }
    }
    let pk: Vec<usize> = source
        .pk_cols
        .iter()
        .map(|name| {
            columns
                .get(name.as_str())
                .copied()
                .ok_or(EditError::PrimaryKeyColumns)
        })
        .collect::<Result<_, _>>()?;
    let kinds: Vec<_> = result
        .columns
        .iter()
        .map(|c| EditorKind::classify(&c.type_name))
        .collect();
    let mut changed: Vec<_> = cells
        .iter()
        .filter(|(_, values)| !values.is_empty())
        .collect();
    changed.sort_unstable_by_key(|(row, _)| **row);
    for (&row, values) in &changed {
        if if is_new_row(row) {
            row - NEW_ROW_BASE >= new_rows
        } else {
            row >= result.rows.len()
        } {
            return Err(EditError::InvalidRow(row));
        }
        for (&col, value) in *values {
            let kind = kinds.get(col).ok_or(EditError::InvalidColumn(col))?;
            if !kind.accepts(value) {
                return Err(EditError::InvalidValue { row, column: col });
            }
        }
    }
    let keys_for = |row: usize| -> Result<Vec<(&str, &Value)>, EditError> {
        let values = result.rows.get(row).ok_or(EditError::InvalidRow(row))?;
        pk.iter()
            .map(|&col| {
                Ok((
                    result.columns[col].name.as_str(),
                    values.get(col).ok_or(EditError::InvalidRow(row))?,
                ))
            })
            .collect()
    };
    let sorted_values = |values: &HashMap<usize, Value>| {
        let mut indices: Vec<_> = values.keys().copied().collect();
        indices.sort_unstable();
        indices
    };
    let mut statements = Vec::with_capacity(changed.len() + deleted.len());
    for (&row, values) in &changed {
        if is_new_row(row) || deleted.contains(&row) {
            continue;
        }
        let sets: Vec<_> = sorted_values(values)
            .into_iter()
            .map(|col| (result.columns[col].name.as_str(), &values[&col]))
            .collect();
        let original = result.rows.get(row).ok_or(EditError::InvalidRow(row))?;
        let mut optimistic_keys = keys_for(row)?;
        for col in sorted_values(values) {
            if !pk.contains(&col) {
                optimistic_keys.push((
                    result.columns[col].name.as_str(),
                    original.get(col).ok_or(EditError::InvalidRow(row))?,
                ));
            }
        }
        statements.push(
            build_update_sql(
                kind,
                source.schema.as_deref(),
                &source.table,
                &sets,
                &optimistic_keys,
            )
            .ok_or(EditError::UnwritableValue)?,
        );
    }
    let mut deleted_rows: Vec<_> = deleted.iter().copied().collect();
    deleted_rows.sort_unstable();
    for row in deleted_rows {
        let original = result.rows.get(row).ok_or(EditError::InvalidRow(row))?;
        let optimistic_keys: Vec<_> = result
            .columns
            .iter()
            .enumerate()
            .map(|(col, column)| {
                Ok((
                    column.name.as_str(),
                    original.get(col).ok_or(EditError::InvalidRow(row))?,
                ))
            })
            .collect::<Result<_, EditError>>()?;
        statements.push(
            build_delete_sql(
                kind,
                source.schema.as_deref(),
                &source.table,
                &optimistic_keys,
            )
            .ok_or(EditError::UnwritableValue)?,
        );
    }

    // No inserts => no full-result scan or PK index allocation. Ignore untouched new rows.
    let inserts: Vec<_> = changed
        .into_iter()
        .filter(|(row, _)| is_new_row(**row))
        .collect();
    if !inserts.is_empty() {
        let mut seen = HashSet::with_capacity(result.rows.len() + inserts.len());
        for (row, values) in result.rows.iter().enumerate() {
            if deleted.contains(&row) {
                continue;
            }
            // UPDATE runs first, so use its final keys when checking INSERT collisions.
            let tuple: Vec<_> = pk
                .iter()
                .map(|&col| {
                    let value = cells
                        .get(&row)
                        .and_then(|v| v.get(&col))
                        .or_else(|| values.get(col))
                        .ok_or(EditError::InvalidRow(row))?;
                    key_part(value)
                })
                .collect::<Result<_, _>>()?;
            seen.insert(tuple);
        }
        for (_, values) in inserts {
            let mut tuple = Vec::with_capacity(pk.len());
            let mut complete_key = true;
            for &col in &pk {
                match values.get(&col).filter(|v| !v.is_null()) {
                    Some(value) => tuple.push(key_part(value)?),
                    None if generated_columns.contains(&col) => complete_key = false,
                    None => {
                        return Err(EditError::MissingPrimaryKey(
                            result.columns[col].name.clone(),
                        ))
                    }
                }
            }
            if complete_key && !seen.insert(tuple) {
                return Err(EditError::DuplicatePrimaryKey);
            }
            let cols: Vec<_> = sorted_values(values)
                .into_iter()
                .filter(|&col| !(generated_columns.contains(&col) && values[&col].is_null()))
                .map(|col| (result.columns[col].name.as_str(), &values[&col]))
                .collect();
            statements.push(
                build_insert_sql(kind, source.schema.as_deref(), &source.table, &cols)
                    .ok_or(EditError::UnwritableValue)?,
            );
        }
    }
    Ok(CommitPlan { statements })
}

#[cfg(test)]
mod tests;
