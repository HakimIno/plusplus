//! Runtime font selection and the small local library of user-imported OpenType files.

use crate::{AppFonts, HEADING_FAMILY};
use egui::{FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FontOption {
    pub key: String,
    pub label: String,
}

pub(crate) fn list_imported() -> Vec<FontOption> {
    let Ok(dir) = dbcore::config::fonts_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut options: Vec<_> = entries
        .flatten()
        .filter_map(|entry| option_from_path(&entry.path()))
        .collect();
    options.sort_by_key(|option| option.label.to_lowercase());
    options
}

/// The bytes behind a font choice: a validated imported file.
fn font_bytes(key: &str) -> Result<Vec<u8>, String> {
    read_valid_font(&imported_path(key)?)
}

fn option_from_path(path: &Path) -> Option<FontOption> {
    if !supported_extension(path) {
        return None;
    }
    let key = path.file_name()?.to_str()?.to_owned();
    let label = path.file_stem()?.to_str()?.replace(['_', '-'], " ");
    (!label.trim().is_empty()).then_some(FontOption { key, label })
}

fn supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc"
            )
        })
}

fn imported_path(key: &str) -> Result<PathBuf, String> {
    let file_name = Path::new(key)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Invalid font name".to_string())?;
    if file_name != key || !supported_extension(Path::new(key)) {
        return Err("Invalid font name".to_string());
    }
    dbcore::config::fonts_dir()
        .map(|dir| dir.join(file_name))
        .map_err(|error| error.to_string())
}

fn read_valid_font(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if metadata.len() > MAX_FONT_BYTES {
        return Err("Font files must be 32 MiB or smaller".to_string());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    // A .ttc collection (e.g. macOS's Menlo) is read at its first face, its regular weight.
    skrifa::FontRef::from_index(&bytes, 0)
        .map_err(|_| "The selected file is not a valid font".to_string())?;
    Ok(bytes)
}

/// Validate and copy a font into the app-owned library. Existing identical files are reused;
/// a numeric suffix prevents an import from silently replacing a different font.
pub(crate) fn import(path: &Path) -> Result<FontOption, String> {
    if !supported_extension(path) {
        return Err("Choose a .ttf, .otf or .ttc font file".to_string());
    }
    let bytes = read_valid_font(path)?;
    let dir = dbcore::config::fonts_dir().map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Could not create {}: {error}", dir.display()))?;

    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Imported font");
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("ttf");
    let mut destination = dir.join(format!("{stem}.{extension}"));
    for suffix in 2.. {
        if !destination.exists() {
            break;
        }
        if std::fs::read(&destination).ok().as_deref() == Some(bytes.as_slice()) {
            return option_from_path(&destination).ok_or_else(|| "Invalid font name".to_string());
        }
        destination = dir.join(format!("{stem}-{suffix}.{extension}"));
    }

    let temporary = destination.with_extension(format!("{extension}.tmp"));
    std::fs::write(&temporary, bytes).map_err(|error| format!("Could not copy font: {error}"))?;
    std::fs::rename(&temporary, &destination)
        .map_err(|error| format!("Could not finish importing font: {error}"))?;
    option_from_path(&destination).ok_or_else(|| "Invalid font name".to_string())
}

fn insert(fonts: &mut FontDefinitions, name: &str, bytes: &[u8]) {
    fonts.font_data.insert(
        name.to_owned(),
        Arc::new(FontData::from_owned(bytes.to_vec())),
    );
}

fn grid_mono_id() -> egui::Id {
    egui::Id::new("grid_all_monospace")
}

/// Whether the results grid and Details set every value in the code font. True once the user
/// has picked an "Editor & data font", so their choice reaches the data, not just numbers.
pub(crate) fn grid_all_monospace(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(grid_mono_id()).unwrap_or(false))
}

/// IBM Plex Sans weights: (family name, Latin face, Thai face). egui ignores variable-font
/// advance widths, so each weight is its own static file. Plex Sans Thai is drawn to pair
/// with Plex Sans, so Thai text keeps the same weight as the Latin text beside it. Regular
/// is `FontFamily::Proportional`; SemiBold also backs the heading family.
const WEIGHTS: &[(&str, &[u8], &[u8])] = &[
    (
        crate::FONT_THIN,
        include_bytes!("../../../app/assets/IBMPlexSans-Thin.ttf"),
        include_bytes!("../../../app/assets/IBMPlexSansThai-Thin.ttf"),
    ),
    (
        crate::FONT_REGULAR,
        include_bytes!("../../../app/assets/IBMPlexSans-Regular.ttf"),
        include_bytes!("../../../app/assets/IBMPlexSansThai-Regular.ttf"),
    ),
    (
        crate::FONT_MEDIUM,
        include_bytes!("../../../app/assets/IBMPlexSans-Medium.ttf"),
        include_bytes!("../../../app/assets/IBMPlexSansThai-Medium.ttf"),
    ),
    (
        crate::FONT_SEMIBOLD,
        include_bytes!("../../../app/assets/IBMPlexSans-SemiBold.ttf"),
        include_bytes!("../../../app/assets/IBMPlexSansThai-SemiBold.ttf"),
    ),
    (
        crate::FONT_BOLD,
        include_bytes!("../../../app/assets/IBMPlexSans-Bold.ttf"),
        include_bytes!("../../../app/assets/IBMPlexSansThai-Bold.ttf"),
    ),
];

