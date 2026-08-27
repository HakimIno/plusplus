//! Read-only inspection for structured and binary database values.

use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dbcore::Value;

use crate::components;
use crate::style::palette;

const BLOB_PREVIEW_BYTES: usize = 16 * 1024;
const INLINE_TEXTURE_MAX_EDGE: u32 = 1_200;
const INLINE_SOURCE_MAX_PIXELS: u64 = 100_000_000;
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerKind {
    Json,
    Blob,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImagePreviewKey {
    tab_id: u64,
    row: usize,
    column: usize,
    address: usize,
    length: usize,
    head: u64,
    tail: u64,
}

impl ImagePreviewKey {
    pub(crate) fn new(tab_id: u64, row: usize, column: usize, bytes: &[u8]) -> Self {
        fn signature(chunk: &[u8]) -> u64 {
            let mut padded = [0_u8; 8];
            padded[..chunk.len()].copy_from_slice(chunk);
            u64::from_le_bytes(padded)
        }

        Self {
            tab_id,
            row,
            column,
            address: bytes.as_ptr() as usize,
            length: bytes.len(),
            head: signature(&bytes[..bytes.len().min(8)]),
            tail: signature(&bytes[bytes.len().saturating_sub(8)..]),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ImagePreview {
    pub(crate) texture: egui::TextureHandle,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: &'static str,
    pub(crate) bytes_len: usize,
}

struct CachedImagePreview {
    key: ImagePreviewKey,
    value: Result<ImagePreview, String>,
}

/// One decoded Details thumbnail. Selecting another image drops the previous GPU texture,
/// keeping memory bounded even when a table contains many large BLOBs.
#[derive(Default)]
pub(crate) struct ImagePreviewCache {
    current: Option<CachedImagePreview>,
}

impl ImagePreviewCache {
    pub(crate) fn get(
        &mut self,
        ctx: &egui::Context,
        key: ImagePreviewKey,
        bytes: &[u8],
    ) -> Result<ImagePreview, String> {
        if let Some(cached) = &self.current {
            if cached.key == key {
                return cached.value.clone();
            }
        }

        let value = decode_image_preview(ctx, key, bytes);
        self.current = Some(CachedImagePreview {
            key,
            value: value.clone(),
        });
        value
    }
}

impl ViewerKind {
    pub(crate) fn action_label(self) -> &'static str {
        match self {
            Self::Json => "View JSON",
            Self::Blob => "Inspect BLOB",
            Self::Image => "View image",
        }
    }
}

#[derive(Clone)]
enum ViewerContent {
    Json {
        formatted: String,
        parse_error: Option<String>,
    },
    Blob {
        bytes: Arc<[u8]>,
        hex: String,
        truncated: bool,
    },
    Image {
        bytes: Arc<[u8]>,
        uri: String,
        format: &'static str,
        width: u32,
        height: u32,
    },
}

/// A self-contained snapshot, so changing tabs or re-running the query cannot invalidate an
/// already-open viewer.
#[derive(Clone)]
pub(crate) struct ValueViewer {
    column: String,
    type_name: String,
    content: ViewerContent,
}

impl ValueViewer {
    pub(crate) fn kind(type_name: &str, value: &Value) -> Option<ViewerKind> {
        match value {
            Value::Bytes(bytes) => Some(if image_format(bytes).is_some() {
                ViewerKind::Image
            } else {
                ViewerKind::Blob
            }),
            Value::Text(text) if is_json(type_name, text) => Some(ViewerKind::Json),
            _ => None,
        }
    }

    pub(crate) fn is_decodable_image(bytes: &[u8]) -> bool {
        image_metadata(bytes).is_some()
    }

    pub(crate) fn new(column: &str, type_name: &str, value: &Value) -> Option<Self> {
        let content = match value {
            Value::Bytes(bytes) => {
                if let Some((format, width, height)) = image_metadata(bytes) {
                    let id = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
                    ViewerContent::Image {
                        bytes: Arc::from(bytes.as_slice()),
                        uri: format!("bytes://database-image-{id}.{format}"),
                        format,
                        width,
                        height,
                    }
                } else {
                    let preview_len = bytes.len().min(BLOB_PREVIEW_BYTES);
                    ViewerContent::Blob {
                        bytes: Arc::from(bytes.as_slice()),
                        hex: hex_dump(&bytes[..preview_len]),
                        truncated: bytes.len() > preview_len,
                    }
                }
            }
            Value::Text(text) if is_json(type_name, text) => {
                let (formatted, parse_error) = match serde_json::from_str::<serde_json::Value>(text)
                {
                    Ok(value) => (
                        serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.clone()),
                        None,
                    ),
                    Err(error) => (text.clone(), Some(error.to_string())),
                };
                ViewerContent::Json {
                    formatted,
                    parse_error,
                }
            }
            _ => return None,
        };
        Some(Self {
            column: column.to_string(),
            type_name: type_name.to_string(),
            content,
        })
    }

    pub(crate) fn show(&self, ctx: &egui::Context) -> bool {
        let mut open = true;
        let mut close_clicked = false;
        components::dialog_window(format!("Value viewer — {}", self.column))
            .open(&mut open)
            .resizable(true)
            .default_size([760.0, 580.0])
            .min_size([480.0, 320.0])
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                self.header(ui);
                ui.add_space(10.0);

                match &self.content {
                    ViewerContent::Json {
                        formatted,
                        parse_error,
                    } => json_view(ui, formatted, parse_error.as_deref()),
                    ViewerContent::Blob {
                        bytes,
                        hex,
                        truncated,
                    } => blob_view(ui, bytes, hex, *truncated),
                    ViewerContent::Image {
                        bytes,
                        uri,
                        width,
                        height,
                        ..
                    } => image_view(ui, bytes.clone(), uri, *width, *height),
                }

                components::dialog_footer(ui, |ui| {
                    if components::button(ui, crate::icons::close(), "Close", true).clicked() {
                        close_clicked = true;
                    }
                    if ui.button("Copy value").clicked() {
                        match &self.content {
                            ViewerContent::Json { formatted, .. } => {
                                ui.ctx().copy_text(formatted.clone());
                            }
                            ViewerContent::Blob { hex, .. } => ui.ctx().copy_text(hex.clone()),
                            ViewerContent::Image { bytes, .. } => {
                                ui.ctx().copy_text(format!("[{} bytes]", bytes.len()));
                            }
                        }
                    }
                });
            });

        !open || close_clicked || ctx.input(|input| input.key_pressed(egui::Key::Escape))
    }

    fn header(&self, ui: &mut egui::Ui) {
        let (kind, detail) = match &self.content {
            ViewerContent::Json { formatted, .. } => ("JSON", format_bytes(formatted.len() as u64)),
            ViewerContent::Blob { bytes, .. } => ("BLOB", format_bytes(bytes.len() as u64)),
            ViewerContent::Image {
                bytes,
                format,
                width,
                height,
                ..
            } => (
                "IMAGE",
                format!(
                    "{}  ·  {} × {} px  ·  {}",
                    format.to_ascii_uppercase(),
                    width,
                    height,
                    format_bytes(bytes.len() as u64)
                ),
            ),
        };

        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(palette::ACCENT().linear_multiply(0.14))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(7, 3))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(kind)
                            .monospace()
                            .size(10.5)
                            .strong()
                            .color(palette::ACCENT()),
                    );
                });
            ui.label(
                egui::RichText::new(&self.type_name)
                    .monospace()
                    .color(palette::TEXT_WEAK()),
            );
            ui.separator();
            ui.label(egui::RichText::new(detail).color(palette::TEXT_FAINT()));
        });
    }
}

