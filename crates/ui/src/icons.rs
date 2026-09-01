//! Hugeicons free Stroke Rounded icons (https://hugeicons.com/icons/stroke-rounded), embedded
//! as white-stroked SVG so the loaded texture can be `.tint()`-ed to the current theme colour.
//! The database-vendor logos in `assets/icondb/` are brand marks rather than interface icons,
//! so they keep their own shapes and colours.

use dbcore::{ConnectionIcon, DbKind};
use egui::{include_image, ImageSource};

/// Default on-canvas size for an icon, in points.
pub const SIZE: f32 = 16.0;

/// Size for database-kind logos in pickers and labels.
pub const DB_KIND_ICON_SIZE: f32 = 18.0;

macro_rules! icon_fns {
    ($($name:ident => $path:literal),* $(,)?) => {
        $(
            #[inline]
            #[allow(dead_code)]
            pub fn $name() -> ImageSource<'static> {
                include_image!($path)
            }
        )*
    };
}

icon_fns! {
    play       => "../assets/icons/play.svg",
    connect    => "../assets/icons/connect.svg",
    disconnect => "../assets/icons/disconnect.svg",
    plus       => "../assets/icons/plus.svg",
    minus      => "../assets/icons/minus.svg",
    edit       => "../assets/icons/edit.svg",
    trash      => "../assets/icons/trash.svg",
    database   => "../assets/icons/database.svg",
    table      => "../assets/icons/table.svg",
    view       => "../assets/icons/view.svg",
    conn_cloud => "../assets/icons/cloud.svg",
    conn_storage => "../assets/icons/disk.svg",
    conn_star  => "../assets/icons/star-emphasis.svg",
    conn_treasure => "../assets/icons/box.svg",
    code       => "../assets/icons/code.svg",
    column     => "../assets/icons/column.svg",
    autocomplete_keyword => "../assets/icons/autocomplete-keyword.svg",
    autocomplete_table => "../assets/icons/autocomplete-table.svg",
    autocomplete_column => "../assets/icons/autocomplete-column.svg",
    diagram    => "../assets/icons/diagram.svg",
    chart_line => "../assets/icons/chart-line.svg",
    chart_bar => "../assets/icons/chart-bar.svg",
    chart_scatter => "../assets/icons/chart-scatter.svg",
    key        => "../assets/icons/key.svg",
    keyboard_command => "../assets/icons/keyboard-command.svg",
    keyboard_control => "../assets/icons/keyboard-control.svg",
    keyboard_return => "../assets/icons/keyboard-return.svg",
    keyboard_shift => "../assets/icons/keyboard-shift.svg",
    index      => "../assets/icons/index.svg",
    filter     => "../assets/icons/filter.svg",
    folder     => "../assets/icons/folder.svg",
    file       => "../assets/icons/file.svg",
    fit        => "../assets/icons/fit.svg",
    history    => "../assets/icons/history.svg",
    relayout   => "../assets/icons/relayout.svg",
    refresh    => "../assets/icons/refresh.svg",
    more_vert  => "../assets/icons/more-vert.svg",
    search     => "../assets/icons/search.svg",
    warning    => "../assets/icons/warning.svg",
    close      => "../assets/icons/close.svg",
    save       => "../assets/icons/save.svg",
    undo       => "../assets/icons/undo.svg",
    redo       => "../assets/icons/redo.svg",
    sort_ascending => "../assets/icons/sort-ascending.svg",
    sort_descending => "../assets/icons/sort-descending.svg",
    star       => "../assets/icons/star.svg",
    star_filled => "../assets/icons/star-filled.svg",
    settings   => "../assets/icons/settings.svg",
    pager      => "../assets/icons/pager.svg",
    arrow_up   => "../assets/icons/arrow-up.svg",
    arrow_down => "../assets/icons/arrow-down.svg",
    arrow_left => "../assets/icons/arrow-left.svg",
    chevron_down => "../assets/icons/chevron-down.svg",
    arrow_right => "../assets/icons/arrow-right.svg",
    arrow_up_right => "../assets/icons/arrow-up-right.svg",
    layout_connections => "../assets/icons/layout-connections.svg",
    layout_schema => "../assets/icons/layout-schema.svg",
    layout_details => "../assets/icons/layout-details.svg",
    layout_query => "../assets/icons/layout-query.svg",
    split_editor => "../assets/icons/split-editor.svg",
    layout_log => "../assets/icons/layout-log.svg",
    db_postgres_dark => "../assets/icondb/skill-icons--postgresql-dark.svg",
    db_postgres_light => "../assets/icondb/skill-icons--postgresql-light.svg",
    db_mysql_dark => "../assets/icondb/skill-icons--mysql-dark.svg",
    db_mysql_light => "../assets/icondb/skill-icons--mysql-light.svg",
    db_mariadb => "../assets/icondb/simple-icons--mariadb.svg",
    db_sqlserver => "../assets/icondb/devicon-plain--microsoftsqlserver-wordmark.svg",
    db_sqlite => "../assets/icondb/skill-icons--sqlite.svg",
    db_cassandra => "../assets/icondb/simple-icons--apachecassandra.svg",
    db_scylladb => "../assets/icondb/simple-icons--scylladb.svg",
}

