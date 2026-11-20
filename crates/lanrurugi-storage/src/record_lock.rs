//! One writer at a time per record (#302).
//!
//! A read-modify-write that read its snapshot before another write landed puts the old values
//! back when it writes. Holding this lock across "read the current record, change it, write it"
//! makes every such sequence see what the previous one wrote. It is per process: this server is
//! the only writer of these records.
//!
//! Take it only around that short read-change-write step. Slow work (a plugin's network call, a
//! subscription's paging) belongs before it, or every other writer of the record waits on it.

use std::sync::LazyLock;

use lockable::LockPool;

/// Live locks by record key; `lockable` drops an entry once nobody holds or waits for it.
static LOCKS: LazyLock<LockPool<String>> = LazyLock::new(LockPool::new);

/// Waits for, then holds, the lock for `key` (for example `archive:<id>`) until the returned guard
/// drops.
pub async fn lock(key: &str) -> impl Send + 'static {
    LOCKS.async_lock(key.to_string()).await
}

#[cfg(test)]
mod tests {
    use super::lock;

    #[tokio::test]
    async fn writers_of_one_record_take_turns_and_others_do_not_wait() {
        let first = lock("archive:a").await;
        // Another record is not blocked.
        let _other = tokio::time::timeout(std::time::Duration::from_millis(50), lock("archive:b"))
            .await
            .expect("a different record must not wait");
        // The same record is.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), lock("archive:a"))
                .await
                .is_err()
        );
        drop(first);
        let _second = tokio::time::timeout(std::time::Duration::from_millis(50), lock("archive:a"))
            .await
            .expect("released, so the next writer goes");
    }
}
