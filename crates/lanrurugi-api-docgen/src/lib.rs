//! Generate LANrurugi's OpenAPI 3.1 document from the server's own route table.
//!
//! The pipeline is five stages, each in its own module:
//!
//! 1. [`scan`] recovers the route table by parsing the API crate's source — axum's `Router` cannot
//!    be asked what was registered, so the source is the source of truth.
//! 2. [`legacy`] reads the vendored upstream LANraragi document, which supplies field-level
//!    documentation for every endpoint the two projects share.
//! 3. [`sidecar`] reads hand-written OpenAPI fragments for everything else.
//! 4. [`merge`] combines them, with the route scan applied last as the completeness authority.
//! 5. [`policy`] reads the route policy — the file the server itself enforces — so the document can
//!    state each endpoint's real authentication requirement.
//! 6. [`access_notes`] adds the per-role behaviour notes, which OpenAPI cannot express structurally.
//! 7. [`validate`] gates the result and reports coverage.
//!
//! [`emit`] assembles and serialises the document. See
//! `specs/011-api-docs-site/contracts/docgen-cli.md` for the command-line contract and
//! `specs/011-api-docs-site/contracts/openapi-document.md` for the output's shape.

pub mod access_notes;
pub mod emit;
pub mod legacy;
pub mod merge;
pub mod policy;
pub mod scan;
pub mod sidecar;
pub mod validate;
