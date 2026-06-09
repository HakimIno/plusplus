//! TablePlus-style result filtering.
//!
//! The filter operates **client-side** on the already-loaded [`QueryResult`]: every row is
//! held in memory, so filtering is just selecting the row indices that satisfy a set of
//! conditions — the same shape the grid already consumes via `row_order`. A condition is a
//! `(column, operator, value)` triple plus an on/off toggle; conditions combine with either
//! "match all" (AND) or "match any" (OR), mirroring TablePlus's filter bar.
//!
//! The UI ([`ui`]) renders one row per condition — `[✓] column ▾  operator ▾  value  Apply
//! − +` — followed by a toolbar with the All/Any switch and Clear / Apply All. It returns a
//! [`FilterEvent`] when the user asks to (re)apply or clear, which the app turns into a
//! `recompute_view`.

use dbcore::{QueryResult, Value};

/// A comparison operator for a single filter condition. Ordering operators (`>`, `<`, …)
/// compare numerically when both sides parse as numbers and fall back to text otherwise; the
/// text operators are case-insensitive.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Contains,
    NotContains,
    Equals,
    NotEquals,
    BeginsWith,
    EndsWith,
    Greater,
    Less,
    GreaterEq,
    LessEq,
    IsNull,
    IsNotNull,
    IsEmpty,
    IsNotEmpty,
}

impl FilterOp {
    /// All operators, in the order they appear in the dropdown.
    pub const ALL: [FilterOp; 14] = [
        FilterOp::Contains,
        FilterOp::NotContains,
        FilterOp::Equals,
        FilterOp::NotEquals,
        FilterOp::BeginsWith,
        FilterOp::EndsWith,
        FilterOp::Greater,
        FilterOp::Less,
        FilterOp::GreaterEq,
        FilterOp::LessEq,
        FilterOp::IsNull,
        FilterOp::IsNotNull,
        FilterOp::IsEmpty,
        FilterOp::IsNotEmpty,
    ];

    /// Human-readable label for the operator dropdown.
    pub fn label(self) -> &'static str {
        match self {
            FilterOp::Contains => "contains",
            FilterOp::NotContains => "does not contain",
            FilterOp::Equals => "equals",
            FilterOp::NotEquals => "not equal",
            FilterOp::BeginsWith => "begins with",
            FilterOp::EndsWith => "ends with",
            FilterOp::Greater => "greater than",
            FilterOp::Less => "less than",
            FilterOp::GreaterEq => "greater or equal",
            FilterOp::LessEq => "less or equal",
            FilterOp::IsNull => "is null",
            FilterOp::IsNotNull => "is not null",
            FilterOp::IsEmpty => "is empty",
            FilterOp::IsNotEmpty => "is not empty",
        }
    }

    /// Whether this operator reads the typed value. Null/empty checks ignore it, so the
    /// value box is disabled for them.
    pub fn needs_value(self) -> bool {
        !matches!(
            self,
            FilterOp::IsNull | FilterOp::IsNotNull | FilterOp::IsEmpty | FilterOp::IsNotEmpty
        )
    }
}

/// A single filter condition: compare `column`'s cell against `value` using `op`. Disabled
/// conditions are ignored without being deleted (the on/off checkbox).
#[derive(Clone)]
pub struct Condition {
    pub enabled: bool,
    pub column: usize,
    pub op: FilterOp,
    pub value: String,
}

impl Default for Condition {
    fn default() -> Self {
        Self {
            enabled: true,
            column: 0,
            op: FilterOp::Contains,
            value: String::new(),
        }
    }
}

/// How multiple enabled conditions combine.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Conjunction {
    /// Every enabled condition must match (AND).
    All,
    /// At least one enabled condition must match (OR).
    Any,
}

/// The whole filter bar's state, owned by the app and persisted across frames.
pub struct FilterState {
    /// Whether the filter bar is shown (toggled from the toolbar / Cmd-F).
    pub visible: bool,
    pub conjunction: Conjunction,
    pub conditions: Vec<Condition>,
}

impl Default for FilterState {
    fn default() -> Self {
        Self {
            visible: false,
            conjunction: Conjunction::All,
            conditions: vec![Condition::default()],
        }
    }
}

impl FilterState {
    /// Reset to a single empty condition (used by Clear and when a new result arrives).
    pub fn reset(&mut self) {
        self.conjunction = Conjunction::All;
        self.conditions = vec![Condition::default()];
    }

    /// Clamp every condition's column index into `[0, ncols)` so a result with fewer columns
    /// can't index out of bounds after a re-query.
    pub fn clamp_columns(&mut self, ncols: usize) {
        let max = ncols.saturating_sub(1);
        for c in &mut self.conditions {
            c.column = c.column.min(max);
        }
    }

    /// Whether any condition is currently enabled (and thus actually narrows the rows).
    pub fn is_active(&self) -> bool {
        self.conditions.iter().any(|c| c.enabled)
    }
}

/// What the user asked the filter bar to do this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterEvent {
    /// (Re)apply the current conditions to the result set.
    Apply,
    /// Clear all conditions back to a single empty one and show every row.
    Clear,
}

