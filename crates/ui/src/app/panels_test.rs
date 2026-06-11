use egui::*;
fn test(ui: &mut Ui) {
    let img = Image::new("foo");
    ui.menu_button(Button::image_and_text(img, "bar"), |ui| {});
}
