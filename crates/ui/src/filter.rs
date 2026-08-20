//! TablePlus-style result filtering.
//!
//! The filter operates **client-side** on the already-loaded [`QueryResult`]: every row is
//! held in memory, so filtering is just selecting the row indices that satisfy a set of
//! conditions — the same shape the grid already consumes via `row_order`. A condition is a
//! `(column, operator, value)` triple plus an on/off toggle; conditions combine with either
//! "match all" (AND) or "match any" (OR), mirroring TablePlus's filter bar.
//!
//! The UI ([`ui`]) renders one compact row per condition — `[✓] column ▾ operator ▾ value − +`
//! — followed by a toolbar with the All/Any switch and Clear / Apply All. It returns a
//! [`FilterEvent`] when the user asks to (re)apply or clear, which the app turns into a
//! `recompute_view`.

use dbcore::{QueryResult, Value};

/// A comparison operator for a single filter condition. Ordering operators (`>`, `<`, …)
/// and `BETWEEN` compare numerically when both sides parse as numbers and fall back to text
/// (case-insensitive) otherwise; the text operators are case-insensitive.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Equals,
    NotEquals,
    Less,
    Greater,
    LessEq,
    GreaterEq,
    In,
    NotIn,
    IsNull,
    IsNotNull,
    Between,
    NotBetween,
    Contains,
    NotContains,
    BeginsWith,
    EndsWith,
}

impl FilterOp {
    /// Operators grouped exactly as they appear in the dropdown; each inner slice is drawn as
    /// a block with a separator between blocks.
    pub const GROUPS: [&'static [FilterOp]; 6] = [
        &[
            FilterOp::Equals,
            FilterOp::NotEquals,
            FilterOp::Less,
            FilterOp::Greater,
            FilterOp::LessEq,
            FilterOp::GreaterEq,
        ],
        &[FilterOp::In, FilterOp::NotIn],
        &[FilterOp::IsNull, FilterOp::IsNotNull],
        &[FilterOp::Between, FilterOp::NotBetween],
        &[FilterOp::Contains, FilterOp::NotContains],
        &[FilterOp::BeginsWith, FilterOp::EndsWith],
    ];

