//! Tabler Icons, embedded as white SVGs for tinting with the active theme.
//! Shared by semantic meaning across the sidebar, tabs, menus, and autocomplete.
//! Outline icons retain Tabler's 24-unit canvas and 2-unit rounded strokes.
//! Sources and upstream names are recorded in assets/icons/tabler.json.
//! Provider marks in assets/icondb/ are transparent monochrome shapes tinted by the theme.

use dbcore::DbKind;
use egui::{include_image, ImageSource};

/// Default on-canvas size for an icon, in points.
pub const SIZE: f32 = 16.0;

macro_rules! icon_fns {
    ($($name:ident => $path:literal),* $(,)?) => {
        $(
            #[inline]
            pub fn $name() -> ImageSource<'static> {
                include_image!($path)
            }
        )*

        #[cfg(test)]
        fn gallery_icons() -> Vec<(&'static str, ImageSource<'static>)> {
            vec![$((stringify!($name), $name())),*]
        }
    };
}

icon_fns! {
    play       => "../../assets/icons/outline/player-play.svg",
    connect    => "../../assets/icons/outline/plug-connected.svg",
    plug_off   => "../../assets/icons/outline/plug-off.svg",
    mood_sad_dizzy => "../../assets/icons/outline/mood-sad-dizzy.svg",
    disconnect => "../../assets/icons/outline/plug-connected-x.svg",
    plus       => "../../assets/icons/outline/plus.svg",
    minus      => "../../assets/icons/outline/minus.svg",
    edit       => "../../assets/icons/outline/pencil.svg",
    trash      => "../../assets/icons/outline/trash.svg",
    database   => "../../assets/icons/outline/database.svg",
    database_export => "../../assets/icons/outline/database-export.svg",
    database_import => "../../assets/icons/outline/database-import.svg",
    check      => "../../assets/icons/outline/circle-check.svg",
    table      => "../../assets/icons/outline/table.svg",
    view       => "../../assets/icons/outline/eye.svg",
    function   => "../../assets/icons/outline/math-function.svg",
    copy       => "../../assets/icons/outline/copy.svg",
    code       => "../../assets/icons/outline/terminal-2.svg",
    column     => "../../assets/icons/outline/columns-3.svg",
    diagram    => "../../assets/icons/outline/sitemap.svg",
    key        => "../../assets/icons/outline/key.svg",
    keyboard_command => "../../assets/icons/outline/command.svg",
    index      => "../../assets/icons/outline/list-details.svg",
    filter     => "../../assets/icons/outline/filter.svg",
    folder     => "../../assets/icons/outline/folder.svg",
    file       => "../../assets/icons/outline/file.svg",
    fit        => "../../assets/icons/outline/arrows-maximize.svg",
    history    => "../../assets/icons/outline/history.svg",
    relayout   => "../../assets/icons/outline/layout-grid.svg",
    refresh    => "../../assets/icons/outline/refresh.svg",
    more_vert  => "../../assets/icons/outline/dots-vertical.svg",
    search     => "../../assets/icons/outline/search.svg",
    warning    => "../../assets/icons/outline/alert-triangle.svg",
    close      => "../../assets/icons/outline/x.svg",
    save       => "../../assets/icons/outline/device-floppy.svg",
    undo       => "../../assets/icons/outline/arrow-back-up.svg",
    redo       => "../../assets/icons/outline/arrow-forward-up.svg",
    result_sort_ascending => "../../assets/icons/outline/result-sort-ascending.svg",
    result_sort_descending => "../../assets/icons/outline/result-sort-descending.svg",
    result_sort_unsorted => "../../assets/icons/outline/result-sort-unsorted.svg",
    star       => "../../assets/icons/outline/star.svg",
    star_filled => "../../assets/icons/filled/star.svg",
    settings   => "../../assets/icons/outline/settings.svg",
    pager      => "../../assets/icons/outline/adjustments-horizontal.svg",
    chevron_left => "../../assets/icons/outline/chevron-left.svg",
    chevron_down => "../../assets/icons/outline/chevron-down.svg",
    chevron_right => "../../assets/icons/outline/chevron-right.svg",
    chevron_up => "../../assets/icons/outline/chevron-up.svg",
    replace    => "../../assets/icons/outline/replace.svg",
    replace_all => "../../assets/icons/outline/arrows-exchange.svg",
    arrow_up_right => "../../assets/icons/outline/arrow-up-right.svg",
    layout_connections => "../../assets/icons/outline/layout-sidebar.svg",
    layout_schema => "../../assets/icons/outline/layout-columns.svg",
    layout_details => "../../assets/icons/outline/layout-sidebar-right.svg",
    layout_query => "../../assets/icons/outline/layout-navbar.svg",
    layout_log => "../../assets/icons/outline/layout-bottombar.svg",
    db_postgres => "../../assets/icondb/postgres.svg",
    db_mysql => "../../assets/icondb/mysql.svg",
    db_mariadb => "../../assets/icondb/mariadb.svg",
    db_sqlserver => "../../assets/icondb/sqlserver.svg",
    db_sqlite => "../../assets/icondb/sqlite.svg",
    db_cassandra => "../../assets/icondb/cassandra.svg",
    db_scylladb => "../../assets/icondb/scylladb.svg",
}

/// One transparent monochrome provider mark, shared by both themes.
pub fn db_kind_icon(kind: DbKind) -> ImageSource<'static> {
    match kind {
        DbKind::Postgres => db_postgres(),
        DbKind::MySql => db_mysql(),
        DbKind::MariaDb => db_mariadb(),
        DbKind::SqlServer => db_sqlserver(),
        DbKind::Sqlite => db_sqlite(),
        DbKind::DuckDb => database(),
        DbKind::Cassandra => db_cassandra(),
        DbKind::ScyllaDb => db_scylladb(),
    }
}

/// White/light on dark themes and dark on light themes, like the surrounding text.
pub fn db_kind_icon_tint() -> egui::Color32 {
    crate::style::palette::TEXT()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Manual visual review of every registered glyph at its actual UI size.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_icon_gallery() {
        let mut results = egui_kittest::SnapshotResults::new();
        for theme_key in ["midnight-conversational", "daylight"] {
            let theme = crate::theme::ThemeRegistry::load().theme_of(theme_key);
            let mut setup = false;
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(960.0, 560.0))
                .with_pixels_per_point(2.0)
                .build_ui(move |ui| {
                    if !setup {
                        egui_extras::install_image_loaders(ui.ctx());
                        crate::theme::set_current(theme);
                        crate::style::apply(ui.ctx());
                        setup = true;
                    }
                    egui::Grid::new("icon_gallery")
                        .num_columns(4)
                        .min_col_width(225.0)
                        .spacing(egui::vec2(12.0, 12.0))
                        .show(ui, |ui| {
                            for (index, (name, icon)) in gallery_icons().into_iter().enumerate() {
                                ui.horizontal(|ui| {
                                    show_native(ui, icon, SIZE);
                                    ui.label(name);
                                });
                                if index % 4 == 3 {
                                    ui.end_row();
                                }
                            }
                        });
                });
            harness.run_steps(8);
            harness.snapshot(format!("icons_{theme_key}"));
            results.extend_harness(&mut harness);
        }
        results.unwrap();
    }
}