// --- matching ----------------------------------------------------------------

/// Does `row` satisfy the enabled `conditions` under `conj`? With no enabled conditions,
/// every row passes (an empty filter is a no-op).
pub fn matches_row(row: &[Value], conditions: &[Condition], conj: Conjunction) -> bool {
    let mut active = conditions.iter().filter(|c| c.enabled).peekable();
    if active.peek().is_none() {
        return true;
    }
    match conj {
        Conjunction::All => active.all(|c| cell_matches(row.get(c.column), c)),
        Conjunction::Any => active.any(|c| cell_matches(row.get(c.column), c)),
    }
}

/// The numeric value of a cell for ordering comparisons, if it has one.
fn numeric(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Text(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Evaluate one condition against one (possibly missing) cell.
fn cell_matches(cell: Option<&Value>, c: &Condition) -> bool {
    let Some(v) = cell else { return false };
    use FilterOp::*;

    match c.op {
        IsNull => return v.is_null(),
        IsNotNull => return !v.is_null(),
        IsEmpty => return v.is_null() || v.display().is_empty(),
        IsNotEmpty => return !(v.is_null() || v.display().is_empty()),
        _ => {}
    }

    // NULL never matches a value-based operator (you want `is null` for that).
    if v.is_null() {
        return false;
    }

    // Ordering operators: prefer a numeric comparison, fall back to lexicographic.
    if matches!(c.op, Greater | Less | GreaterEq | LessEq) {
        let ord = match (numeric(v), c.value.trim().parse::<f64>()) {
            (Some(a), Ok(b)) => a.partial_cmp(&b),
            _ => Some(v.display().cmp(&c.value)),
        };
        let Some(ord) = ord else { return false };
        return match c.op {
            Greater => ord.is_gt(),
            Less => ord.is_lt(),
            GreaterEq => ord.is_ge(),
            LessEq => ord.is_le(),
            _ => unreachable!(),
        };
    }

    // Text operators: case-insensitive substring/equality.
    let hay = v.display().to_lowercase();
    let needle = c.value.to_lowercase();
    match c.op {
        Contains => hay.contains(&needle),
        NotContains => !hay.contains(&needle),
        Equals => hay == needle,
        NotEquals => hay != needle,
        BeginsWith => hay.starts_with(&needle),
        EndsWith => hay.ends_with(&needle),
        _ => true,
    }
}

/// Compute the display order for `result`: the indices of rows passing the filter. The
/// caller applies any active sort on top of this.
pub fn passing_rows(result: &QueryResult, state: &FilterState) -> Vec<usize> {
    (0..result.rows.len())
        .filter(|&r| matches_row(&result.rows[r], &state.conditions, state.conjunction))
        .collect()
}

// --- UI ----------------------------------------------------------------------

/// Render the filter bar. Mutates `state` in place (text edits, dropdowns, add/remove) and
/// returns a [`FilterEvent`] when the user presses Apply / Clear (or hits Enter in a value
/// box). `columns` are the result's column names, used for the column dropdown.
pub fn ui(ui: &mut egui::Ui, state: &mut FilterState, columns: &[String]) -> Option<FilterEvent> {
    use crate::style::palette;

    let mut event: Option<FilterEvent> = None;
    // Deferred structural edits — we can't mutate the Vec while iterating it.
    let mut remove_at: Option<usize> = None;
    let mut add_after: Option<usize> = None;

    ui.add_space(4.0);

    let n = state.conditions.len();
    for (i, cond) in state.conditions.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.add_space(2.0);

            // On/off toggle for this condition.
            ui.checkbox(&mut cond.enabled, "")
                .on_hover_text("Enable / disable this condition");

            // Column picker.
            egui::ComboBox::from_id_salt(("filter_col", i))
                .width(150.0)
                .selected_text(columns.get(cond.column).map(String::as_str).unwrap_or("—"))
                .show_ui(ui, |ui| {
                    for (c, name) in columns.iter().enumerate() {
                        ui.selectable_value(&mut cond.column, c, name);
                    }
                });

            // Operator picker.
            egui::ComboBox::from_id_salt(("filter_op", i))
                .width(160.0)
                .selected_text(cond.op.label())
                .show_ui(ui, |ui| {
                    for op in FilterOp::ALL {
                        ui.selectable_value(&mut cond.op, op, op.label());
                    }
                });

            // The + / − controls are laid out from the right; the value box then fills the
            // gap between the operator dropdown and the buttons. Putting them in one
            // right-to-left scope is what lets the value stretch.
            let btn = egui::vec2(28.0, crate::style::CONTROL_H);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("+").min_size(btn))
                    .on_hover_text("Add condition")
                    .clicked()
                {
                    add_after = Some(i);
                }
                if ui
                    .add_enabled(n > 1, egui::Button::new("−").min_size(btn))
                    .on_hover_text("Remove condition")
                    .clicked()
                {
                    remove_at = Some(i);
                }
                ui.add_space(2.0);

                // Value box — fills the remaining width at the shared control height. Disabled
                // for null/empty operators; Enter applies, matching TablePlus.
                let needs_value = cond.op.needs_value();
                let width = ui.available_width();
                let resp = ui
                    .add_enabled_ui(needs_value, |ui| {
                        let hint = if needs_value { "value" } else { "" };
                        crate::style::text_input(ui, &mut cond.value, hint, width)
                    })
                    .inner;
                if needs_value
                    && resp.lost_focus()
                    && ui.input(|inp| inp.key_pressed(egui::Key::Enter))
                {
                    event = Some(FilterEvent::Apply);
                }
            });
        });
        ui.add_space(3.0);
    }

    // Toolbar: All/Any switch on the left, Clear / Apply on the right.
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.colored_label(palette::TEXT_FAINT(), "match");
        let mut conj = state.conjunction;
        egui::ComboBox::from_id_salt("filter_conj")
            .width(64.0)
            .selected_text(match conj {
                Conjunction::All => "all",
                Conjunction::Any => "any",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut conj, Conjunction::All, "all");
                ui.selectable_value(&mut conj, Conjunction::Any, "any");
            });
        if conj != state.conjunction {
            state.conjunction = conj;
            event = Some(FilterEvent::Apply);
        }
        ui.colored_label(palette::TEXT_FAINT(), "of the following");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::icons::primary_button(ui, crate::icons::filter(), "Apply All", true)
                .on_hover_text("Apply filter  (Enter)")
                .clicked()
            {
                event = Some(FilterEvent::Apply);
            }
            if ui.button("Clear").clicked() {
                event = Some(FilterEvent::Clear);
            }
        });
    });
    ui.add_space(4.0);

    // Apply deferred structural edits.
    if let Some(i) = add_after {
        let template = state.conditions[i].column;
        state.conditions.insert(
            i + 1,
            Condition {
                column: template,
                ..Condition::default()
            },
        );
    }
    if let Some(i) = remove_at {
        if state.conditions.len() > 1 {
            state.conditions.remove(i);
            event = Some(FilterEvent::Apply);
        }
    }

    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::Value;

    fn cond(column: usize, op: FilterOp, value: &str) -> Condition {
        Condition {
            enabled: true,
            column,
            op,
            value: value.into(),
        }
    }

    fn row(vals: &[Value]) -> Vec<Value> {
        vals.to_vec()
    }

    #[test]
    fn text_operators_are_case_insensitive() {
        let r = row(&[Value::Text("Hello World".into())]);
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::Contains, "hello")],
            Conjunction::All
        ));
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::BeginsWith, "HELLO")],
            Conjunction::All
        ));
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::EndsWith, "world")],
            Conjunction::All
        ));
        assert!(!matches_row(
            &r,
            &[cond(0, FilterOp::Equals, "hello")],
            Conjunction::All
        ));
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::NotContains, "xyz")],
            Conjunction::All
        ));
    }

    #[test]
    fn numeric_ordering_uses_numbers_not_strings() {
        let r = row(&[Value::Int(9)]);
        // "9" > "100" lexicographically, but 9 < 100 numerically — we want numeric.
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::Less, "100")],
            Conjunction::All
        ));
        assert!(!matches_row(
            &r,
            &[cond(0, FilterOp::Greater, "100")],
            Conjunction::All
        ));
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::GreaterEq, "9")],
            Conjunction::All
        ));
    }

    #[test]
    fn null_and_empty_checks() {
        let nul = row(&[Value::Null]);
        let empty = row(&[Value::Text(String::new())]);
        let filled = row(&[Value::Text("x".into())]);
        assert!(matches_row(
            &nul,
            &[cond(0, FilterOp::IsNull, "")],
            Conjunction::All
        ));
        assert!(matches_row(
            &empty,
            &[cond(0, FilterOp::IsEmpty, "")],
            Conjunction::All
        ));
        assert!(matches_row(
            &filled,
            &[cond(0, FilterOp::IsNotEmpty, "")],
            Conjunction::All
        ));
        // A value operator never matches NULL.
        assert!(!matches_row(
            &nul,
            &[cond(0, FilterOp::Contains, "")],
            Conjunction::All
        ));
    }

    #[test]
    fn all_vs_any_conjunction() {
        let r = row(&[Value::Text("cat".into()), Value::Int(5)]);
        let conds = vec![
            cond(0, FilterOp::Equals, "cat"),
            cond(1, FilterOp::Greater, "10"),
        ];
        assert!(!matches_row(&r, &conds, Conjunction::All)); // second fails
        assert!(matches_row(&r, &conds, Conjunction::Any)); // first passes
    }

    #[test]
    fn disabled_and_empty_conditions_pass_everything() {
        let r = row(&[Value::Text("anything".into())]);
        let disabled = vec![Condition {
            enabled: false,
            ..cond(0, FilterOp::Equals, "nope")
        }];
        assert!(matches_row(&r, &disabled, Conjunction::All));
        assert!(matches_row(&r, &[], Conjunction::All));
    }
}
