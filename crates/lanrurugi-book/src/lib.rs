//! E-book support for the archive layer: PDF, EPUB, MOBI/AZW3, DjVu, FB2 and TXT books are listed
//! and read like any other archive, one image per page. Parsing and rendering run in
//! `lanrurugi-book-worker` subprocesses; this crate is format detection, the protocol, and the
//! synchronous pooled client the page functions in `lanrurugi_scanner::archive_format` call.

mod client;
pub mod format;
pub mod protocol;

pub use client::{configure, inspect, read_page, BookConfig, BookError, DEFAULT_RENDER_WIDTH};
pub use format::{BookFormat, TXT_MIN_BYTES};
pub use protocol::{
    page_entry_name, parse_page_entry_name, BookInfo, ImageKind, OutlineItem, PageSource,
};
