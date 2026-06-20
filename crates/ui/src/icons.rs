//! Iconoir icons (https://iconoir.com), downloaded as SVG and embedded into the binary.
//! They are rendered via `egui_extras`' SVG image loader and tinted to the current theme
//! text colour, so they stay crisp at any size and adapt to light/dark themes.

use dbcore::{ConnectionIcon, DbKind};
use egui::{include_image, ImageSource};

/// Default on-canvas size for an icon, in points.
pub const SIZE: f32 = 16.0;

/// Size for database-kind logos in pickers and labels.
pub const DB_KIND_ICON_SIZE: f32 = 16.0;

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
    edit       => "../assets/icons/edit.svg",
    trash      => "../assets/icons/trash.svg",
    database   => "../assets/icons/streamline-plump-color--database.svg",
    table      => "../assets/icons/streamline-plump-color--table-flat.svg",
    conn_cloud => "../assets/icons/streamline-plump-color--cloud-data-transfer-flat.svg",
    conn_storage => "../assets/icons/streamline-plump-color--hard-drive-2-flat.svg",
    conn_star  => "../assets/icons/streamline-plump-color--star-circle-flat.svg",
    conn_treasure => "../assets/icons/streamline-plump-color--treasure-chest-flat.svg",
    code       => "../assets/icons/code.svg",
    column     => "../assets/icons/column.svg",
    diagram    => "../assets/icons/diagram.svg",
    key        => "../assets/icons/key.svg",
    index      => "../assets/icons/index.svg",
    filter     => "../assets/icons/filter.svg",
    more_vert  => "../assets/icons/more-vert.svg",
    search     => "../assets/icons/search.svg",
    warning    => "../assets/icons/warning.svg",
    close      => "../assets/icons/close.svg",
    save       => "../assets/icons/save.svg",
    settings   => "../assets/icons/settings.svg",
    empty_results => "../assets/empty-results.svg",
    db_postgres_dark => "../assets/icondb/skill-icons--postgresql-dark.svg",
    db_postgres_light => "../assets/icondb/skill-icons--postgresql-light.svg",
    db_mysql_dark => "../assets/icondb/skill-icons--mysql-dark.svg",
    db_mysql_light => "../assets/icondb/skill-icons--mysql-light.svg",
    db_mariadb => "../assets/icondb/devicon--mariadb.svg",
    db_sqlserver => "../assets/icondb/devicon-plain--microsoftsqlserver-wordmark.svg",
    db_sqlite => "../assets/icondb/skill-icons--sqlite.svg",
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
    }
}

fn db_kind_button_image(kind: DbKind) -> egui::Image<'static> {
    egui::Image::new(db_kind_icon(kind)).fit_to_exact_size(egui::vec2(
        DB_KIND_ICON_SIZE,
        DB_KIND_ICON_SIZE,
    ))
}

/// Menu row: one selectable button with logo and label sharing hover/selection.
pub fn db_kind_selectable(
    ui: &mut egui::Ui,
    current: &mut DbKind,
    kind: DbKind,
) -> egui::Response {
    let selected = *current == kind;
    let btn = egui::Button::image_and_text(db_kind_button_image(kind), kind.label())
        .selected(selected)
        .frame_when_inactive(selected)
        .frame(true)
        .min_size(egui::vec2(ui.available_width(), 0.0));

    let mut response = ui.add(btn);
    if response.clicked() && !selected {
        *current = kind;
        response.mark_changed();
    }
    response
}

