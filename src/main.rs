fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("rustjay-mosh")
            .with_inner_size([1280.0, 720.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "rustjay-mosh",
        options,
        Box::new(|cc| Ok(Box::new(rustjay_mosh::ui::app::MoshApp::new(cc)))),
    )
}
