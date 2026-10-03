//! Guard actions that would destroy staged data or schema changes.
use super::*;

pub(super) struct PendingLeave {
    pub action: Action,
    pub tab_ids: Vec<u64>,
}

impl DbGuiApp {
    pub(super) fn tab_has_unsaved_changes(&self, idx: usize) -> bool {
        self.tabs.get(idx).is_some_and(|tab| {
            // A draft tab holds an unsaved object only once something was typed into it.
            if tab.draft_tab {
                return self.draft_is_dirty_at(idx);
            }
            tab.edits.has_pending()
                || tab.edits.active.is_some()
                || (tab.diagram.is_none()
                    && matches!(tab.schema_editor.as_ref(), Some(ObjectEditor::Table(e)) if e.has_changes()))
        })
    }

    /// Stable tab ids identify every affected workspace, including inactive connections.
    fn leaving_tabs(&self, action: &Action) -> Vec<u64> {
        let connection = match action {
            Action::SaveAndConnect => self.editor.as_ref().map(|editor| editor.config.id.as_str()),
            Action::Disconnect => self
                .tabs
                .get(self.active_query_tab)
                .and_then(|t| t.conn_id.as_deref()),
            Action::DisconnectConn(i)
            | Action::DeleteConnection(i)
            | Action::SwitchDatabase { conn_idx: i, .. }
            | Action::Connect(i) => self.connections.get(*i).map(|c| c.id.as_str()),
            _ => None,
        };
        // Run / reload replaces the result in place, which throws staged row edits away.
        let run_target = self.focused_tab_idx();
        self.tabs
            .iter()
            .enumerate()
            .filter(|(idx, tab)| {
                if let Some(id) = connection {
                    return tab.conn_id.as_deref() == Some(id);
                }
                match action {
                    Action::CloseTab(i) | Action::CloseSplitPaneTab { idx: i, .. } => idx == i,
                    Action::CloseOtherTabs(i) => idx != i,
                    Action::CloseTabsToRight(i) => idx > i,
                    Action::CloseAllTabs | Action::Quit => true,
                    Action::ReloadTableStructure => *idx == self.active_query_tab,
                    Action::RunQuery | Action::RunCurrentQuery => *idx == run_target,
                    _ => false,
                }
            })
            .map(|(_, t)| t.id)
            .collect()
    }

    pub(super) fn guard_leaving(&mut self, action: Action) -> Option<Action> {
        if self.pending_leave.is_some() {
            return matches!(
                action,
                Action::SaveBeforeLeaving | Action::DiscardBeforeLeaving | Action::CancelLeaving
            )
            .then_some(action);
        }
        let tab_ids = self.leaving_tabs(&action);
        if tab_ids.iter().any(|id| {
            self.tabs
                .iter()
                .position(|t| t.id == *id)
                .is_some_and(|idx| self.tab_has_unsaved_changes(idx))
        }) {
            let action = match action {
                Action::Disconnect => self
                    .tabs
                    .get(self.active_query_tab)
                    .and_then(|tab| {
                        self.connections
                            .iter()
                            .position(|c| Some(&c.id) == tab.conn_id.as_ref())
                    })
                    .map(Action::DisconnectConn)
                    .unwrap_or(Action::Disconnect),
                action => action,
            };
            self.pending_leave = Some(PendingLeave { action, tab_ids });
            return None;
        }
        Some(action)
    }

    pub(super) fn save_before_leaving(&mut self) {
        let Some(pending) = self.pending_leave.take() else {
            return;
        };
        let Some(idx) = self.tabs.iter().position(|t| pending.tab_ids.contains(&t.id)
            && (t.edits.has_pending() || t.edits.active.is_some()
                || (t.diagram.is_none() && matches!(t.schema_editor.as_ref(), Some(ObjectEditor::Table(e)) if e.has_changes())))) else { return };
        // Return to the exact dirty tab and keep the existing review/Guardian workflow.
        // Leaving is never resumed automatically: a failed/cancelled save keeps the work open.
        self.close_split_workspace();
        self.active_query_tab = idx;
        self.settings_open = false;
        self.focused_pane = 0;
        if matches!(self.tabs[idx].schema_editor.as_ref(), Some(ObjectEditor::Table(e)) if e.has_changes())
        {
            self.apply_action(Action::GenerateSchema);
        } else {
            self.apply_action(Action::PreviewEdits);
        }
    }

