fn main() -> eframe::Result<()> {
    // Decoding moshed streams floods the console with h264 "error" spam by
    // design (broken references ARE the effect). Only true fatals get through;
    // CLI ffmpeg stderr capture for import/mux errors is a separate process
    // and unaffected.
    if ffmpeg_next::init().is_ok() {
        ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Fatal);
    }

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