fn sans_key(family: &str) -> String {
    format!("plex_{family}")
}

fn thai_key(family: &str) -> String {
    format!("plex_thai_{family}")
}

pub(crate) fn install(
    ctx: &egui::Context,
    app_fonts: AppFonts,
    ui_font: Option<&str>,
    code_font: Option<&str>,
) -> Result<(), String> {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        (
            "ibm_plex_mono",
            include_bytes!("../../../app/assets/IBMPlexMono-Regular.ttf") as &[u8],
        ),
        ("unifont", app_fonts.universal_regular),
    ] {
        insert(&mut fonts, name, bytes);
    }

    for (family, sans, thai) in WEIGHTS {
        for (key, bytes) in [(sans_key(family), sans), (thai_key(family), thai)] {
            fonts
                .font_data
                .insert(key, Arc::new(FontData::from_static(bytes)));
        }
    }

    let ui_custom = ui_font.map(font_bytes).transpose()?;
    let code_custom = code_font.map(font_bytes).transpose()?;
    if let Some(bytes) = &ui_custom {
        insert(&mut fonts, "custom_ui", bytes);
    }
    if let Some(bytes) = &code_custom {
        insert(&mut fonts, "custom_code", bytes);
    }

    // One chain per weight: Plex Sans, its Thai companion at the same weight, then Unifont.
    let chain = |family: &str| {
        let mut chain = Vec::new();
        if ui_custom.is_some() {
            chain.push("custom_ui".to_owned());
        }
        chain.extend([sans_key(family), thai_key(family), "unifont".to_owned()]);
        chain
    };
    fonts
        .families
        .insert(FontFamily::Proportional, chain(crate::FONT_REGULAR));
    fonts.families.insert(
        FontFamily::Name(HEADING_FAMILY.into()),
        chain(crate::FONT_SEMIBOLD),
    );
    for (family, _, _) in WEIGHTS {
        fonts
            .families
            .insert(FontFamily::Name((*family).into()), chain(family));
    }

    // Maps the code/data family to the bundled IBM Plex Mono face.
    // User-selected code fonts still take precedence.
    let mut monospace = Vec::new();
    if code_custom.is_some() {
        monospace.push("custom_code".to_owned());
    } else if ui_custom.is_some() {
        monospace.push("custom_ui".to_owned());
    }
    monospace.extend([
        "ibm_plex_mono".to_owned(),
        thai_key(crate::FONT_REGULAR),
        "unifont".to_owned(),
    ]);
    fonts.families.insert(FontFamily::Monospace, monospace);

    ctx.set_fonts(fonts);
    ctx.data_mut(|d| d.insert_temp(grid_mono_id(), code_font.is_some()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plex_weights_render_with_distinct_widths() {
        let ctx = egui::Context::default();
        let app_fonts = AppFonts {
            universal_regular: include_bytes!("../../../app/assets/Unifont-Regular.otf"),
        };
        install(&ctx, app_fonts, None, None).unwrap();
        let _ = ctx.run_ui(Default::default(), |_| {});
        let _ = ctx.run_ui(Default::default(), |_| {});
        let width = |family: FontFamily| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    "Database".into(),
                    egui::FontId::new(20.0, family),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            })
        };
        let named = |n: &str| width(FontFamily::Name(n.into()));
        let (thin, regular) = (named(crate::FONT_THIN), width(FontFamily::Proportional));
        let (medium, semibold, bold) = (
            named(crate::FONT_MEDIUM),
            named(crate::FONT_SEMIBOLD),
            named(crate::FONT_BOLD),
        );
        assert!(thin < regular && regular < medium && medium < semibold && semibold < bold);
    }

    #[test]
    fn a_font_collection_loads_at_its_first_face() {
        // macOS ships Menlo as a .ttc; skip where it doesn't exist.
        let menlo = Path::new("/System/Library/Fonts/Menlo.ttc");
        if menlo.exists() {
            assert!(supported_extension(menlo));
            assert!(read_valid_font(menlo).is_ok());
        }
    }

    #[test]
    fn rejects_paths_outside_the_font_library() {
        assert!(imported_path("../font.ttf").is_err());
        assert!(imported_path("font.woff").is_err());
    }

    #[test]
    fn accepts_a_real_opentype_font() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/assets/IBMPlexSans-Regular.ttf");
        assert!(read_valid_font(&path).is_ok());
    }

    #[test]
    fn default_code_family_is_monospace() {
        let ctx = egui::Context::default();
        install(
            &ctx,
            AppFonts {
                universal_regular: include_bytes!("../../../app/assets/Unifont-Regular.otf"),
            },
            None,
            None,
        )
        .unwrap();
        let _ = ctx.run_ui(Default::default(), |_| {});

        let font = egui::FontId::new(12.0, egui::FontFamily::Monospace);
        let (narrow, wide) = ctx.fonts_mut(|fonts| {
            let mut width = |text: &str| {
                fonts
                    .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            };
            (width("iiii"), width("WWWW"))
        });
        assert!(
            (wide - narrow).abs() < 0.01,
            "code family must be fixed-width"
        );
    }
}
