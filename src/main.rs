#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod archive;
mod config;
mod models;
mod plan;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 900.0])
            .with_min_inner_size([1180.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "WinRehome",
        options,
        Box::new(|creation_context| {
            app::configure_egui(&creation_context.egui_ctx);
            Ok(Box::new(app::WinRehomeApp::new()))
        }),
    )
}
