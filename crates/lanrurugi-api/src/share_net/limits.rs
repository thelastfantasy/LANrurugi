//! Sharing-side upload caps: the engine-side half, parameterised by a settings view and a zone.

use chrono_tz::Tz;

use lanrurugi_share::settings::ShareSettings;

use crate::AppState;

/// The current mode and caps, read from storage.
pub async fn current(state: &AppState) -> (ModeStatus, Caps, Caps) {
    let settings = crate::share_settings::load(state).await;
    current_with(&settings, timezone(state).await)
}

pub fn current_with(settings: &ShareSettings, tz: Tz) -> (ModeStatus, Caps, Caps) {
    lanrurugi_share::limits::current(settings, tz)
}

/// Admits one upload to `peer` under the group's current caps.
pub async fn admit(state: &AppState, group: Group, peer: &str) -> Result<Caps, Held> {
    use lanrurugi_share::host::ShareHost;
    state.admit_upload(group, peer.to_string()).await
}

pub use lanrurugi_share::limits::{throttle, Caps, Group, Held, ModeStatus};

/// The configured zone, or UTC when unset/unreadable.
pub async fn timezone(state: &AppState) -> Tz {
    use deadpool_redis::redis::AsyncCommands;
    let name: Option<String> = match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget(lanrurugi_storage::keys::CONFIG_KEY, "timezone")
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };
    name.and_then(|n| n.parse().ok()).unwrap_or(chrono_tz::UTC)
}