fn json_view(ui: &mut egui::Ui, formatted: &str, parse_error: Option<&str>) {
    if let Some(error) = parse_error {
        ui.label(
            egui::RichText::new(format!("Invalid JSON: {error}"))
                .color(palette::DANGER())
                .small(),
        );
        ui.add_space(6.0);
    }
    code_surface(ui, "json_value_scroll", formatted);
}

fn blob_view(ui: &mut egui::Ui, bytes: &[u8], hex: &str, truncated: bool) {
    ui.label(
        egui::RichText::new("Offset    Hex bytes                                         ASCII")
            .monospace()
            .size(10.5)
            .color(palette::TEXT_FAINT()),
    );
    ui.add_space(4.0);
    code_surface(ui, "blob_value_scroll", hex);
    if truncated {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "Showing the first {} of {}",
                format_bytes(BLOB_PREVIEW_BYTES as u64),
                format_bytes(bytes.len() as u64)
            ))
            .small()
            .color(palette::TEXT_FAINT()),
        );
    }
}

fn code_surface(ui: &mut egui::Ui, id: &'static str, text: &str) {
    egui::Frame::new()
        .fill(palette::CODE_BG())
        .stroke(egui::Stroke::new(1.0, palette::BORDER()))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt(id)
                .auto_shrink([false, false])
                .max_height(430.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .monospace()
                                .size(11.5)
                                .color(palette::TEXT()),
                        )
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
        });
}

fn image_view(ui: &mut egui::Ui, bytes: Arc<[u8]>, uri: &str, width: u32, height: u32) {
    let available = egui::vec2(ui.available_width(), ui.available_height().min(430.0));
    egui::Frame::new()
        .fill(palette::CODE_BG())
        .stroke(egui::Stroke::new(1.0, palette::BORDER()))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::from_bytes(uri.to_string(), bytes)
                        .max_size(available - egui::vec2(24.0, 24.0))
                        .shrink_to_fit()
                        .maintain_aspect_ratio(true)
                        .alt_text(format!("Database image, {width} by {height} pixels")),
                );
            });
        });
}

