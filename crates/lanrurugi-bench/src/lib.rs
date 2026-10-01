//! US8 concurrency/performance benchmark harness (research.md §11). Lives outside `crates/`
//! (plan.md's Project Structure) because it orchestrates *two independent systems* — the legacy
//! LANraragi Perl instance and this rewrite's binary — side by side, which is an external
//! comparison concern rather than a single crate's unit tests. Exposed as a library so both the
//! standalone `lanrurugi-bench-compare` binary and `lanrurugi serve`'s `POST /bench/run` /
//! `lanrurugi bench` CLI paths (T089/T090) share one implementation.

pub mod compare;
pub mod synthetic_library;