/// Combo-style picker: one button (icon + label + arrow) opening a logo-labelled menu.
pub fn db_kind_combo(
    ui: &mut egui::Ui,
    current: &mut DbKind,
    id: impl std::hash::Hash,
    width: f32,
) -> egui::Response {
    let btn = egui::Button::image_and_text(db_kind_button_image(*current), current.label())
        .right_text(egui::RichText::new("▾").size(10.0))
        .min_size(egui::vec2(width, 0.0));
    let button_response = ui.add(btn);

    egui::Popup::menu(&button_response)
        .id(egui::Id::new(id).with("popup"))
        .width(button_response.rect.width())
        .show(|ui| {
            ui.set_min_width(ui.available_width());
            for kind in [
                DbKind::Postgres,
                DbKind::MySql,
                DbKind::MariaDb,
                DbKind::SqlServer,
                DbKind::Sqlite,
            ] {
                db_kind_selectable(ui, current, kind);
            }
        });

    button_response
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

/// Connection picker icons are full-colour Streamline assets — never theme-tinted.
pub fn connection_icon_is_colored(_icon: ConnectionIcon) -> bool {
    true
}

/// Paint a connection sidebar icon at `rect`.
pub fn paint_connection_icon(
    ui: &egui::Ui,
    icon: ConnectionIcon,
    rect: egui::Rect,
    tint: egui::Color32,
) {
    let img = egui::Image::new(connection_icon(icon)).fit_to_exact_size(rect.size());
    if connection_icon_is_colored(icon) {
        img.paint_at(ui, rect);
    } else {
        img.tint(tint).paint_at(ui, rect);
    }
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

/// Full-colour Streamline assets (database, table, …) — keep the SVG's own palette.
pub fn show_native(ui: &mut egui::Ui, src: ImageSource<'static>, size: f32) -> egui::Response {
    ui.add(
        egui::Image::new(src).fit_to_exact_size(egui::vec2(size, size)),
    )
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

/// A text button with a leading icon.
pub fn button(
    ui: &mut egui::Ui,
    src: ImageSource<'static>,
    text: &str,
    enabled: bool,
) -> egui::Response {
    let tint = ui.visuals().widgets.inactive.fg_stroke.color;
    let img = image(ui, src, SIZE, tint);
    ui.add_enabled(enabled, egui::Button::image_and_text(img, text))
}

/// A filled, accent-coloured primary button — for the one main action in view (Run).
pub fn primary_button(
    ui: &mut egui::Ui,
    src: ImageSource<'static>,
    text: &str,
    enabled: bool,
) -> egui::Response {
    use crate::style::palette;
    let img = image(ui, src, SIZE, palette::ON_ACCENT());
    let btn = egui::Button::image_and_text(
        img,
        egui::RichText::new(text)
            .color(palette::ON_ACCENT())
            .strong(),
    )
    .fill(palette::ACCENT())
    .stroke(egui::Stroke::new(1.0, palette::ACCENT_HOVER()));
    ui.add_enabled(enabled, btn)
}

/// A text button with a leading icon that reads as "on" (accent-tinted with a soft fill)
/// when `active`. Used for toggles like the filter-bar switch.
#[allow(dead_code)]
pub fn toggle_button(
    ui: &mut egui::Ui,
    src: ImageSource<'static>,
    text: &str,
    enabled: bool,
    active: bool,
) -> egui::Response {
    use crate::style::palette;
    let tint = if active {
        palette::ACCENT()
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    let img = image(ui, src, SIZE, tint);
    let label = if active {
        egui::RichText::new(text).color(palette::ACCENT()).strong()
    } else {
        egui::RichText::new(text)
    };
    let mut btn = egui::Button::image_and_text(img, label);
    if active {
        btn = btn
            .fill(palette::SELECTION())
            .stroke(egui::Stroke::new(1.0, palette::ACCENT()));
    }
    ui.add_enabled(enabled, btn)
}

/// A compact icon-only button with a hover tooltip.
#[allow(dead_code)]
pub fn icon_button(ui: &mut egui::Ui, src: ImageSource<'static>, hover: &str) -> egui::Response {
    let tint = ui.visuals().widgets.inactive.fg_stroke.color;
    let img = image(ui, src, SIZE, tint);
    ui.add(egui::Button::image(img)).on_hover_text(hover)
}
