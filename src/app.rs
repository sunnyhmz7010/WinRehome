use crate::{archive, plan};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone)]
struct LoadedArchive {
    path: PathBuf,
    manifest: archive::ArchiveManifest,
}

#[derive(Default)]
pub struct WinRehomeApp {
    preview: Option<plan::BackupPreview>,
    selected_user_roots: HashSet<String>,
    selected_portable_apps: HashSet<String>,
    archive_path_input: String,
    restore_destination_input: String,
    loaded_archive: Option<LoadedArchive>,
    last_archive: Option<archive::BackupResult>,
    last_restore: Option<archive::RestoreResult>,
    last_error: Option<String>,
}

impl WinRehomeApp {
    fn load_preview(&mut self, preview: plan::BackupPreview) {
        self.selected_user_roots = preview.default_user_root_keys();
        self.selected_portable_apps = preview.default_portable_keys();
        self.preview = Some(preview);
        self.last_error = None;
    }

    fn load_archive_from_path(&mut self, path: PathBuf) {
        match archive::read_archive_manifest(&path) {
            Ok(manifest) => {
                self.restore_destination_input = archive::default_restore_dir(&path)
                    .map(|value| value.display().to_string())
                    .unwrap_or_default();
                self.archive_path_input = path.display().to_string();
                self.loaded_archive = Some(LoadedArchive { path, manifest });
                self.last_restore = None;
                self.last_error = None;
            }
            Err(error) => {
                self.loaded_archive = None;
                self.last_restore = None;
                self.last_error = Some(error.to_string());
            }
        }
    }

    fn clear_preview(&mut self) {
        self.preview = None;
        self.selected_user_roots.clear();
        self.selected_portable_apps.clear();
        self.last_archive = None;
        self.last_restore = None;
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
                            self.last_archive = None;
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
                    format!("Operation failed: {error}"),
                );
            }

            if let Some(result) = &self.last_archive {
                ui.add_space(10.0);
                ui.colored_label(
                    Color32::from_rgb(35, 120, 70),
                    format!(
                        "Archive created: {} ({} files, {}, stored {})",
                        result.archive_path.display(),
                        result.file_count,
                        format_bytes(result.original_bytes),
                        format_bytes(result.stored_bytes)
                    ),
                );
            }

            if let Some(result) = &self.last_restore {
                ui.add_space(10.0);
                ui.colored_label(
                    Color32::from_rgb(45, 95, 175),
                    format!(
                        "Archive restored: {} -> {} ({} files, {})",
                        result.archive_path.display(),
                        result.destination_root.display(),
                        result.restored_files,
                        format_bytes(result.restored_bytes)
                    ),
                );
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);
            ui.group(|ui| {
                ui.label(RichText::new("Archive Restore").strong());
                ui.horizontal(|ui| {
                    if ui.button("Load Latest Archive").clicked() {
                        match archive::find_latest_archive() {
                            Ok(Some(path)) => self.load_archive_from_path(path),
                            Ok(None) => {
                                self.loaded_archive = None;
                                self.last_restore = None;
                                self.last_error = Some(
                                    "No .wrh archive was found in the default backup folder."
                                        .to_string(),
                                );
                            }
                            Err(error) => {
                                self.loaded_archive = None;
                                self.last_restore = None;
                                self.last_error = Some(error.to_string());
                            }
                        }
                    }

                    ui.label("Archive path");
                    ui.text_edit_singleline(&mut self.archive_path_input);
                    if ui.button("Load Archive").clicked() {
                        let path = PathBuf::from(self.archive_path_input.trim());
                        self.load_archive_from_path(path);
                    }
                });

                if let Some(loaded) = self.loaded_archive.clone() {
                    ui.add_space(8.0);
                    ui.label(format!("Loaded archive: {}", loaded.path.display()));
                    ui.label(format!(
                        "Created: {} | Files: {} | Original: {} | Stored: {}",
                        loaded.manifest.created_at_unix,
                        loaded.manifest.files.len(),
                        format_bytes(loaded.manifest.original_bytes),
                        format_bytes(loaded.manifest.stored_bytes)
                    ));
                    ui.label(format!(
                        "Installed app records: {} | User roots: {} | Portable apps: {}",
                        loaded.manifest.installed_apps.len(),
                        loaded.manifest.selected_user_roots.len(),
                        loaded.manifest.selected_portable_apps.len()
                    ));
                    ui.horizontal(|ui| {
                        ui.label("Restore to");
                        ui.text_edit_singleline(&mut self.restore_destination_input);
                        if ui.button("Restore Archive").clicked() {
                            let destination = PathBuf::from(self.restore_destination_input.trim());
                            match archive::restore_archive(&loaded.path, &destination) {
                                Ok(result) => {
                                    self.last_restore = Some(result);
                                    self.last_error = None;
                                }
                                Err(error) => {
                                    self.last_restore = None;
                                    self.last_error = Some(error.to_string());
                                }
                            }
                        }
                    });
                }
            });

            if let Some(preview) = self.preview.clone() {
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

                    if ui.button("Create Backup Archive").clicked() {
                        match archive::create_backup_archive(
                            &preview,
                            &self.selected_user_roots,
                            &self.selected_portable_apps,
                        ) {
                            Ok(result) => {
                                self.load_archive_from_path(result.archive_path.clone());
                                self.last_archive = Some(result);
                                self.last_restore = None;
                                self.last_error = None;
                            }
                            Err(error) => {
                                self.last_archive = None;
                                self.loaded_archive = None;
                                self.last_restore = None;
                                self.last_error = Some(error.to_string());
                            }
                        }
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
