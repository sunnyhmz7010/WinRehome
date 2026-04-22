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
        Box::new(|_creation_context| Ok(Box::new(app::WinRehomeApp::new()))),
    )
}
