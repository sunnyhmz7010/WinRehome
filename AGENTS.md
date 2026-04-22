# AGENTS.md

## Reusable Rules

These rules are intentionally written in a reusable way so they can be copied into other repositories as a starting point.

- This repository's `Reusable Rules` section is intended to be the shared baseline across projects. By default, other project `AGENTS.md` files should keep these reusable rules aligned in structure, intent, and policy unless the user explicitly asks for a deliberate deviation.

### General Working Style

- Prefer minimal, targeted changes over broad refactors.
- Preserve existing product copy unless the task requires rewriting it.
- Keep user-facing docs concise and practical; avoid adding AI collaboration notes or marketing filler unless explicitly requested.
- Keep `README.md` user-facing and promotional for external readers. Contributor rules, operational constraints, missing-work notes, AI guidance, release-process conventions, and collaboration guidance belong in `AGENTS.md`, not `README.md`.
- When updating `README.md`, follow the style of strong, high-star GitHub project READMEs: lead with clear value, polished feature framing, concise usage/integration guidance, and externally useful examples.
- For the project's leading one-sentence summary in README or similar public-facing docs, prefer a direct product-description sentence instead of starting with the repository name or "This project is ...", unless the user explicitly asks for that phrasing.
- In public-facing docs such as `README.md`, write commands using standard upstream tooling, not local wrappers, aliases, shell functions, or private helper commands. Keep local convenience commands in contributor-only docs such as `AGENTS.md`.
- Do not add README sections framed as internal progress tracking or roadmap bookkeeping, such as “当前已实现”, “当前缺失”, “后续里程碑”, “未来计划”, or similar wording.
- Do not use “当前…” style internal status phrasing in README copy unless the user explicitly requests it. README should read like a polished public-facing project page, not an internal handoff note.
- For searches, prefer `rg`.
- Use `apply_patch` for manual edits when the environment is stable.
- Do not run destructive git commands unless explicitly requested.

### Validation And Hygiene

- Keep the working tree clean before handoff: do not leave local build outputs, dependency caches, screenshots for debugging, or temporary troubleshooting files committed or untracked.
- When the environment lacks the required toolchain and the user does not need full local verification, it is acceptable to skip heavy verification, but say so explicitly.
- Release notes are user-facing change logs. Do not include internal verification/process statements such as having run tests, builds, audits, or CI checks unless explicitly requested.
- When repository structure, commands, external capabilities, release process, or recurring engineering pitfalls change, update `AGENTS.md` in the same task. Keeping this file current is required, not optional.
- If newly learned guidance appears to be reusable across repositories rather than specific to the current project, ask whether to automatically scan other project `AGENTS.md` files, apply the shared rule where appropriate, and push those updates to their remotes.
- For GitHub-hosted repositories, maintain the baseline repository-governance files consistently across projects unless the user explicitly asks for divergence. This baseline includes `LICENSE`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, issue templates, and similar repo-health/community files.
- "Consistently" does not mean every line must be identical. Keep the structure, tone, and policy baseline aligned, but make the necessary project-specific substitutions for repository name, product name, links, version fields, platform fields, security scope, issue-form fields, and other repo-specific facts.
- If one of those GitHub governance files is added, removed, or materially changed in a way that should become the new shared baseline, ask whether to propagate the same baseline change across other GitHub repositories and push the updates, while still preserving required project-specific substitutions.

### Security And Review

- Review code with a bug-risk mindset first. Prioritize functional regressions, security issues, breaking changes, and missing tests before style or cleanup suggestions.
- If code returns `text/html` built from server-side string templates, HTML-escape all text fields from settings, persisted data, and user-controlled input before interpolating them into tags such as `<title>`, headings, attributes, or inline scripts.
- Do not assume only frontend `innerHTML` paths are XSS-relevant. Also inspect backend-rendered HTML, email templates, CMS fragments, and any raw string formatting that bypasses auto-escaping.
- For admin permission checks, prefer no-side-effect probes against real resources.
- Do not use invalid create requests to probe permissions; validation failures can mask the real authorization result and create misleading server logs.

### Dependency And Upgrade Rules

- Do not merge dependency or toolchain bumps just to clear security alerts or Dependabot PRs. First confirm the repo's current config is compatible and all required CI/build/test steps stay green.
- Treat build-tool upgrades such as `vite`, bundlers, editors, framework compilers, and test runners as compatibility work, not routine version bumps. If the upgrade breaks the build, defer it or patch it properly instead of merging a red PR.
- When a security alert applies only to dev tooling or to a runtime mode the project does not use, verify the real exposure before escalating. Distinguish "reported in the dependency graph" from "actually exploitable in this repo."

