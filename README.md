# WinRehome

Windows-only migration backup prototype focused on preserving valuable user data while minimizing archive size.

## Product direction

- Installed applications are recorded from Windows uninstall metadata instead of backing up binaries.
- Portable applications are discovered as candidates and backed up as part of a single archive, not one zip per app.
- User data is selected through allow-lists and migration-value heuristics, not by blindly excluding `Program Files`.
- Cache, temp, logs, and build outputs are excluded by default.

## First-build stack

- Rust 1.94+
- `eframe/egui` for a lightweight native desktop shell without WebView2
- Windows registry scanning via `winreg`
- File system discovery via `walkdir`

## Planned milestones

1. Preview scan and classification rules
2. Reviewable backup plan with user overrides
3. Single-file archive format with metadata index and chunked compression
4. Restore workflow for personal files and portable applications
5. Incremental scan using NTFS change tracking
