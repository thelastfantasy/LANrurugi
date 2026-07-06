# LANrurugi

A from-scratch Rust + React rewrite of [LANraragi](https://github.com/Difegue/LANraragi), a
self-hosted manga/doujinshi library manager. Aims for feature parity with, and full data/API
compatibility with, existing LANraragi installations, while fixing a known duplicate-detection
defect and adding genuine multi-core concurrency where the legacy Perl implementation had none.

**Status**: Planning/design phase — no implementation yet. The full spec-kit planning artifacts
(specifications, technical plans, research decisions, and task breakdowns) live under
[`specs/`](./specs/):

- [`specs/001-lanrurugi-full-rewrite/`](./specs/001-lanrurugi-full-rewrite/) — Phase 1: library
  continuity, non-merging ingestion, third-party API compatibility, plugin metadata enrichment,
  backup/export, duplicate repair, UI localization, and a concurrency benchmark against the
  legacy system.
- [`specs/002-ocr-manga-translation/`](./specs/002-ocr-manga-translation/) — Phase 2 (depends on
  Phase 1, does not block it): optional on-page manga translation via OCR detection/recognition,
  a user-selectable translation backend (cloud or locally-hosted), and volume-level font matching.

Project governance, architectural principles, and technology stack decisions are recorded in the
[project constitution](./.specify/memory/constitution.md).

## License

MIT — see [LICENSE](./LICENSE).