    pub(super) fn discard_before_leaving(&mut self) {
        let Some(pending) = self.pending_leave.take() else {
            return;
        };
        for tab in self
            .tabs
            .iter_mut()
            .filter(|t| pending.tab_ids.contains(&t.id))
        {
            tab.edits.cancel_active();
            tab.edits.clear();
            tab.schema_editor = None;
        }
        self.apply_action(pending.action);
    }

    /// Result replacement must never silently throw away edits, even through auto-reloads.
    pub(super) fn allow_result_replacement(&mut self, idx: usize) -> bool {
        if self.tab_has_unsaved_changes(idx) {
            self.error = Some("Save or discard this tab's changes before reloading, filtering, sorting, or changing pages.".into());
            false
        } else {
            true
        }
    }

    pub(super) fn replacement_would_lose_edits(&mut self, tab_id: u64) -> bool {
        let dirty = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .is_some_and(|tab| tab.edits.has_pending() || tab.edits.active.is_some());
        if dirty {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.stream = None;
                tab.edits.pending_source = None;
            }
            self.error = Some("The new result was not installed because this tab has unsaved edits. Save or discard them, then reload.".into());
        }
        dirty
    }

    /// "Discard all changes?" — the confirmation for a reload, re-run or draft reset.
    fn discard_changes_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let shortcut = if cfg!(target_os = "macos") {
            "⌘S"
        } else {
            "Ctrl+S"
        };
        let response = egui::Modal::new(egui::Id::new("discard_all_changes"))
            .frame(crate::components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(320.0);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Warning")
                        .size(15.0)
                        .strong()
                        .color(crate::style::palette::TEXT()),
                );
                ui.add_space(8.0);
                ui.label("Discard all changes?");
                ui.label(
                    egui::RichText::new(format!("Tip: you can commit changes with {shortcut}."))
                        .color(crate::style::palette::TEXT_WEAK()),
                );
                crate::components::dialog_footer(ui, |ui| {
                    if crate::components::button(ui, crate::icons::close(), "Cancel", true)
                        .clicked()
                    {
                        actions.push(Action::CancelLeaving);
                    }
                    if crate::components::primary_button(ui, crate::icons::trash(), "Discard", true)
                        .clicked()
                    {
                        actions.push(Action::DiscardBeforeLeaving);
                    }
                });
            });
        if response.should_close() {
            actions.push(Action::CancelLeaving);
        }
    }

    /// Whether the open New View / Trigger / Table / Routine editor holds work worth
    /// confirming before it is thrown away. A pristine draft is reset without a prompt.
    pub(super) fn draft_is_dirty(&self) -> bool {
        self.draft_is_dirty_at(self.active_query_tab)
    }

    pub(super) fn draft_is_dirty_at(&self, idx: usize) -> bool {
        use crate::schema::{ObjectEditor, ObjectMode, SchemaEditor, SchemaEditorMode};
        let Some(tab) = self.tabs.get(idx) else {
            return false;
        };
        match tab.schema_editor.as_ref() {
            Some(ObjectEditor::Table(e)) if e.mode == SchemaEditorMode::New => {
                let fresh = SchemaEditor::new_table(e.db_kind, None);
                !(e.table_name.trim().is_empty() || e.table_name.starts_with("untitled_table_"))
                    || !e.indexes.is_empty()
                    || !e.fks.is_empty()
                    || e.columns.len() != fresh.columns.len()
                    || e.columns.iter().zip(&fresh.columns).any(|(a, b)| {
                        a.name != b.name
                            || a.data_type != b.data_type
                            || a.nullable != b.nullable
                            || a.primary_key != b.primary_key
                            || a.default != b.default
                    })
            }
            Some(ObjectEditor::View(e)) if e.mode == ObjectMode::Create => {
                e.select_body.trim() != "SELECT" || !e.name.starts_with("untitled_view_")
            }
            Some(ObjectEditor::Trigger(e)) if e.mode == ObjectMode::Create => {
                !e.name.trim().is_empty()
                    || !e.body.trim().is_empty()
                    || !e.when_condition.trim().is_empty()
            }
            // A routine's fields are many; any open draft counts.
            Some(ObjectEditor::Routine(e)) => e.mode == ObjectMode::Create,
            _ => false,
        }
    }

    /// The action that starts the open draft over, if it is a *new* object (an edit of an
    /// existing one has nothing to reset to).
    fn draft_reset_action(&self) -> Option<Action> {
        use crate::schema::{ObjectEditor, ObjectMode, SchemaEditorMode};
        match self.tab().schema_editor.as_ref()? {
            ObjectEditor::Table(e) if e.mode == SchemaEditorMode::New => Some(Action::OpenNewTable),
            ObjectEditor::View(e) if e.mode == ObjectMode::Create => Some(Action::OpenNewView),
            ObjectEditor::Trigger(e) if e.mode == ObjectMode::Create => {
                Some(Action::OpenNewTrigger)
            }
            ObjectEditor::Routine(e) if e.mode == ObjectMode::Create => {
                Some(Action::OpenNewRoutine(e.kind))
            }
            _ => None,
        }
    }

    /// Reload (`cancel == false`) or leave (`cancel == true`) the open draft editor. A draft
    /// with work in it asks first; otherwise the returned action is ready to run.
    pub(super) fn draft_exit_action(&mut self, cancel: bool) -> Option<Action> {
        let action = if cancel {
            Action::CancelSchema
        } else if !self.draft_is_dirty() && self.tab().draft_tab {
            // A pristine draft has nothing to reload, and "New …" would open another tab.
            self.status_msg = "Nothing to reload — this draft is still blank".into();
            return None;
        } else {
            match self.draft_reset_action() {
                Some(action) => action,
                None => {
                    self.status_msg = "Nothing to reload — this draft isn't saved yet".into();
                    return None;
                }
            }
        };
        if self.draft_is_dirty() {
            // Discarding a draft that lives in its own tab closes that tab rather than
            // resetting it to a blank draft.
            let action = if self.tab().draft_tab {
                Action::CancelSchema
            } else {
                action
            };
            self.pending_leave = Some(PendingLeave {
                action,
                tab_ids: vec![self.tab().id],
            });
            return None;
        }
        Some(action)
    }

    pub(super) fn unsaved_changes_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(pending) = self.pending_leave.as_ref() else {
            return;
        };
        let labels: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(idx, t)| {
                pending.tab_ids.contains(&t.id) && self.tab_has_unsaved_changes(*idx)
            })
            .map(|(idx, tab)| {
                let label = self.tab_label(idx);
                self.connections
                    .iter()
                    .find(|c| Some(&c.id) == tab.conn_id.as_ref())
                    .map(|c| format!("{label} — {}", c.name))
                    .unwrap_or(label)
            })
            .collect();
        let can_save = self.busy == Busy::Idle;
        // Reloading, re-running, resetting or cancelling a draft all mean the same thing to the
        // user: "throw this away and start over". They get the short confirmation.
        if matches!(
            pending.action,
            Action::RunQuery
                | Action::RunCurrentQuery
                | Action::ReloadTableStructure
                | Action::CancelSchema
                | Action::OpenNewTable
                | Action::OpenNewView
                | Action::OpenNewTrigger
                | Action::OpenNewRoutine(_)
        ) {
            self.discard_changes_dialog(ctx, actions);
            return;
        }
        let response = egui::Modal::new(egui::Id::new("unsaved_changes"))
            .frame(crate::components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Keep your changes?")
                        .size(18.0)
                        .strong()
                        .color(crate::style::palette::TEXT()),
                );
                ui.add_space(6.0);
                ui.label("This action would discard changes in:");
                for label in labels {
                    ui.label(format!("• {label}"));
                }
                ui.add_space(8.0);
                ui.label("Save opens the review for one tab. After saving, try the action again.");
                crate::components::dialog_footer(ui, |ui| {
                    if crate::components::primary_button(
                        ui,
                        crate::icons::save(),
                        "Save…",
                        can_save,
                    )
                    .clicked()
                    {
                        actions.push(Action::SaveBeforeLeaving);
                    }
                    if crate::components::button(ui, crate::icons::trash(), "Discard", can_save)
                        .clicked()
                    {
                        actions.push(Action::DiscardBeforeLeaving);
                    }
                    if crate::components::button(ui, crate::icons::close(), "Cancel", true)
                        .clicked()
                    {
                        actions.push(Action::CancelLeaving);
                    }
                });
            });
        if response.should_close() {
            actions.push(Action::CancelLeaving);
        }
    }
}