fn is_json(type_name: &str, text: &str) -> bool {
    if type_name.to_ascii_uppercase().contains("JSON") {
        return true;
    }
    let trimmed = text.trim_start();
    matches!(trimmed.as_bytes().first(), Some(b'{') | Some(b'['))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
}

fn image_metadata(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    let label = image_format_label(format)?;
    let (width, height) = reader.into_dimensions().ok()?;
    Some((label, width, height))
}

fn image_format(bytes: &[u8]) -> Option<&'static str> {
    image_format_label(image::guess_format(bytes).ok()?)
}

fn image_format_label(format: image::ImageFormat) -> Option<&'static str> {
    Some(match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::Gif => "gif",
        image::ImageFormat::WebP => "webp",
        _ => return None,
    })
}

fn decode_image_preview(
    ctx: &egui::Context,
    key: ImagePreviewKey,
    bytes: &[u8],
) -> Result<ImagePreview, String> {
    let (format, width, height) = image_metadata(bytes)
        .ok_or_else(|| "This BLOB is not a supported PNG, JPEG, GIF, or WebP image".to_string())?;
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels > INLINE_SOURCE_MAX_PIXELS {
        return Err(format!(
            "Image is too large to preview safely ({} × {} px)",
            width, height
        ));
    }

    let decoded = image::load_from_memory(bytes).map_err(|error| error.to_string())?;
    let thumbnail = decoded.thumbnail(INLINE_TEXTURE_MAX_EDGE, INLINE_TEXTURE_MAX_EDGE);
    let rgba = thumbnail.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let texture = ctx.load_texture(
        format!(
            "details-image-{}-{}-{}-{}",
            key.tab_id, key.row, key.column, key.address
        ),
        color,
        egui::TextureOptions::LINEAR,
    );

    Ok(ImagePreview {
        texture,
        width,
        height,
        format,
        bytes_len: bytes.len(),
    })
}

fn hex_dump(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(4));
    for (line, chunk) in bytes.chunks(16).enumerate() {
        let _ = write!(output, "{:08x}  ", line * 16);
        for index in 0..16 {
            if let Some(byte) = chunk.get(index) {
                let _ = write!(output, "{byte:02x} ");
            } else {
                output.push_str("   ");
            }
            if index == 7 {
                output.push(' ');
            }
        }
        output.push(' ');
        for byte in chunk {
            output.push(if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            });
        }
        output.push('\n');
    }
    output
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_columns_and_object_text_are_viewable() {
        assert_eq!(
            ValueViewer::kind("JSONB", &Value::Text("null".into())),
            Some(ViewerKind::Json)
        );
        assert_eq!(
            ValueViewer::kind("TEXT", &Value::Text(r#"{"ok":true}"#.into())),
            Some(ViewerKind::Json)
        );
        assert_eq!(
            ValueViewer::kind("TEXT", &Value::Text("ordinary text".into())),
            None
        );
    }

    #[test]
    fn blob_hex_dump_has_offsets_hex_and_ascii() {
        let dump = hex_dump(b"Hello\0world");
        assert!(dump.starts_with("00000000  48 65 6c 6c 6f 00 77 6f"));
        assert!(dump.contains("Hello.world"));
    }

    #[test]
    fn bundled_png_is_detected_as_an_image() {
        let bytes = include_bytes!("../assets/illus/empty-chameleon.png");
        let (format, width, height) = image_metadata(bytes).expect("valid bundled PNG");
        assert_eq!(format, "png");
        assert!(width > 0 && height > 0);
    }

    #[test]
    fn inline_preview_reuses_its_decoded_texture() {
        let ctx = egui::Context::default();
        let bytes = include_bytes!("../assets/illus/empty-chameleon.png");
        let key = ImagePreviewKey::new(7, 2, 4, bytes);
        let mut cache = ImagePreviewCache::default();

        let first = cache.get(&ctx, key, bytes).expect("preview");
        let second = cache.get(&ctx, key, bytes).expect("cached preview");

        assert_eq!(first.texture.id(), second.texture.id());
        assert_eq!((first.width, first.height), (second.width, second.height));
    }

    #[test]
    fn viewer_variants_render_headlessly() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let viewers = [
            ValueViewer::new(
                "payload",
                "JSONB",
                &Value::Text(r#"{"rows":[1,2],"ok":true}"#.into()),
            )
            .unwrap(),
            ValueViewer::new("data", "BLOB", &Value::Bytes(vec![0, 1, 2, 255])).unwrap(),
            ValueViewer::new(
                "avatar",
                "BLOB",
                &Value::Bytes(include_bytes!("../assets/illus/empty-chameleon.png").to_vec()),
            )
            .unwrap(),
        ];

        for viewer in viewers {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(!viewer.show(ui.ctx()));
            });
        }
    }
}
