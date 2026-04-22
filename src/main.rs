#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod archive;
mod config;
mod models;
mod plan;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "WinRehome",
        options,
        Box::new(|creation_context| {
            app::configure_egui(&creation_context.egui_ctx);
            Ok(Box::new(app::WinRehomeApp::new()))
        }),
    )
}
