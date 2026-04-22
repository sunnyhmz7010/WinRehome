# Repository Guidelines

## Project Summary

`WinRehome` is a Windows-only migration backup tool, not a disk-image backup product.

- Goal: keep only data with real migration value while minimizing archive size.
- Installed applications: record inventory only; do not back up installed binaries.
- Portable applications: detect as candidates, review, and pack into the main archive.
- User files: include by allow-list and migration heuristics, not broad directory exclusion alone.

Keep `README.md` user-facing. Contributor rules, operational constraints, and collaboration guidance belong here.

## Project Structure & Important Files

- `src/main.rs`: desktop app entry point
- `src/app.rs`: egui UI shell and preview screen
- `src/models.rs`: shared domain models
- `src/plan.rs`: scan, classification, and preview plan logic
- `src/archive.rs`: `.wrh` archive writing, manifest reading, and restore logic
- `docs/architecture.md`: architecture and reliability notes
- `.github/workflows/cd.yaml`: GitHub Release build and asset upload workflow
- `README.md`: Chinese user-facing project overview
- `Cargo.toml`: package metadata and dependencies

Do not edit generated files under `target/`.

## Build, Test, and Development Commands

Run commands from the repo root and prefix shell commands with `rtk`.

- `rtk cargo check`: fast compile validation
- `rtk cargo fmt`: format source files
- `rtk cargo run`: launch the desktop prototype
- `rtk cargo test`: run tests

If dependency fetches fail because of proxy settings, clear proxy environment variables before retrying.

## Coding Style & Naming Conventions

- Use standard Rust formatting with 4-space indentation and `cargo fmt`.
- Use `snake_case` for files, modules, and functions.
- Use `PascalCase` for structs and enums.
- Keep UI code in `src/app.rs`; move scanning, archive, and restore behavior into focused modules.
- Prefer explicit rule names such as `is_known_noise` over vague helpers.

## Backup Classification Rules

- Do not treat software under `Program Files`, `Program Files (x86)`, `WindowsApps`, or Windows system paths as portable backup content.
- Installed software detection should prefer registry uninstall metadata and other verifiable Windows sources.
- Portable-app detection must stay explainable. Favor confidence scoring and user review over opaque “smart” guesses.
- Default exclusions must cover cache, temp, logs, and build artifacts unless there is a documented restore value.
- When classification rules change, update both code and `docs/architecture.md`.

## Restore Safety Rules

- Reliability is more important than compression ratio or scan aggressiveness.
- Archive changes must preserve validation and safe restore behavior.
- Do not introduce restore flows that overwrite user files silently.
- If archive format behavior changes, document compatibility expectations in `docs/architecture.md`.
- The current archive format is a single `.wrh` file with a manifest footer; keep format updates explicit and reviewable.
- Restore logic must continue to verify stored size and CRC before reporting success.

## Testing & Validation

- Add unit tests beside the module with `#[cfg(test)]` when logic is self-contained.
- Add integration tests under `tests/` for scan, archive, and restore flows as those features land.
- Name tests by behavior, for example `skips_program_files_roots`.
- Before handoff, run at least `rtk cargo check`; run `rtk cargo test` when tests exist or logic changes materially.

## Commit, PR, and Working Style

- Use Conventional Commit style, for example `feat: add portable app confidence scoring`.
- Keep changes small, targeted, and easy to verify.
- Prefer conservative behavior over clever behavior when user data safety is involved.
- Do not leave temporary debug files, local archives, or exploratory outputs in the working tree.
- When repo structure, commands, or rules change, update this file in the same task.

## Release Conventions

- GitHub release packaging is handled by `.github/workflows/cd.yaml`.
- The release workflow is triggered by the `Release published` event.
- Release assets should include a Windows executable named like `WinRehome-vX.Y.Z-windows-x64.exe`.
- If release packaging changes, update both the workflow and this file in the same task.
