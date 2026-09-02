//! Redis key names for the search index (logical DB `DB_SEARCH`), shared between `indexer`
//! (which writes them) and `engine` (which reads them) so the two can't drift apart on a key name.

/// Sorted set of `"<lowercased title>\x00<archive id>"` members, score 0 — legacy's own
/// alphabetical title index (`Utils/Database.pm::add_to_indexes`'s `LRR_TITLES` zset).
pub const TITLES_KEY: &str = "LRR_TITLES";

/// Set of archive IDs with no tags at all.
pub const UNTAGGED_KEY: &str = "LRR_UNTAGGED";

/// Set of archive IDs still marked "new" (unread since being added).
pub const NEW_KEY: &str = "LRR_NEW";

/// Set of ids eligible to appear standalone in a `groupby_tanks=true` search (the default) —
/// despite the legacy-inherited name, membership means "not *currently folded into* a tank
/// display," not "belongs to a tankoubon." Every freshly-catalogued archive starts a member
/// (`index_new_archive`, confirmed by that function's own test naming this same check
/// `ungrouped`); a Tankoubon's member archives are removed from this set for as long as they stay
/// members (so only the tank's own aggregate entry shows up in a grouped search, not both it and
/// every member individually), via `sync_tank_membership`. The Tankoubon's own id is added once,
/// unconditionally, at creation (`add_tank_to_index`) and removed only on outright deletion
/// (`remove_tank_from_index`) — deliberately *not* re-derived from its current member count, so an
/// emptied-out tank stays visible (with zero pages) instead of silently vanishing from the default
/// view with no discoverable path back to repopulate or delete it.
pub const TANKGROUPED_KEY: &str = "LRR_TANKGROUPED";
