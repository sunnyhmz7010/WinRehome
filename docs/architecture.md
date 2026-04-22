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
- High-value user data roots
- Portable application candidate discovery from curated search roots
- Desktop preview UI

### Deliberately deferred

- Full-disk intelligent scanning
- Custom archive writer
- Incremental block deduplication
- Restore UI
- Shadow copy support for locked files

## Archive design target

- One archive file per snapshot
- Metadata header plus append-only chunk table
- Strong checksums per chunk
- Zstd compression as default
- Optional future content-defined chunking for deduplication

## Why `eframe/egui` first

- Single binary deployment
- No WebView2 runtime requirement
- Fast iteration while the rules engine is still moving
- Easy later split between UI and archive core