/// Embedded logo for a database backend; picks light/dark Postgres/MySQL variants from the theme.
pub fn db_kind_icon(kind: DbKind) -> ImageSource<'static> {
    let dark = crate::theme::current().is_dark;
    match kind {
        DbKind::Postgres => {
            if dark {
                db_postgres_dark()
            } else {
                db_postgres_light()
            }
        }
        DbKind::MySql => {
            if dark {
                db_mysql_dark()
            } else {
                db_mysql_light()
            }
        }
        DbKind::MariaDb => db_mariadb(),
        DbKind::SqlServer => db_sqlserver(),
        DbKind::Sqlite => db_sqlite(),
        DbKind::DuckDb => database(),
        DbKind::Cassandra => db_cassandra(),
        DbKind::ScyllaDb => db_scylladb(),
    }
}

/// Provider assets carry their final colours, so a white tint preserves them. The two
/// `simple-icons` monochrome marks (Cassandra/ScyllaDB) are `currentColor` glyphs instead,
/// so they take the theme's text colour like any other single-colour icon.
pub fn db_kind_icon_tint(kind: DbKind) -> egui::Color32 {
    match kind {
        DbKind::DuckDb | DbKind::Cassandra | DbKind::ScyllaDb => crate::style::palette::TEXT(),
        _ => egui::Color32::WHITE,
    }
}

/// Build a themed image widget for an icon at the given size.
fn image(
    ui: &egui::Ui,
    src: ImageSource<'static>,
    size: f32,
    tint: egui::Color32,
) -> egui::Image<'static> {
    let _ = ui;
    egui::Image::new(src)
        .fit_to_exact_size(egui::vec2(size, size))
        .tint(tint)
}

/// Map a persisted connection icon to its embedded SVG.
pub fn connection_icon(icon: ConnectionIcon) -> ImageSource<'static> {
    match icon {
        ConnectionIcon::Database => database(),
        ConnectionIcon::Table => table(),
        ConnectionIcon::Cloud => conn_cloud(),
        ConnectionIcon::Storage => conn_storage(),
        ConnectionIcon::Star => conn_star(),
        ConnectionIcon::Treasure => conn_treasure(),
    }
}

/// Paint a connection sidebar icon at `rect`, tinted to `tint` (Hugeicons glyphs are
/// single-colour and adopt the theme like every other icon).
pub fn paint_connection_icon(
    ui: &egui::Ui,
    icon: ConnectionIcon,
    rect: egui::Rect,
    tint: egui::Color32,
) {
    egui::Image::new(connection_icon(icon))
        .fit_to_exact_size(rect.size())
        .tint(tint)
        .paint_at(ui, rect);
}

/// Compact picker tile for the connection dialog.
pub fn connection_icon_picker_button(
    ui: &mut egui::Ui,
    icon: ConnectionIcon,
    selected: bool,
    size: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.5, crate::style::palette::ACCENT()),
                egui::StrokeKind::Outside,
            );
        } else if resp.hovered() {
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.0, crate::style::palette::BORDER()),
                egui::StrokeKind::Outside,
            );
        }
        let icon_rect = rect.shrink(5.0);
        let tint = if selected {
            crate::style::palette::TEXT()
        } else {
            crate::style::palette::TEXT_WEAK()
        };
        paint_connection_icon(ui, icon, icon_rect, tint);
    }
    resp.on_hover_text(icon.label())
}

/// Render an inline icon at the theme's primary text colour — the default weight for
/// schema-tree and toolbar glyphs (database, table, diagram, …).
pub fn show_native(ui: &mut egui::Ui, src: ImageSource<'static>, size: f32) -> egui::Response {
    let tint = crate::style::palette::TEXT();
    ui.add(image(ui, src, size, tint))
}

/// Render a dimmed/weak inline icon (matches `ui.weak`).
pub fn show_weak(ui: &mut egui::Ui, src: ImageSource<'static>, size: f32) -> egui::Response {
    let tint = crate::style::palette::TEXT_FAINT();
    ui.add(image(ui, src, size, tint))
}

/// Render an inline icon tinted to an explicit colour (for semantic glyphs like the
/// error/warning triangle).
pub fn show_colored(
    ui: &mut egui::Ui,
    src: ImageSource<'static>,
    size: f32,
    color: egui::Color32,
) -> egui::Response {
    ui.add(image(ui, src, size, color))
}
