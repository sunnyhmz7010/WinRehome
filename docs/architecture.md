# WinRehome Architecture Notes

## Primary goal

Build a Windows migration backup tool that minimizes backup size without losing high-value user data.

## Reliability principles

1. Never classify installable software payloads as portable backup content without a confidence score and user review.
2. Prefer positive inclusion rules for user data over broad "exclude system directories" logic.
3. Keep archive writes transactional and verifiable before deleting old snapshots.
4. Separate discovery, planning, archive writing, and restore into isolated stages.

## MVP boundaries

### Included in MVP

- Installed software inventory from registry uninstall keys
- High-value user data roots with default recommendations
- Portable application candidate discovery from curated search roots, including single executable candidates
- Desktop preview UI with reviewable backup-plan estimates
- Single-file `.wrh` archive writing with manifest validation
- `.wrh` archive verification before restore with size and CRC checks
- Restore to a target directory with category toggles and root-level selection
- Restore conflict handling with fail-on-conflict by default and explicit skip-existing mode
- Local config persistence for saved selections, backup output directory, and restore targets
- Recent archive discovery for faster reopen and retry flows

### Deliberately deferred

- Full-disk intelligent scanning
- Incremental block deduplication
- Shadow copy support for locked files
- In-place merge or overwrite policies beyond the current explicit fail-or-skip behavior

## Archive design target

- One archive file per snapshot
- `WRH1` header plus manifest footer
- Archive reads validate the manifest format version before restore or verify continues
- Per-file metadata with source path, archive path, offsets, sizes, and CRC32
- Deflate compression per file in the current prototype
- Restore selection is driven by manifest root prefixes instead of per-file UI state
- Optional future content-defined chunking for deduplication

## Why `eframe/egui` first

- Single binary deployment
- No WebView2 runtime requirement
- Fast iteration while the rules engine is still moving
- Easy later split between UI and archive core
