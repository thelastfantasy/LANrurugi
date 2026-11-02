//! The categorized log files legacy's Settings → Logs page reads (`Utils/Logging.pm`'s five
//! `get_logger($name, $logfile)` call sites, `Controller/Logging.pm`'s five `print_*` actions) —
//! shared between `lanrurugi_server::telemetry` (writes them) and `lanrurugi_api::logs` (serves
//! them back over `GET /logs/{category}`), so the two can't drift out of sync.

pub const CATEGORIES: &[&str] = &["general", "shinobu", "plugins", "redis", "http"];