    /// Short label shown in the dropdown and selected box — a symbol for the comparison
    /// operators, an uppercase keyword or title-case phrase for the rest. Symbols keep the
    /// common operators compact and instantly recognisable.
    pub fn label(self) -> &'static str {
        match self {
            FilterOp::Equals => "=",
            FilterOp::NotEquals => "<>",
            FilterOp::Less => "<",
            FilterOp::Greater => ">",
            FilterOp::LessEq => "<=",
            FilterOp::GreaterEq => ">=",
            FilterOp::In => "IN",
            FilterOp::NotIn => "NOT IN",
            FilterOp::IsNull => "IS NULL",
            FilterOp::IsNotNull => "IS NOT NULL",
            FilterOp::Between => "BETWEEN",
            FilterOp::NotBetween => "NOT BETWEEN",
            FilterOp::Contains => "Contains",
            FilterOp::NotContains => "Not contains",
            FilterOp::BeginsWith => "Has prefix",
            FilterOp::EndsWith => "Has suffix",
        }
    }

    /// Plain-language description shown on hover, so the symbols stay discoverable.
    pub fn description(self) -> &'static str {
        match self {
            FilterOp::Equals => "equals",
            FilterOp::NotEquals => "not equal",
            FilterOp::Less => "less than",
            FilterOp::Greater => "greater than",
            FilterOp::LessEq => "less than or equal",
            FilterOp::GreaterEq => "greater than or equal",
            FilterOp::In => "in a comma-separated list",
            FilterOp::NotIn => "not in a comma-separated list",
            FilterOp::IsNull => "is null",
            FilterOp::IsNotNull => "is not null",
            FilterOp::Between => "between two values (inclusive)",
            FilterOp::NotBetween => "not between two values",
            FilterOp::Contains => "contains the text",
            FilterOp::NotContains => "does not contain the text",
            FilterOp::BeginsWith => "begins with the text",
            FilterOp::EndsWith => "ends with the text",
        }
    }

    /// Placeholder for the value box, hinting the expected input for multi-value operators.
    pub fn value_hint(self) -> &'static str {
        match self {
            FilterOp::In | FilterOp::NotIn => "a, b, c",
            FilterOp::Between | FilterOp::NotBetween => "min, max",
            _ => "value",
        }
    }

    /// Whether this operator reads the typed value. Null checks ignore it, so the value box
    /// is disabled for them.
    pub fn needs_value(self) -> bool {
        !matches!(self, FilterOp::IsNull | FilterOp::IsNotNull)
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

impl Condition {
    /// Whether this condition actually narrows the result. A disabled condition never does;
    /// neither does an enabled value-operator (`contains`, `equals`, …) whose value box is
    /// still blank — that's a half-typed filter, so it must pass every row rather than, say,
    /// dropping NULL cells. Null/empty operators need no value and are effective when enabled.
    pub fn is_effective(&self) -> bool {
        self.enabled && (!self.op.needs_value() || !self.value.trim().is_empty())
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

    /// Whether any condition actually narrows the rows (enabled, and not a blank value box).
    pub fn is_active(&self) -> bool {
        self.conditions.iter().any(|c| c.is_effective())
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

/// Does `row` satisfy the `conditions` under `conj`? Only *effective* conditions count (see
/// [`Condition::is_effective`]); with none, every row passes (an empty filter is a no-op).
pub fn matches_row(row: &[Value], conditions: &[Condition], conj: Conjunction) -> bool {
    let mut active = conditions.iter().filter(|c| c.is_effective()).peekable();
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

/// Order `v` against the string `s`: numerically when both parse as numbers, else
/// case-insensitive lexicographic. `None` only for incomparable floats (NaN).
fn compare(v: &Value, s: &str) -> Option<std::cmp::Ordering> {
    match (numeric(v), s.trim().parse::<f64>()) {
        (Some(a), Ok(b)) => a.partial_cmp(&b),
        _ => Some(v.display().to_lowercase().cmp(&s.trim().to_lowercase())),
    }
}

/// Split a `BETWEEN` value box into its two bounds. Accepts `min, max` or `min and max`.
fn two_bounds(s: &str) -> Option<(String, String)> {
    let (a, b) = if let Some((a, b)) = s.split_once(',') {
        (a, b)
    } else if let Some(idx) = s.to_lowercase().find(" and ") {
        (&s[..idx], &s[idx + 5..])
    } else {
        return None;
    };
    let (a, b) = (a.trim(), b.trim());
    (!a.is_empty() && !b.is_empty()).then(|| (a.to_string(), b.to_string()))
}

/// Whether `v` equals the string `s` (numeric when both are numbers, else case-insensitive).
fn value_equals(v: &Value, s: &str) -> bool {
    match (numeric(v), s.trim().parse::<f64>()) {
        (Some(a), Ok(b)) => a == b,
        _ => v.display().to_lowercase() == s.trim().to_lowercase(),
    }
}

/// Evaluate one condition against one (possibly missing) cell.
fn cell_matches(cell: Option<&Value>, c: &Condition) -> bool {
    let Some(v) = cell else { return false };
    use FilterOp::*;

    match c.op {
        IsNull => return v.is_null(),
        IsNotNull => return !v.is_null(),
        _ => {}
    }

    // NULL never matches a value-based operator (you want `is null` for that).
    if v.is_null() {
        return false;
    }

    match c.op {
        // Ordering: numeric when possible, lexicographic otherwise.
        Greater | Less | GreaterEq | LessEq => {
            let Some(ord) = compare(v, &c.value) else {
                return false;
            };
            match c.op {
                Greater => ord.is_gt(),
                Less => ord.is_lt(),
                GreaterEq => ord.is_ge(),
                LessEq => ord.is_le(),
                _ => unreachable!(),
            }
        }
        // Inclusive range; an unparseable range matches nothing (and `NOT BETWEEN` everything).
        Between | NotBetween => {
            let within = two_bounds(&c.value).is_some_and(|(lo, hi)| {
                matches!(compare(v, &lo), Some(o) if o.is_ge())
                    && matches!(compare(v, &hi), Some(o) if o.is_le())
            });
            if c.op == Between {
                within
            } else {
                !within
            }
        }
        // Membership in a comma-separated list (blank items ignored).
        In | NotIn => {
            let found = c
                .value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .any(|item| value_equals(v, item));
            if c.op == In {
                found
            } else {
                !found
            }
        }
        // Text operators: case-insensitive.
        Contains | NotContains | Equals | NotEquals | BeginsWith | EndsWith => {
            let hay = v.display().to_lowercase();
            let needle = c.value.to_lowercase();
            match c.op {
                Contains => hay.contains(&needle),
                NotContains => !hay.contains(&needle),
                Equals => value_equals(v, &c.value),
                NotEquals => !value_equals(v, &c.value),
                BeginsWith => hay.starts_with(&needle),
                EndsWith => hay.ends_with(&needle),
                _ => unreachable!(),
            }
        }
        IsNull | IsNotNull => unreachable!("handled above"),
    }
}

/// Compute the display order for `result`: the indices of rows passing the filter. The
/// caller applies any active sort on top of this.
pub fn passing_rows(result: &QueryResult, state: &FilterState) -> Vec<usize> {
    if !state.is_active() {
        return (0..result.rows.len()).collect();
    }
    (0..result.rows.len())
        .filter(|&r| matches_row(&result.rows[r], &state.conditions, state.conjunction))
        .collect()
}

// --- UI ----------------------------------------------------------------------

/// Render the filter bar. Mutates `state` in place (text edits, dropdowns, add/remove) and
/// returns a [`FilterEvent`] when the user presses Apply / Clear (or hits Enter in a value
/// box). `columns` are the result's column names, used for the column dropdown.
pub fn ui(ui: &mut egui::Ui, state: &mut FilterState, columns: &[String]) -> Option<FilterEvent> {
    use crate::style::{palette, CONTROL_H};

    let mut event: Option<FilterEvent> = None;
    // Deferred structural edits — we can't mutate the Vec while iterating it.
    let mut remove_at: Option<usize> = None;
    let mut add_after: Option<usize> = None;

    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);

    let n = state.conditions.len();
    let row_w = ui.available_width();
    for (i, cond) in state.conditions.iter_mut().enumerate() {
        ui.allocate_ui_with_layout(
            egui::vec2(row_w, CONTROL_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                crate::components::accent_checkbox(ui, true, &mut cond.enabled, None)
                    .on_hover_text("Enable / disable this condition");

                egui::ComboBox::from_id_salt(("filter_col", i))
                    .width(128.0)
                    .selected_text(columns.get(cond.column).map(String::as_str).unwrap_or("—"))
                    .show_ui(ui, |ui| {
                        for (c, name) in columns.iter().enumerate() {
                            ui.selectable_value(&mut cond.column, c, name);
                        }
                    });

                egui::ComboBox::from_id_salt(("filter_op", i))
                    .width(108.0)
                    .selected_text(cond.op.label())
                    .show_ui(ui, |ui| {
                        for (gi, group) in FilterOp::GROUPS.iter().enumerate() {
                            if gi > 0 {
                                ui.separator();
                            }
                            for &op in *group {
                                ui.selectable_value(&mut cond.op, op, op.label())
                                    .on_hover_text(op.description());
                            }
                        }
                    });

                // Right-to-left so + / − / Apply sit on the trailing edge and the value
                // box stretches into whatever width remains, the same on every row.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::components::Btn::ghost_icon(crate::icons::plus())
                        .tooltip("Add condition")
                        .show(ui)
                        .clicked()
                    {
                        add_after = Some(i);
                    }
                    if crate::components::Btn::ghost_icon(crate::icons::minus())
                        .tooltip("Remove condition")
                        .enabled(n > 1)
                        .show(ui)
                        .clicked()
                    {
                        remove_at = Some(i);
                    }
                    if crate::components::Btn::ghost_icon(crate::icons::filter())
                        .tooltip("Apply")
                        .show(ui)
                        .clicked()
                    {
                        event = Some(FilterEvent::Apply);
                    }

                    let needs_value = cond.op.needs_value();
                    let width = ui.available_width();
                    let resp = ui
                        .add_enabled_ui(needs_value, |ui| {
                            let hint = if needs_value {
                                cond.op.value_hint()
                            } else {
                                ""
                            };
                            crate::components::text_input(ui, &mut cond.value, hint, width)
                        })
                        .inner;
                    if needs_value
                        && resp.lost_focus()
                        && ui.input(|inp| inp.key_pressed(egui::Key::Enter))
                    {
                        event = Some(FilterEvent::Apply);
                    }
                });
            },
        );
    }

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), CONTROL_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(
                egui::Image::new(crate::icons::filter())
                    .fit_to_exact_size(egui::Vec2::splat(14.0))
                    .tint(palette::TEXT_WEAK()),
            )
            .on_hover_text("How conditions combine");
            let mut conj = state.conjunction;
            egui::ComboBox::from_id_salt("filter_conj")
                .width(56.0)
                .selected_text(match conj {
                    Conjunction::All => "All",
                    Conjunction::Any => "Any",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut conj, Conjunction::All, "All");
                    ui.selectable_value(&mut conj, Conjunction::Any, "Any");
                });
            if conj != state.conjunction {
                state.conjunction = conj;
                event = Some(FilterEvent::Apply);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::components::primary_button(ui, crate::icons::filter(), "Apply All", true)
                    .on_hover_text("Apply every enabled condition")
                    .clicked()
                {
                    event = Some(FilterEvent::Apply);
                }
                if crate::components::Btn::new("Clear").show(ui).clicked() {
                    event = Some(FilterEvent::Clear);
                }
            });
        },
    );

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
    fn null_checks() {
        let nul = row(&[Value::Null]);
        let filled = row(&[Value::Text("x".into())]);
        assert!(matches_row(
            &nul,
            &[cond(0, FilterOp::IsNull, "")],
            Conjunction::All
        ));
        assert!(matches_row(
            &filled,
            &[cond(0, FilterOp::IsNotNull, "")],
            Conjunction::All
        ));
        // A value operator never matches NULL (with an actual value to compare against).
        assert!(!matches_row(
            &nul,
            &[cond(0, FilterOp::Contains, "x")],
            Conjunction::All
        ));
    }

    #[test]
    fn in_list_matches_any_item() {
        let r = row(&[Value::Text("cat".into())]);
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::In, "dog, CAT, bird")],
            Conjunction::All
        ));
        assert!(!matches_row(
            &r,
            &[cond(0, FilterOp::NotIn, "dog, cat")],
            Conjunction::All
        ));
        // Numeric items compare as numbers, so "007" matches 7.
        let n = row(&[Value::Int(7)]);
        assert!(matches_row(
            &n,
            &[cond(0, FilterOp::In, "1, 007, 9")],
            Conjunction::All
        ));
    }

    #[test]
    fn between_is_inclusive_and_numeric() {
        let r = row(&[Value::Int(50)]);
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::Between, "10, 100")],
            Conjunction::All
        ));
        // Inclusive at the bounds.
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::Between, "50 and 100")],
            Conjunction::All
        ));
        assert!(!matches_row(
            &r,
            &[cond(0, FilterOp::Between, "60, 100")],
            Conjunction::All
        ));
        assert!(matches_row(
            &r,
            &[cond(0, FilterOp::NotBetween, "60, 100")],
            Conjunction::All
        ));
        // A range with only one bound parses to nothing → BETWEEN matches no row.
        assert!(!matches_row(
            &r,
            &[cond(0, FilterOp::Between, "50")],
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

    #[test]
    fn blank_value_condition_is_a_no_op_even_for_null_cells() {
        // The default filter bar holds one enabled `contains` condition with an empty value.
        // It must not narrow anything — not even rows whose target cell is NULL — until the
        // user actually types a value. (Regression: a fresh result showed "0 of N rows".)
        let default = vec![Condition::default()];
        assert!(!FilterState::default().is_active());
        assert!(matches_row(&[Value::Null], &default, Conjunction::All));
        assert!(matches_row(
            &[Value::Text("x".into())],
            &default,
            Conjunction::All
        ));
        // Once a value is typed, it filters again (and a value op still skips NULLs).
        let typed = vec![cond(0, FilterOp::Contains, "x")];
        assert!(matches_row(
            &[Value::Text("xyz".into())],
            &typed,
            Conjunction::All
        ));
        assert!(!matches_row(&[Value::Null], &typed, Conjunction::All));
        // A no-value operator (is null) stays effective with an empty value box.
        assert!(matches_row(
            &[Value::Null],
            &[cond(0, FilterOp::IsNull, "")],
            Conjunction::All
        ));
    }
}
