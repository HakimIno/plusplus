//! Cross-tab result memory accounting and least-recently-used eviction.

use super::*;

/// egui owns editor undo snapshots independently of QueryTab. Release those snapshots
/// when their tab is removed, including bulk closes and preview-tab replacement.
#[derive(Default)]
pub(super) struct TabTextEditMemory {
    ctx: Option<egui::Context>,
    ids: HashSet<egui::Id>,
}

impl TabTextEditMemory {
    pub(super) fn track(&mut self, ctx: &egui::Context, id: egui::Id) {
        self.ctx.get_or_insert_with(|| ctx.clone());
        self.ids.insert(id);
    }
}

impl Drop for TabTextEditMemory {
    fn drop(&mut self) {
        if let Some(ctx) = &self.ctx {
            ctx.data_mut(|data| {
                for &id in &self.ids {
                    data.remove::<egui::text_edit::TextEditState>(id);
                }
            });
        }
    }
}

impl QueryTab {
    pub(super) fn estimated_result_memory_bytes(&self) -> usize {
        let result = self
            .result
            .as_ref()
            .map_or(0, QueryResult::estimated_memory_bytes);
        let parked_batch_results = self
            .batch_results
            .iter()
            .filter_map(|stored| stored.result.as_ref())
            .map(QueryResult::estimated_memory_bytes)
            .sum::<usize>();
        let display_order = self.row_order.capacity() * std::mem::size_of::<usize>();
        let pending_stream = self.stream.as_ref().map_or(0, |stream| {
            stream.pending_rows.capacity() * std::mem::size_of::<Vec<dbcore::Value>>()
                + stream
                    .pending_rows
                    .iter()
                    .map(|row| {
                        row.capacity() * std::mem::size_of::<dbcore::Value>()
                            + row
                                .iter()
                                .map(|value| {
                                    value
                                        .estimated_memory_bytes()
                                        .saturating_sub(std::mem::size_of::<dbcore::Value>())
                                })
                                .sum::<usize>()
                    })
                    .sum::<usize>()
        });
        result + parked_batch_results + display_order + pending_stream
    }
}

/// How long the app must sit untouched before freed heap pages are handed back to the OS.
const IDLE_TRIM_AFTER: std::time::Duration = std::time::Duration::from_secs(20);

/// Return freed-but-retained heap pages to the OS. macOS's malloc keeps them resident after
/// a big result is dropped, so Activity Monitor would otherwise show the peak forever.
fn release_free_memory() {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means "all zones"; goal 0 means "as much as possible".
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}

impl DbGuiApp {
    /// Once the user has stopped interacting and no query runs, trim the allocator a single
    /// time so memory falls back after heavy use instead of sitting at its peak.
    pub(super) fn trim_memory_when_idle(&mut self, ctx: &egui::Context) {
        let active = self.busy != Busy::Idle
            || ctx.input(|i| !i.raw.events.is_empty() || i.pointer.delta() != egui::Vec2::ZERO);
        if active {
            self.last_active = std::time::Instant::now();
            self.idle_trimmed = false;
            return;
        }
        if self.idle_trimmed {
            return;
        }
        let idle = self.last_active.elapsed();
        if idle < IDLE_TRIM_AFTER {
            ctx.request_repaint_after(IDLE_TRIM_AFTER - idle);
            return;
        }
        self.idle_trimmed = true;
        release_free_memory();
    }

    pub(super) fn touch_result(&mut self, idx: usize) {
        self.result_access_clock = self.result_access_clock.saturating_add(1);
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.result_last_used = self.result_access_clock;
        }
    }

    pub(super) fn total_result_memory_bytes(&self) -> usize {
        self.tabs
            .iter()
            .map(QueryTab::estimated_result_memory_bytes)
            .sum()
    }

    /// Keep the active tab and any tab with uncommitted edits. Inactive clean results are
    /// released from least- to most-recently used until the shared budget is satisfied.
    pub(super) fn enforce_result_memory_budget(&mut self) -> usize {
        let mut total = self.total_result_memory_bytes();
        let mut released = 0usize;
        while total > self.result_memory_budget {
            let candidate = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(idx, tab)| {
                    *idx != self.active_query_tab
                        && (tab.result.is_some()
                            || tab
                                .batch_results
                                .iter()
                                .any(|stored| stored.result.is_some()))
                        && tab.stream.is_none()
                        && !tab.edits.has_pending()
                })
                .min_by_key(|(_, tab)| tab.result_last_used)
                .map(|(idx, _)| idx);
            let Some(idx) = candidate else {
                break;
            };
            let before = self.tabs[idx].estimated_result_memory_bytes();
            let tab = &mut self.tabs[idx];
            tab.result = None;
            tab.clear_batch_results();
            tab.row_order.clear();
            tab.row_order.shrink_to_fit();
            tab.sort = None;
            tab.selection.clear();
            tab.result_evicted = true;
            tab.page_exhausted = false;
            released = released.saturating_add(before);
            total = total.saturating_sub(before);
        }
        released
    }
}