### Release Rules

- Rewrite stable release notes from the commits actually included by the published tag. Do not mix in changes that landed only on `main` after that tag.
- When converting prereleases into a stable release, aggregate the effective user-visible changes across the prerelease cycle instead of copying beta notes verbatim.
- If replacing or deleting an older release in favor of a newer one, compare the old tag, the new tag, and the default branch separately so unreleased work is not accidentally documented.
- Do not promote a prerelease to a stable `vX.Y.Z` release unless the user explicitly asks for that exact stable release.
- GitHub release titles should default to the bare tag name such as `v0.1.0` or `v0.1.0-beta.1`, not `WinRehome v0.1.0`, unless the user explicitly asks for a product-prefixed title.

## Repository-Specific Rules

This repository is `WinRehome`, a Windows-only migration backup desktop application.

### Project Summary

- 一个面向 Windows 的迁移备份工具原型，目标是在尽可能节省备份空间的前提下，保留真正有迁移价值的个人数据。
- Not a disk-image backup product.
- Goal: keep only data with real migration value while minimizing archive size.
- Installed applications are recorded as inventory, not backed up as installed binaries.
- Portable applications are detected as candidates, reviewed, and packed into the main archive.
- User files are included by allow-list and migration heuristics, not broad directory exclusion alone.

### Important Files

- `src/main.rs`: desktop app entry point
- `src/app.rs`: egui UI shell and workflow screens
- `src/models.rs`: shared domain models
- `src/plan.rs`: scan, classification, exclusion, and preview logic
- `src/archive.rs`: `.wrh` archive writing, manifest reading, and restore logic
- `src/config.rs`: persisted app configuration and saved selections
- `docs/architecture.md`: architecture and reliability notes
- `.github/workflows/cd.yaml`: GitHub Release build and asset upload workflow
- `.github/ISSUE_TEMPLATE/`: issue intake templates
- `SECURITY.md`: security reporting and support policy
- `README.md`: user-facing project page

### Repository Development Notes

- Build and validation commands:
  - `rtk cargo check`
  - `rtk cargo fmt`
  - `rtk cargo test`
  - `rtk cargo run`
- This repository is Windows-only. Validate file-system, registry, archive, and restore behavior on Windows semantics; do not assume Linux/macOS path behavior is relevant.
- If dependency fetches fail because of proxy settings in this environment, clear proxy variables before retrying `cargo` commands.
- Do not edit generated files under `target/`.
- Keep UI code in `src/app.rs`; keep scanning, archive, restore, and config behavior in focused modules.
- Keep Windows desktop builds GUI-only: preserve the Windows GUI subsystem entry-point behavior and keep a reliable Chinese-capable font fallback configured at startup so release binaries do not show an extra console window or tofu text.
- Prefer explicit helper names such as `is_known_noise` over vague utility naming.
- Keep restore selection persistence compatible with manual archive reloads; saved root selections should not be silently discarded when reopening the same archive.

### Backup Classification Rules

- Do not treat software under `Program Files`, `Program Files (x86)`, `WindowsApps`, or Windows system paths as portable backup content.
- Installed software detection should prefer registry uninstall metadata and other verifiable Windows sources.
- Portable-app detection must stay explainable. Favor confidence scoring and user review over opaque “smart” guesses.
- Default exclusions must cover cache, temp, logs, and build artifacts unless there is a documented restore value.
- When classification behavior changes, update both the implementation and `docs/architecture.md`.

### Restore Safety Rules

- Reliability is more important than compression ratio or scan aggressiveness.
- Archive changes must preserve validation and safe restore behavior.
- Do not introduce restore flows that overwrite user files silently.
- The current archive format is a single `.wrh` file with a manifest footer; keep format updates explicit and reviewable.
- Restore logic must continue to verify stored size and CRC before reporting success.
- Restore UX may offer category-level restore, but safety checks must stay the same regardless of scope.
- Restore UX should summarize the selected restore scope before execution, including at least selected roots and estimated file payload.
- Empty explicit restore-root selection means restore nothing; only the plain full-restore entrypoint should restore all manifest roots by default.
- Skip-existing restore behavior must remain opt-in and must report how many files were skipped.
- Restore must reject archive entry paths that attempt to escape the chosen destination root.
- If archive format behavior changes, document compatibility expectations in `docs/architecture.md`.

### Repository Release Conventions

- GitHub release packaging is handled by `.github/workflows/cd.yaml`.
- The release workflow is triggered by the `Release published` event.
- Release assets should include a Windows executable named like `WinRehome-vX.Y.Z-windows-x64.exe`.
- If release packaging changes, update both the workflow and this file in the same task.
