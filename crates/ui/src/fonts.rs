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
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "ttf" | "otf"))
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
    skrifa::FontRef::new(&bytes)
        .map_err(|_| "The selected file is not a valid font".to_string())?;
    Ok(bytes)
}

/// Validate and copy a font into the app-owned library. Existing identical files are reused;
/// a numeric suffix prevents an import from silently replacing a different font.
pub(crate) fn import(path: &Path) -> Result<FontOption, String> {
    if !supported_extension(path) {
        return Err("Choose a .ttf or .otf font file".to_string());
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

pub(crate) fn install(
    ctx: &egui::Context,
    app_fonts: AppFonts,
    ui_font: Option<&str>,
    code_font: Option<&str>,
) -> Result<(), String> {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("inter", app_fonts.ui_regular),
        ("inter_semibold", app_fonts.ui_semibold),
        ("jetbrains_mono", app_fonts.code_regular),
        ("thai", app_fonts.thai_regular),
        ("thai_semibold", app_fonts.thai_semibold),
        ("unifont", app_fonts.universal_regular),
    ] {
        insert(&mut fonts, name, bytes);
    }

    let ui_custom = ui_font
        .map(imported_path)
        .transpose()?
        .map(|path| read_valid_font(&path))
        .transpose()?;
    let code_custom = code_font
        .map(imported_path)
        .transpose()?
        .map(|path| read_valid_font(&path))
        .transpose()?;
    if let Some(bytes) = &ui_custom {
        insert(&mut fonts, "custom_ui", bytes);
    }
    if let Some(bytes) = &code_custom {
        insert(&mut fonts, "custom_code", bytes);
    }

    let mut proportional = Vec::new();
    if ui_custom.is_some() {
        proportional.push("custom_ui".to_owned());
    }
    proportional.extend(["inter".to_owned(), "thai".to_owned(), "unifont".to_owned()]);
    fonts
        .families
        .insert(FontFamily::Proportional, proportional);

    let mut monospace = Vec::new();
    if code_custom.is_some() {
        monospace.push("custom_code".to_owned());
    }
    monospace.extend([
        "jetbrains_mono".to_owned(),
        "thai".to_owned(),
        "unifont".to_owned(),
    ]);
    fonts.families.insert(FontFamily::Monospace, monospace);

    let mut headings = Vec::new();
    if ui_custom.is_some() {
        headings.push("custom_ui".to_owned());
    }
    headings.extend([
        "inter_semibold".to_owned(),
        "thai_semibold".to_owned(),
        "inter".to_owned(),
        "thai".to_owned(),
        "unifont".to_owned(),
    ]);
    fonts
        .families
        .insert(FontFamily::Name(HEADING_FAMILY.into()), headings);
    ctx.set_fonts(fonts);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_outside_the_font_library() {
        assert!(imported_path("../font.ttf").is_err());
        assert!(imported_path("font.woff").is_err());
    }

    #[test]
    fn accepts_a_real_opentype_font() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/assets/Inter-Regular.ttf");
        assert!(read_valid_font(&path).is_ok());
    }
}
