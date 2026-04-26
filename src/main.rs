#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod archive;
mod config;
mod models;
mod plan;

fn main() -> eframe::Result<()> {
    let saved_config = config::load_config().ok().flatten();
    let remember_window_geometry = saved_config
        .as_ref()
        .map(|saved| saved.remember_window_geometry)
        .unwrap_or(true);
    let saved_window_geometry = saved_config
        .and_then(|saved| saved.last_window_geometry)
        .filter(|saved| saved.is_valid());
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1360.0, 900.0])
        .with_min_inner_size([1180.0, 820.0]);
    let mut centered = true;
    if remember_window_geometry {
        if let Some(saved) = saved_window_geometry {
            viewport = viewport
                .with_position([saved.x, saved.y])
                .with_inner_size([saved.width, saved.height])
                .with_maximized(saved.maximized);
            centered = false;
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        centered,
        renderer: eframe::Renderer::Glow,
        persist_window: false,
        persistence_path: config::eframe_persistence_path().ok(),
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
