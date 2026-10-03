//! UI panels grouped by responsibility.
//!
//! `DbGuiApp` panel entry points live in the owning module's implementation block:
//! - `chrome`, `connections`, `status`: application bars and connection controls.
//! - `query_console`, `query_workspace`, `editor_*`: SQL editing and execution controls.
//! - `results`, `pager`, `query_plan`, `live_log`, `details`: query output and inspection.
//! - `sidebar`, `saved_queries`, `tree`: navigation and saved-query trees.
//! - `schema_editor`, `schema_grid`, `structure`, `erd`: database structure views.
//! - `welcome`, `settings`, `updates`, `dialogs`, `import`: pages and modal workflows.
//!
//! Keep helpers private to their module, or `pub(super)` when another panel needs them.
//! App entry points use `pub(in crate::app)`; test helpers are re-exported below.

mod activity;
mod backup;
mod chrome;
mod connections;
mod details;
mod dialogs;
mod editor_assist;
mod editor_cursors;
mod editor_find;
mod erd;
mod import;
mod live_log;
mod pager;
mod query_console;
mod query_plan;
mod query_workspace;
mod results;
mod saved_queries;
mod schema_editor;
mod schema_grid;
mod settings;
mod sidebar;
mod status;
mod structure;
mod tree;
mod updates;
mod welcome;

#[cfg(test)]
pub(super) use editor_assist::error_under_caret;
#[cfg(test)]
pub(super) use editor_assist::toggle_comment_edit;
#[cfg(test)]
pub(super) use live_log::live_log_max_size;
#[cfg(test)]
pub(super) use saved_queries::grouped_favorites_for_connection;
#[cfg(test)]
pub(super) use saved_queries::grouped_history;
pub(super) use saved_queries::HistoryDay;
#[cfg(test)]
pub(super) use schema_grid::db_type_options;
