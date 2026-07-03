//! Shared UI components used across the crate.

pub(crate) mod badge;
pub(crate) mod button;
pub(crate) mod dialog;
pub(crate) mod feedback;
pub(crate) mod input;
pub(crate) mod tabs;
pub(crate) mod toolbar;

pub(crate) use badge::*;
pub(crate) use button::*;
pub(crate) use dialog::*;
pub(crate) use feedback::*;
pub(crate) use input::*;
pub(crate) use tabs::*;
pub(crate) use toolbar::*;

use crate::style::palette;

/// Resolved colours for a selectable or draggable widget.
pub(crate) struct InteractionColors {
    pub fill: egui::Color32,
    pub stroke: egui::Stroke,
    pub text: egui::Color32,
}

/// Shared hover/selected/dragging palette for chips, rows, and tabs.
pub(crate) fn interaction_colors(
    resp: &egui::Response,
    selected: bool,
    dragging: bool,
) -> InteractionColors {
    let fill = if selected || dragging {
        palette::SURFACE()
    } else if resp.hovered() {
        palette::SURFACE_HOVER()
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if dragging {
        egui::Stroke::new(1.0, palette::ACCENT())
    } else if selected {
        egui::Stroke::new(1.0, palette::BORDER_STRONG())
    } else if resp.hovered() {
        egui::Stroke::new(1.0, palette::BORDER())
    } else {
        egui::Stroke::NONE
    };
    let text = if selected {
        palette::TEXT()
    } else {
        palette::TEXT_WEAK()
    };
    InteractionColors { fill, stroke, text }
}
