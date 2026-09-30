//! Erd rendering and interaction.

use crate::app::{Action, DbGuiApp};
use crate::components;
use crate::icons;
use crate::style::palette;

/// Draw the diagram content inside the scene: FK curves first (under), then the
/// draggable table boxes. All coordinates are scene-local; `egui::Scene` applies
/// the pan/zoom transform around us.
fn erd_canvas(ui: &mut egui::Ui, erd: &mut crate::erd::ErDiagram) {
    use crate::erd::{HEADER_H, ROW_H};

    // Node titles get one point over Body for hierarchy; Heading is avoided on purpose —
    // its custom font family only exists once the app installs fonts (not in headless tests).
    let title_font = egui::FontId::proportional(13.5);
    let body_font = egui::TextStyle::Body.resolve(ui.style());
    let small_font = egui::TextStyle::Small.resolve(ui.style());
    let painter = ui.painter().clone();

    // Nothing below allocates ui space (it's all painter + interact), so tell the
    // scene the content bounds explicitly — zoom-to-fit and the first frame's clip
    // rect are computed from `min_rect` and would otherwise see an empty scene.
    let mut bounds = egui::Rect::NOTHING;
    for node in &erd.nodes {
        bounds = bounds.union(node.rect());
    }
    ui.expand_to_include_rect(bounds.expand(60.0));

    // The scene clips us to the visible viewport (in scene coordinates): everything
    // fully outside it can be skipped, which is what keeps huge schemas responsive.
    // On the fit-request frame the transform (and thus the clip) is degenerate —
    // draw everything rather than cull against garbage.
    let clip = ui.clip_rect();
    let visible = if clip.is_finite() {
        clip
    } else {
        egui::Rect::EVERYTHING
    };
    let zoom = ui
        .ctx()
        .layer_transform_to_global(ui.layer_id())
        .map_or(1.0, |t| t.scaling);
    // Below this zoom the column text is unreadable anyway; draw title-only boxes
    // and skip the per-row galleys, by far the most expensive part of a big canvas.
    let detailed = zoom > 0.4;

    // Light themes need the canvas furniture pulled the other way: Daylight's border
    // and surface tones sit within a few steps of white, so dots and boxes painted
    // with them disappear. Pulling toward the text colour contrasts in both modes.
    let is_dark = crate::theme::current().is_dark;
    let node_fill = if is_dark {
        palette::SURFACE()
    } else {
        // White cards on the grey panel canvas, the way light-mode Figma reads.
        palette::BASE()
    };
    let edge_color = if is_dark {
        palette::BORDER_STRONG()
    } else {
        palette::TEXT_FAINT().gamma_multiply(0.7)
    };

    // Dot grid under everything, in scene coordinates so it pans and zooms with the
    // diagram. The spacing doubles until dots stay ≥ ~22 screen px apart, so zooming
    // out coarsens the grid instead of flooding the canvas.
    if clip.is_finite() && zoom > 0.0 {
        let mut spacing = 28.0_f32;
        while spacing * zoom < 22.0 {
            spacing *= 2.0;
        }
        let dot = palette::TEXT_FAINT().gamma_multiply(0.35);
        let mut x = (visible.left() / spacing).floor() * spacing;
        while x <= visible.right() {
            let mut y = (visible.top() / spacing).floor() * spacing;
            while y <= visible.bottom() {
                painter.circle_filled(egui::pos2(x, y), 1.1, dot);
                y += spacing;
            }
            x += spacing;
        }
    }

    // Measure boxes once with real font metrics (the layout used char-count estimates).
    for node in &mut erd.nodes {
        if node.size != egui::Vec2::ZERO {
            continue;
        }
        let mut width: f32 = painter
            .layout_no_wrap(node.title.clone(), title_font.clone(), palette::TEXT())
            .size()
            .x
            + 24.0;
        for col in &node.columns {
            let name = painter.layout_no_wrap(col.name.clone(), body_font.clone(), palette::TEXT());
            let ty =
                painter.layout_no_wrap(col.data_type.clone(), small_font.clone(), palette::TEXT());
            // marker + name + gap + type + padding
            width = width.max(16.0 + name.size().x + 24.0 + ty.size().x + 12.0);
        }
        node.size = egui::vec2(
            width.clamp(170.0, 380.0),
            HEADER_H + node.columns.len() as f32 * ROW_H + 6.0,
        );
    }

    // Edges first, so the boxes draw over them.
    for edge in &erd.edges {
        let highlighted = erd.selected.is_some_and(|s| s == edge.from || s == edge.to);
        let color = if highlighted {
            palette::ACCENT()
        } else {
            edge_color
        };
        let stroke = egui::Stroke::new(if highlighted { 2.0_f32 } else { 1.4_f32 }, color);

        let from_rect = erd.nodes[edge.from].rect();
        let to_rect = erd.nodes[edge.to].rect();
        // The curve stays within the two boxes' hull plus its control-point reach.
        if !visible.intersects(from_rect.union(to_rect).expand(150.0)) {
            continue;
        }
        let from_y = from_rect.top() + HEADER_H + (edge.from_row as f32 + 0.5) * ROW_H;
        let to_y = match edge.to_row {
            Some(row) => to_rect.top() + HEADER_H + (row as f32 + 0.5) * ROW_H,
            None => to_rect.top() + HEADER_H * 0.5,
        };

        if edge.from == edge.to {
            // Self-reference: a small loop out of the right side.
            let r = from_rect.right();
            let p0 = egui::pos2(r, from_y);
            let p1 = egui::pos2(
                r,
                to_y + if edge.to_row == Some(edge.from_row) {
                    ROW_H * 0.6
                } else {
                    0.0
                },
            );
            let reach = 46.0;
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [
                    p0,
                    p0 + egui::vec2(reach, 0.0),
                    p1 + egui::vec2(reach, 0.0),
                    p1,
                ],
                false,
                egui::Color32::TRANSPARENT,
                stroke,
            ));
            let out = egui::vec2(1.0, 0.0); // both ends leave through the right edge
            erd_child_mark(&painter, p0, out, edge.many, stroke);
            erd_parent_mark(&painter, p1, out, edge.optional, stroke);
            continue;
        }

        // Exit/enter on the sides that face each other.
        let from_right = to_rect.center().x >= from_rect.center().x;
        let p0 = egui::pos2(
            if from_right {
                from_rect.right()
            } else {
                from_rect.left()
            },
            from_y,
        );
        let p1 = egui::pos2(
            if from_right {
                to_rect.left()
            } else {
                to_rect.right()
            },
            to_y,
        );
        let reach = ((p1.x - p0.x).abs() * 0.5).clamp(32.0, 140.0);
        let out0 = egui::vec2(if from_right { 1.0 } else { -1.0 }, 0.0);
        let c0 = p0 + out0 * reach;
        let c1 = p1 - out0 * reach;
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [p0, c0, c1, p1],
            false,
            egui::Color32::TRANSPARENT,
            stroke,
        ));
        erd_child_mark(&painter, p0, out0, edge.many, stroke);
        erd_parent_mark(&painter, p1, -out0, edge.optional, stroke);
    }

    // Nodes: drag to move, click to highlight a table's relations.
    let mut clicked: Option<usize> = None;
    for (i, node) in erd.nodes.iter_mut().enumerate() {
        let id = ui.id().with(("erd_node", i));
        let rect = node.rect();
        if !visible.intersects(rect) {
            continue; // fully off-screen: nothing to draw or interact with
        }
        let resp = ui.interact(rect, id, egui::Sense::click_and_drag());
        if resp.dragged() {
            node.pos += resp.drag_delta();
        }
        if resp.clicked() {
            clicked = Some(i);
        }
        let rect = node.rect(); // after the drag delta
        let selected = erd.selected == Some(i);

        let border = if selected {
            egui::Stroke::new(1.6_f32, palette::ACCENT())
        } else if resp.hovered() {
            egui::Stroke::new(1.4_f32, palette::BORDER_STRONG())
        } else {
            // BORDER is invisible against a light canvas; the strong tone works in both.
            egui::Stroke::new(1.0_f32, palette::BORDER_STRONG())
        };
        painter.rect(rect, 6.0, node_fill, border, egui::StrokeKind::Inside);
        // Header band + title.
        let header_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), HEADER_H));
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.top() + HEADER_H),
                egui::pos2(rect.right(), rect.top() + HEADER_H),
            ],
            egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()),
        );
        let title = painter.layout_no_wrap(node.title.clone(), title_font.clone(), palette::TEXT());
        painter.galley(
            egui::pos2(
                header_rect.left() + 10.0,
                header_rect.center().y - title.size().y / 2.0,
            ),
            title,
            palette::TEXT(),
        );

        // Column rows: a marker (PK dot / FK ring), the name, and the type right-aligned.
        // Zoomed far out they'd be sub-pixel noise, so the box + title carry the shape.
        let columns: &[crate::erd::ErdColumn] = if detailed { &node.columns } else { &[] };
        for (r, col) in columns.iter().enumerate() {
            let y = rect.top() + HEADER_H + (r as f32 + 0.5) * ROW_H;
            let marker = egui::pos2(rect.left() + 11.0, y);
            if col.primary_key {
                painter.circle_filled(marker, 2.8, palette::ACCENT());
            } else if col.foreign_key {
                painter.circle_stroke(marker, 2.8, egui::Stroke::new(1.2_f32, palette::ACCENT()));
            }
            let name_color = if col.primary_key {
                palette::TEXT()
            } else {
                palette::TEXT_WEAK()
            };
            let name = painter.layout_no_wrap(col.name.clone(), body_font.clone(), name_color);
            painter.galley(
                egui::pos2(rect.left() + 20.0, y - name.size().y / 2.0),
                name,
                name_color,
            );
            let ty = painter.layout_no_wrap(
                col.data_type.clone(),
                small_font.clone(),
                palette::TEXT_FAINT(),
            );
            painter.galley(
                egui::pos2(rect.right() - 8.0 - ty.size().x, y - ty.size().y / 2.0),
                ty,
                palette::TEXT_FAINT(),
            );
        }

        // The FK summary for this table, on hover.
        if resp.hovered() && !erd.edges.is_empty() {
            let details: Vec<&str> = erd
                .edges
                .iter()
                .filter(|e| e.from == i)
                .map(|e| e.detail.as_str())
                .collect();
            if !details.is_empty() {
                resp.on_hover_text(details.join("\n"));
            }
        }
    }
    if let Some(i) = clicked {
        erd.selected = if erd.selected == Some(i) {
            None
        } else {
            Some(i)
        };
    }
}

