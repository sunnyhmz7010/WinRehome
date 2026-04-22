use crate::plan;
use eframe::egui::{self, Color32, RichText};
use std::collections::HashSet;

#[derive(Default)]
pub struct WinRehomeApp {
    preview: Option<plan::BackupPreview>,
    selected_user_roots: HashSet<String>,
    selected_portable_apps: HashSet<String>,
    last_error: Option<String>,
}

impl WinRehomeApp {
    fn load_preview(&mut self, preview: plan::BackupPreview) {
        self.selected_user_roots = preview.default_user_root_keys();
        self.selected_portable_apps = preview.default_portable_keys();
        self.preview = Some(preview);
        self.last_error = None;
    }

    fn clear_preview(&mut self) {
        self.preview = None;
        self.selected_user_roots.clear();
        self.selected_portable_apps.clear();
        self.last_error = None;
    }
}

impl eframe::App for WinRehomeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("WinRehome");
            ui.label("Windows migration backup prototype focused on valuable data only.");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui.button("Generate Preview").clicked() {
                    match plan::build_preview() {
                        Ok(preview) => self.load_preview(preview),
                        Err(error) => {
                            self.last_error = Some(error.to_string());
                        }
                    }
                }

                if ui.button("Clear").clicked() {
                    self.clear_preview();
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            ui.group(|ui| {
                ui.label(RichText::new("Reliability Guardrails").strong());
                ui.label("- Installed apps are recorded, not packed.");
                ui.label("- Portable apps are only candidates until confirmed.");
                ui.label("- User data uses allow-lists, not 'exclude Program Files'.");
                ui.label("- Cache, temp, log, and build-output roots are excluded by default.");
            });

            if let Some(error) = &self.last_error {
                ui.add_space(10.0);
                ui.colored_label(
                    Color32::from_rgb(200, 40, 40),
                    format!("Preview generation failed: {error}"),
                );
            }

            if let Some(preview) = &self.preview {
                let summary = preview
                    .summarize_selection(&self.selected_user_roots, &self.selected_portable_apps);

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                ui.heading("Preview");
                ui.label(format!(
                    "Installed apps recorded: {}",
                    preview.installed_apps.len()
                ));
                ui.label(format!(
                    "Portable candidates found: {}",
                    preview.portable_candidates.len()
                ));
                ui.label(format!(
                    "High-value user roots found: {}",
                    preview.user_data_roots.len()
                ));
                ui.label(format!(
                    "Global exclusion rules: {}",
                    preview.exclusion_rules.len()
                ));

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Use Recommended").clicked() {
                        self.selected_user_roots = preview.default_user_root_keys();
                        self.selected_portable_apps = preview.default_portable_keys();
                    }

                    if ui.button("Clear Selections").clicked() {
                        self.selected_user_roots.clear();
                        self.selected_portable_apps.clear();
                    }
                });

                ui.add_space(10.0);
                ui.group(|ui| {
                    ui.label(RichText::new("Selected Backup Plan").strong());
                    ui.label(format!("User roots kept: {}", summary.selected_user_roots));
                    ui.label(format!(
                        "Portable apps kept: {}",
                        summary.selected_portable_apps
                    ));
                    ui.label(format!("Estimated files: {}", summary.total_files));
                    ui.label(format!(
                        "Estimated size: {}",
                        format_bytes(summary.total_bytes)
                    ));
                });

                ui.add_space(10.0);
                ui.columns(3, |columns| {
                    columns[0].group(|ui| {
                        ui.label(RichText::new("Installed Apps").strong());
                        for app in preview.installed_apps.iter().take(12) {
                            ui.label(format!("{} [{}]", app.display_name, app.source));
                            if let Some(path) = &app.install_location {
                                ui.small(format!("Location: {}", path.display()));
                            }
                            ui.small(format!("Key: {}", app.uninstall_key));
                            ui.add_space(4.0);
                        }
                    });

                    columns[1].group(|ui| {
                        ui.label(RichText::new("Portable Candidates").strong());
                        egui::ScrollArea::vertical()
                            .max_height(420.0)
                            .show(ui, |ui| {
                                for item in preview.portable_candidates.iter().take(20) {
                                    let key = plan::path_key(&item.root_path);
                                    let mut selected = self.selected_portable_apps.contains(&key);
                                    if ui.checkbox(&mut selected, &item.display_name).changed() {
                                        if self.selected_portable_apps.contains(&key) {
                                            self.selected_portable_apps.remove(&key);
                                        } else {
                                            self.selected_portable_apps.insert(key.clone());
                                        }
                                    }
                                    ui.small(format!("Confidence: {}", item.confidence_label()));
                                    ui.small(format!("Root: {}", item.root_path.display()));
                                    ui.small(format!(
                                        "Main exe: {}",
                                        item.main_executable.display()
                                    ));
                                    ui.small(format!(
                                        "Estimated size: {} across {} files",
                                        format_bytes(item.stats.total_bytes),
                                        item.stats.file_count
                                    ));
                                    ui.small(format!(
                                        "Default: {}",
                                        if item.default_selected {
                                            "selected"
                                        } else {
                                            "not selected"
                                        }
                                    ));
                                    for reason in item.reasons.iter().take(3) {
                                        ui.small(reason);
                                    }
                                    ui.add_space(6.0);
                                }
                            });
                    });

                    columns[2].group(|ui| {
                        ui.label(RichText::new("User Data Roots").strong());
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for root in &preview.user_data_roots {
                                    let key = plan::path_key(&root.path);
                                    let mut selected = self.selected_user_roots.contains(&key);
                                    if ui.checkbox(&mut selected, root.label).changed() {
                                        if selected {
                                            self.selected_user_roots.insert(key.clone());
                                        } else {
                                            self.selected_user_roots.remove(&key);
                                        }
                                    }
                                    ui.small(format!("Category: {}", root.category));
                                    ui.small(format!("Path: {}", root.path.display()));
                                    ui.small(root.reason);
                                    ui.small(format!(
                                        "Estimated size: {} across {} files",
                                        format_bytes(root.stats.total_bytes),
                                        root.stats.file_count
                                    ));
                                    ui.small(format!(
                                        "Default: {}",
                                        if root.default_selected {
                                            "selected"
                                        } else {
                                            "not selected"
                                        }
                                    ));
                                    ui.add_space(6.0);
                                }
                            });

                        ui.add_space(10.0);
                        ui.label(RichText::new("Exclusions").strong());
                        for rule in &preview.exclusion_rules {
                            ui.small(format!("{}: {}", rule.label, rule.pattern));
                        }
                    });
                });
            }
        });
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f >= GB {
        format!("{:.2} GB", bytes_f / GB)
    } else if bytes_f >= MB {
        format!("{:.2} MB", bytes_f / MB)
    } else if bytes_f >= KB {
        format!("{:.2} KB", bytes_f / KB)
    } else {
        format!("{bytes} B")
    }
}
