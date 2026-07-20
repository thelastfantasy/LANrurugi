//! Redis key names for the search index (logical DB `DB_SEARCH`), shared between `indexer`
//! (which writes them) and `engine` (which reads them) so the two can't drift apart on a key name.

/// Sorted set of `"<lowercased title>\x00<archive id>"` members, score 0 — legacy's own
/// alphabetical title index (`Utils/Database.pm::add_to_indexes`'s `LRR_TITLES` zset).
pub const TITLES_KEY: &str = "LRR_TITLES";

/// Set of archive IDs with no tags at all.
pub const UNTAGGED_KEY: &str = "LRR_UNTAGGED";

/// Set of archive IDs still marked "new" (unread since being added).
pub const NEW_KEY: &str = "LRR_NEW";

/// Set of archive IDs that belong to a tankoubon grouping.
pub const TANKGROUPED_KEY: &str = "LRR_TANKGROUPED";