/// Crow's-foot mark at the referencing (FK) end of an edge. `p` sits on the box border
/// and `out` is the unit direction the edge leaves the box in: a three-prong foot fanning
/// into the border for "many", a single perpendicular bar for "one" (unique FK).
fn erd_child_mark(
    painter: &egui::Painter,
    p: egui::Pos2,
    out: egui::Vec2,
    many: bool,
    stroke: egui::Stroke,
) {
    let n = egui::vec2(-out.y, out.x);
    if many {
        let q = p + out * 10.0; // the point on the line the prongs fan out from
        for k in [-1.0, 0.0, 1.0] {
            painter.line_segment([q, p + n * (4.5 * k)], stroke);
        }
    } else {
        let q = p + out * 7.0;
        painter.line_segment([q + n * 4.5, q - n * 4.5], stroke);
    }
}

/// Cardinality mark at the referenced (parent) end: a double bar for "exactly one", or a
/// hollow circle plus bar for "zero or one" (nullable FK). `out` points away from the box.
fn erd_parent_mark(
    painter: &egui::Painter,
    p: egui::Pos2,
    out: egui::Vec2,
    optional: bool,
    stroke: egui::Stroke,
) {
    let n = egui::vec2(-out.y, out.x);
    let bar = |at: f32| {
        let q = p + out * at;
        painter.line_segment([q + n * 4.5, q - n * 4.5], stroke);
    };
    if optional {
        bar(6.0);
        painter.circle_stroke(p + out * 13.5, 3.2, stroke);
    } else {
        bar(6.0);
        bar(10.0);
    }
}

