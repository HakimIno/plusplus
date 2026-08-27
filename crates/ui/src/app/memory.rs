//! Cross-tab result memory accounting and least-recently-used eviction.

use super::*;

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

impl DbGuiApp {
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
