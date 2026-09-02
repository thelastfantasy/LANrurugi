# RAR fixture provenance

These files are bundled test fixtures from the [`unrar`](https://crates.io/crates/unrar) crate
(v0.5.8, MIT/Apache-2.0), copied verbatim from its `data/` directory. No FOSS RAR-writer exists
(proprietary format), so this project reuses these pre-made, redistribution-safe fixtures instead
of generating new ones — see `specs/003-ui-test-automation/research.md` §6.

| File | Source path in `unrar` v0.5.8 |
|---|---|
| `crypted.rar` | `data/crypted.rar` |
| `archive.part1.rar` | `data/archive.part1.rar` |
| `100M.part00002.rar` | `data/100M.part00002.rar` |
| `unicode.rar` | `data/unicode.rar` |
| `unicodefilename❤️.rar` | `data/unicodefilename❤️.rar` |

Note: neither bundled fixture contains genuine CJK filenames (`unicode.rar`'s entry is
Latin/symbol/emoji text; `unicodefilename❤️.rar`'s *archive* filename has an emoji but its one
internal entry is a plain `.gitignore`). Extending the CJK-mojibake regression (data-model.md
Regression Fixture #6) to a project-built RAR fixture would require a personally-licensed RAR tool
this implementation environment doesn't have — see research.md §6 and tasks.md T041's own note.