impl DbGuiApp {
    /// The ER diagram view: a pan/zoom canvas (`egui::Scene`) of draggable table boxes
    /// connected by foreign-key curves. Takes over the central panel while open.
    pub(in crate::app) fn erd_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        let target = self
            .active()
            .map(|active| format!("{} ({})", active.name, active.db.kind().label()))
            .unwrap_or_else(|| "disconnected".to_string());
        let Some(erd) = self.tabs.get_mut(idx).and_then(|t| t.diagram.as_mut()) else {
            // A Diagram tab without its snapshot (shouldn't happen at runtime).
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.colored_label(
                    palette::TEXT_FAINT(),
                    "This diagram is no longer available.",
                );
            });
            return;
        };

        // Header: `⌗ orders   sample.sqlite · 6 tables · 6 relations` on the left,
        // the depth control and the canvas actions on the right. The tab strip
        // already provides close, so there is no ×.
        // 2 (panel margin) + 2 here = the 4px item-spacing gap below, so the band
        // sits centered between the tab strip and the separator.
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            icons::show_native(ui, icons::diagram(), icons::SIZE);
            ui.add_space(2.0);
            let (title, context) = match &erd.focus {
                Some(f) => (
                    f.table.clone(),
                    format!(
                        "{} · {} tables · {} relations",
                        erd.database,
                        erd.nodes.len(),
                        erd.edges.len()
                    ),
                ),
                None => (
                    erd.database.clone(),
                    format!("{} tables · {} relations", erd.nodes.len(), erd.edges.len()),
                ),
            };
            ui.label(egui::RichText::new(title).strong().color(palette::TEXT()));
            ui.add_space(6.0);
            ui.colored_label(palette::TEXT_FAINT(), context);
            if erd.focus.is_none() {
                ui.add_space(6.0);
                ui.colored_label(palette::TEXT_FAINT(), format!("Target: {target}"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if erd.tracks_schema
                    && components::pill_icon_button(
                        ui,
                        icons::refresh(),
                        "Rebuild from the current schema",
                    )
                    .clicked()
                {
                    actions.push(Action::RefreshErd);
                }
                if components::pill_icon_button(
                    ui,
                    icons::relayout(),
                    "Recompute the automatic arrangement",
                )
                .clicked()
                {
                    erd.layout();
                }
                if components::pill_icon_button(ui, icons::fit(), "Zoom to fit all tables")
                    .clicked()
                {
                    erd.request_fit();
                }
                if erd.focus.is_none() {
                    ui.add_space(10.0);
                    if components::primary_button(
                        ui,
                        icons::play(),
                        "Forward Engineer",
                        !erd.design.tables.is_empty(),
                    )
                    .on_hover_text("Apply this design as DDL on the connected database")
                    .clicked()
                    {
                        actions.push(Action::ForwardEngineerErd);
                    }
                    if components::pill_icon_button(
                        ui,
                        icons::save(),
                        "Export a connection-independent .plusplus-er.json file",
                    )
                    .clicked()
                    {
                        actions.push(Action::ExportErd);
                    }
                    if components::pill_icon_button(ui, icons::plus(), "Add table to this design")
                        .clicked()
                    {
                        actions.push(Action::AddErdTable);
                    }
                    if let Some(selected) = erd.selected {
                        if components::pill_icon_button(ui, icons::edit(), "Edit selected table")
                            .clicked()
                        {
                            actions.push(Action::EditErdTable(selected));
                        }
                        if components::pill_icon_button(ui, icons::trash(), "Remove selected table")
                            .clicked()
                        {
                            actions.push(Action::DeleteErdTable(selected));
                        }
                    }
                }
                if let Some(focus) = &erd.focus {
                    ui.add_space(10.0);
                    let depths = [1, 2, crate::erd::DEPTH_ALL];
                    let selected = depths
                        .iter()
                        .position(|d| *d == focus.depth)
                        .unwrap_or(depths.len() - 1);
                    let choice = components::segmented_sized(
                        ui,
                        &[
                            (icons::diagram(), "1"),
                            (icons::diagram(), "2"),
                            (icons::diagram(), "All"),
                        ],
                        selected,
                        132.0,
                        false,
                    );
                    if choice != selected {
                        actions.push(Action::SetErdDepth(depths[choice]));
                    }
                }
            });
        });
        ui.separator();

        if erd.nodes.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.colored_label(palette::TEXT_FAINT(), "This database has no tables.");
            });
            return;
        }

        let mut scene_rect = erd.scene_rect;
        egui::Scene::new()
            .zoom_range(0.1..=2.5)
            .show(ui, &mut scene_rect, |ui| {
                erd_canvas(ui, erd);
            });
        erd.scene_rect = scene_rect;
    }
}
