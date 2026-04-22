use crate::{archive, config, plan};
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
    restore_user_data: bool,
    restore_portable_apps: bool,
    loaded_archive: Option<LoadedArchive>,
    last_archive: Option<archive::BackupResult>,
    last_restore: Option<archive::RestoreResult>,
    last_error: Option<String>,
}

impl WinRehomeApp {
    pub fn new() -> Self {
        let mut app = Self {
            restore_user_data: true,
            restore_portable_apps: true,
            ..Self::default()
        };
        if let Ok(Some(saved)) = config::load_config() {
            app.selected_user_roots = config::normalize_existing_paths(&saved.selected_user_roots);
            app.selected_portable_apps =
                config::normalize_existing_paths(&saved.selected_portable_apps);
            app.archive_path_input = saved.last_archive_path.unwrap_or_default();
            app.restore_destination_input = saved.last_restore_destination.unwrap_or_default();
            app.restore_user_data = saved.restore_user_data;
            app.restore_portable_apps = saved.restore_portable_apps;
        }
        app
    }

    fn load_preview(&mut self, preview: plan::BackupPreview) {
        let saved_user_roots: HashSet<String> = preview
            .user_data_roots
            .iter()
            .map(|root| plan::path_key(&root.path))
            .filter(|key| self.selected_user_roots.contains(key))
            .collect();
        let saved_portable_apps: HashSet<String> = preview
            .portable_candidates
            .iter()
            .map(|candidate| plan::path_key(&candidate.root_path))
            .filter(|key| self.selected_portable_apps.contains(key))
            .collect();

        self.selected_user_roots = if saved_user_roots.is_empty() {
            preview.default_user_root_keys()
        } else {
            saved_user_roots
        };
        self.selected_portable_apps = if saved_portable_apps.is_empty() {
            preview.default_portable_keys()
        } else {
            saved_portable_apps
        };
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
                self.restore_user_data = true;
                self.restore_portable_apps = true;
                self.last_restore = None;
                self.last_error = None;
                let _ = self.persist_config();
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

    fn persist_config(&self) -> anyhow::Result<PathBuf> {
        config::save_config(&config::AppConfig {
            selected_user_roots: self.selected_user_roots.clone(),
            selected_portable_apps: self.selected_portable_apps.clone(),
            last_archive_path: (!self.archive_path_input.trim().is_empty())
                .then(|| self.archive_path_input.trim().to_string()),
            last_restore_destination: (!self.restore_destination_input.trim().is_empty())
                .then(|| self.restore_destination_input.trim().to_string()),
            restore_user_data: self.restore_user_data,
            restore_portable_apps: self.restore_portable_apps,
        })
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
                        if ui
                            .checkbox(&mut self.restore_user_data, "Restore user data")
                            .changed()
                        {
                            let _ = self.persist_config();
                        }
                        if ui
                            .checkbox(&mut self.restore_portable_apps, "Restore portable apps")
                            .changed()
                        {
                            let _ = self.persist_config();
                        }
                    });
                    ui.collapsing("Archive contents", |ui| {
                        if !loaded.manifest.selected_user_roots.is_empty() {
                            ui.label(RichText::new("User roots").strong());
                            for root in &loaded.manifest.selected_user_roots {
                                ui.small(format!("{} -> {}", root.label, root.path));
                            }
                            ui.add_space(6.0);
                        }
                        if !loaded.manifest.selected_portable_apps.is_empty() {
                            ui.label(RichText::new("Portable apps").strong());
                            for app in &loaded.manifest.selected_portable_apps {
                                ui.small(format!("{} -> {}", app.display_name, app.root_path));
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Restore to");
                        ui.text_edit_singleline(&mut self.restore_destination_input);
                        if ui.button("Restore Archive").clicked() {
                            let destination = PathBuf::from(self.restore_destination_input.trim());
                            let restore_result =
                                if self.restore_user_data && self.restore_portable_apps {
                                    archive::restore_archive(&loaded.path, &destination)
                                } else {
                                    archive::restore_archive_with_selection(
                                        &loaded.path,
                                        &destination,
                                        archive::RestoreSelection {
                                            restore_user_data: self.restore_user_data,
                                            restore_portable_apps: self.restore_portable_apps,
                                        },
                                    )
                                };
                            match restore_result {
                                Ok(result) => {
                                    self.last_restore = Some(result);
                                    self.last_error = None;
                                    let _ = self.persist_config();
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
                        let _ = self.persist_config();
                    }

                    if ui.button("Clear Selections").clicked() {
                        self.selected_user_roots.clear();
                        self.selected_portable_apps.clear();
                        let _ = self.persist_config();
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
                                let _ = self.persist_config();
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
                                        let _ = self.persist_config();
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
                                        let _ = self.persist_config();
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
