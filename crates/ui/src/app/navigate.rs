//! Opening tables and following relationships: table tabs, preview slots, FK navigation.

use super::*;

impl DbGuiApp {
    /// Open a table (from the schema sidebar) as a named tab.
    ///
    /// - If the table is already open in a tab, just switch to it (no duplicate).
    /// - Otherwise show it in the reusable italic *preview* tab (single-click): one preview
    ///   slot is reused as you click through tables, so they don't pile up. A blank scratch
    ///   tab is upgraded into that preview slot rather than spawning a new tab.
    /// - `pin` (double-click) makes the tab permanent (non-italic) instead.
    pub(super) fn open_table(
        &mut self,
        sql: String,
        source: EditSource,
        pin: bool,
        kind: crate::components::QueryTabKind,
    ) {
        let conn_id = self.tab().conn_id.clone();
        let same = |s: &EditSource| s.table == source.table && s.schema == source.schema;
        // Already open (loaded or in-flight)? Activate it, pinning if asked.
        if let Some(idx) = self.tabs.iter().position(|t| {
            t.conn_id == conn_id
                && t.edits
                    .source
                    .as_ref()
                    .or(t.edits.pending_source.as_ref())
                    .is_some_and(same)
        }) {
            if pin {
                self.tabs[idx].preview = false;
            }
            self.select_tab(idx);
            // Result is cleared on disconnect; re-selecting the same table from the sidebar
            // must re-run the preview query instead of leaving an empty grid.
            if self.tabs[idx].result.is_none()
                && self.tabs[idx]
                    .conn_id
                    .as_deref()
                    .is_some_and(|cid| self.active_connections.iter().any(|c| c.config_id == cid))
            {
                self.start_query_for(idx);
            }
            return;
        }

        self.open_in_preview_slot(sql, source, !pin, kind);
    }
    /// Load `sql` into a table tab bound to `source` and run it. Picks the reusable preview
    /// slot, else a blank scratch active tab, else a fresh tab (see [`Self::preview_target_slot`]),
    /// then rebuilds that tab from scratch — clearing any previous preview's result/filter/edits —
    /// while keeping its stable id and connection binding. Shared by [`Self::open_table`] and
    /// foreign-key follow. `preview` marks the tab as the transient (italic, reusable) preview.
    pub(super) fn open_in_preview_slot(
        &mut self,
        sql: String,
        source: EditSource,
        preview: bool,
        kind: crate::components::QueryTabKind,
    ) {
        // Preview tabs are global and may be reused across connections. Always bind the rebuilt
        // tab to the connection that initiated this open, never the preview slot's old owner.
        let conn_id = self.tab().conn_id.clone();
        let idx = self.preview_target_slot();
        let id = self.tabs[idx].id;
        let mut tab = QueryTab::new(id, source.table.clone());
        tab.conn_id = conn_id;
        tab.kind = kind;
        tab.sql = sql;
        tab.preview = preview;
        tab.edits.pending_source = Some(source);
        self.tabs[idx] = tab;
        self.active_query_tab = idx;
        self.workspace_dirty = true;
        self.start_query_for(idx);
    }
    /// Follow the foreign key that column `col` of the active table tab's row `row` participates
    /// in: open a preview tab on the referenced table, filtered to the key the cell points at.
    /// `row`/`col` index the *raw* result (not the display order). A no-op with a status hint
    /// when the column isn't a foreign key or its value is NULL (nothing to navigate to).
    pub(super) fn follow_foreign_key(&mut self, row: usize, col: usize) {
        let idx = self.active_query_tab;
        match self.build_fk_follow(idx, row, col) {
            Some((sql, source)) => {
                self.open_in_preview_slot(sql, source, true, crate::components::QueryTabKind::Table)
            }
            None => {
                self.status_msg =
                    "No foreign key to follow here (or the value is empty).".to_string();
            }
        }
    }
    /// Resolve the foreign key column `col` (raw result column) belongs to on the tab at `idx`,
    /// and build a `SELECT … WHERE <ref key = cell value>` on the referenced table plus its edit
    /// source. `row` is the raw result-row index. Returns `None` when the column isn't a foreign
    /// key, the referenced value is wholly NULL, or the connection/columns can't be resolved.
    pub(super) fn build_fk_follow(
        &self,
        idx: usize,
        row: usize,
        col: usize,
    ) -> Option<(String, EditSource)> {
        let tab = self.tabs.get(idx)?;
        let result = tab.result.as_ref()?;
        let conn = tab
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))?;
        let kind = conn.db.kind();
        let info = self.structure_table(idx)?;
        let col_name = &result.columns.get(col)?.name;
        // The FK this column takes part in (first match; a column is rarely in more than one).
        let fk = info
            .foreign_keys
            .iter()
            .find(|fk| fk.columns.iter().any(|c| c.eq_ignore_ascii_case(col_name)))?;
        // Pair each referenced column with the value from this row's matching referencing column.
        let mut keys: Vec<(&str, &dbcore::Value)> = Vec::with_capacity(fk.ref_columns.len());
        for (referencing, referenced) in fk.columns.iter().zip(fk.ref_columns.iter()) {
            let ci = result
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(referencing))?;
            keys.push((referenced.as_str(), result.rows.get(row)?.get(ci)?));
        }
        // A wholly-NULL foreign key references nothing — nothing to navigate to.
        if keys.iter().all(|(_, v)| v.is_null()) {
            return None;
        }
        let sql = dbcore::build_select_where_sql(
            kind,
            fk.ref_schema.as_deref(),
            &fk.ref_table,
            &keys,
            100,
        )?;
        // Edit source for the referenced table: its PK columns make the opened rows editable —
        // empty on a read-only connection, exactly like the sidebar and `derive_edit_source` paths.
        let ref_info = conn.schema.tables.iter().find(|t| {
            t.name.eq_ignore_ascii_case(&fk.ref_table)
                && match (&fk.ref_schema, &t.schema) {
                    (Some(s), Some(ts)) => s.eq_ignore_ascii_case(ts),
                    (None, _) => true,
                    (Some(_), None) => false,
                }
        });
        let pk_cols = match ref_info {
            Some(t) if !self.tab_connection_is_read_only(idx) => t
                .columns
                .iter()
                .filter(|c| c.primary_key)
                .map(|c| c.name.clone())
                .collect(),
            _ => Vec::new(),
        };
        Some((
            sql,
            EditSource {
                schema: ref_info
                    .and_then(|t| t.schema.clone())
                    .or_else(|| fk.ref_schema.clone()),
                table: ref_info
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| fk.ref_table.clone()),
                pk_cols,
            },
        ))
    }
    /// Per-result-column foreign-key labels for the tab at `idx`: `Some(ref_table)` where the
    /// column takes part in a foreign key on the current table, else `None`. Drives the grid's
    /// FK link tint + "Follow →" affordance. Empty when there's no result or no structure yet.
    pub(super) fn fk_column_labels(&self, idx: usize) -> Vec<Option<String>> {
        let Some(result) = self.tabs.get(idx).and_then(|t| t.result.as_ref()) else {
            return Vec::new();
        };
        let Some(info) = self.structure_table(idx) else {
            return vec![None; result.column_count()];
        };
        result
            .columns
            .iter()
            .map(|col| {
                info.foreign_keys
                    .iter()
                    .find(|fk| fk.columns.iter().any(|c| c.eq_ignore_ascii_case(&col.name)))
                    .map(|fk| fk.ref_table.clone())
            })
            .collect()
    }
    /// The tab slot a preview should land in: the existing reusable preview slot, else the
    /// active tab when it's a blank scratch tab, else a freshly opened tab. Shared by
    /// [`Self::open_table`] and [`Self::open_definition`].
    pub(super) fn preview_target_slot(&mut self) -> usize {
        if let Some(i) = self.tabs.iter().position(|t| t.preview) {
            i
        } else {
            let cur = &self.tabs[self.active_query_tab];
            if cur.edits.source.is_none() && cur.result.is_none() && cur.sql.trim().is_empty() {
                self.active_query_tab
            } else {
                self.new_tab();
                self.active_query_tab
            }
        }
    }
    /// Show a database object's definition SQL (a routine body, a trigger's `CREATE` text)
    /// in a preview tab for reading. Unlike [`Self::open_table`] the SQL is *not* executed —
    /// re-running a `CREATE` would recreate the object — it's just placed in the editor so the
    /// user can read, copy, or run it deliberately.
    pub(super) fn open_definition(
        &mut self,
        title: String,
        sql: String,
        kind: crate::components::QueryTabKind,
    ) {
        let idx = self.preview_target_slot();
        let id = self.tabs[idx].id;
        let conn_id = self.tabs[idx].conn_id.clone();
        let mut tab = QueryTab::new(id, title);
        tab.conn_id = conn_id;
        tab.kind = kind;
        tab.sql = if sql.trim().is_empty() {
            "-- No definition available (the backend did not expose this object's source).".into()
        } else {
            sql
        };
        tab.preview = true;
        self.tabs[idx] = tab;
        self.active_query_tab = idx;
        self.workspace_dirty = true;
    }
}
