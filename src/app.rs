use crate::plan;
use eframe::egui::{self, Color32, RichText};

#[derive(Default)]
pub struct MigrateBackupApp {
    preview: Option<plan::BackupPreview>,
    last_error: Option<String>,
}

impl eframe::App for MigrateBackupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("WinRehome");
            ui.label(
                "A Windows-only migration backup prototype focused on valuable personal data.",
            );
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui.button("Generate Preview").clicked() {
                    match plan::build_preview() {
                        Ok(preview) => {
                            self.preview = Some(preview);
                            self.last_error = None;
                        }
                        Err(error) => {
                            self.last_error = Some(error.to_string());
                        }
                    }
                }

                if ui.button("Clear").clicked() {
                    self.preview = None;
                    self.last_error = None;
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
                    "High-value user roots selected: {}",
                    preview.user_data_roots.len()
                ));
                ui.label(format!(
                    "Global exclusion rules: {}",
                    preview.exclusion_rules.len()
                ));

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
                        for item in preview.portable_candidates.iter().take(12) {
                            ui.label(format!(
                                "{} ({})",
                                item.display_name,
                                item.confidence_label()
                            ));
                            ui.small(format!("Root: {}", item.root_path.display()));
                            ui.small(format!("Main exe: {}", item.main_executable.display()));
                            for reason in item.reasons.iter().take(2) {
                                ui.small(reason);
                            }
                            ui.add_space(4.0);
                        }
                    });

                    columns[2].group(|ui| {
                        ui.label(RichText::new("Included Roots").strong());
                        for root in &preview.user_data_roots {
                            ui.label(format!("{} -> {}", root.label, root.path.display()));
                        }

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
