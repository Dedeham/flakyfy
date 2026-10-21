//! `api/admin/precache` (Play instantly on the dashboard's Mixes page): the lists the pre-cache
//! could keep ready, which of them are picked, and what it has fetched.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;
use serde_json::json;

use super::request_binding::{error, ok, split, utc_opt};
use crate::app::AppState;
use crate::http::routes::RouteSet;
use crate::services::library::playlist_precache_worker::ListInfo;
use crate::services::library::precache_store::Pick;

/// Picks past this are ignored: no household has this many lists worth keeping ready.
const MAX_PICKS: usize = 200;

pub fn routes() -> RouteSet {
    RouteSet::new().route("/api/admin/precache", get(get_precache).put(put_precache))
}

async fn get_precache(State(state): State<AppState>) -> Response {
    ok(&status(&state))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PutBody {
    picks: Vec<PickIn>,
}

#[derive(Deserialize)]
struct PickIn {
    owner: String,
    key: String,
    count: Option<i32>,
}

/// Replaces the picks. Lists that do not exist are left out rather than failing the save, since a
/// list that is not made yet is simply not there to pick.
async fn put_precache(State(state): State<AppState>, request: Request) -> Response {
    let (_, body) = split(request).await;
    let body: PutBody = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "The request body is not the picks to save.",
            );
        }
    };
    let known = state.playlist_precache_worker.world().lists();
    let picks = apply_picks(&known, body.picks);
    state.playlist_precache_worker.store().update(|s| s.picks = picks);
    ok(&status(&state))
}

/// The picks to keep: those naming a list that exists, once each, with a count in range.
fn apply_picks(known: &[ListInfo], incoming: Vec<PickIn>) -> Vec<Pick> {
    let mut picks: Vec<Pick> = Vec::new();
    for pick in incoming.into_iter().take(MAX_PICKS) {
        let owner = pick.owner.trim().to_lowercase();
        let exists = known
            .iter()
            .any(|list| list.owner == owner && list.key == pick.key);
        let twice = picks
            .iter()
            .any(|kept| kept.owner == owner && kept.key == pick.key);
        if exists && !twice {
            picks.push(Pick {
                owner,
                key: pick.key,
                count: pick.count.map(|n| n.clamp(1, 50)),
            });
        }
    }
    picks
}

fn status(state: &AppState) -> serde_json::Value {
    let settings = state.settings.current().generated_playlists.clone();
    let worker = &state.playlist_precache_worker;
    let known = worker.world().lists();
    let saved = worker.store().read(|s| s.clone());
    let lists: Vec<_> = known
        .iter()
        .map(|list| {
            let pick = saved
                .picks
                .iter()
                .find(|pick| pick.owner == list.owner && pick.key == list.key);
            let ready = saved
                .cached
                .iter()
                .filter(|c| c.owner == list.owner && c.list == list.key)
                .count();
            json!({
                "owner": list.owner,
                "key": list.key,
                "label": list.label,
                "kind": list.kind,
                "outside": list.outside,
                "picked": pick.is_some(),
                "count": pick.and_then(|pick| pick.count),
                "ready": ready,
            })
        })
        .collect();
    let cached: Vec<_> = saved
        .cached
        .iter()
        .map(|c| {
            json!({
                "owner": c.owner,
                "list": c.list,
                "artist": c.artist,
                "title": c.title,
                "cachedUtc": super::request_binding::utc(&c.cached_utc),
                "note": c.note,
            })
        })
        .collect();
    json!({
        "enabled": settings.precache,
        "dryRun": settings.precache_dry_run,
        "defaultCount": settings.effective_precache_count(),
        "keepDays": settings.effective_precache_keep_days(),
        "lists": lists,
        "cached": cached,
        "lastFillUtc": utc_opt(&saved.last_fill_utc),
        "lastExpiryUtc": utc_opt(&saved.last_expiry_utc),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(owner: &str, key: &str) -> ListInfo {
        ListInfo {
            owner: owner.into(),
            key: key.into(),
            label: key.into(),
            kind: "Made for you",
            outside: 10,
        }
    }

    fn pick(owner: &str, key: &str, count: Option<i32>) -> PickIn {
        PickIn {
            owner: owner.into(),
            key: key.into(),
            count,
        }
    }

    #[test]
    fn apply_picks_keeps_only_lists_that_exist() {
        let known = [list("jamie", "for:newreleases")];
        let kept = apply_picks(
            &known,
            vec![
                pick("jamie", "for:newreleases", None),
                pick("jamie", "for:nothing", None),
            ],
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].key, "for:newreleases");
    }

    #[test]
    fn apply_picks_reads_the_owner_without_case_or_spaces_and_drops_a_repeat() {
        let known = [list("jamie", "for:newreleases")];
        let kept = apply_picks(
            &known,
            vec![
                pick(" Jamie ", "for:newreleases", None),
                pick("jamie", "for:newreleases", Some(9)),
            ],
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].owner, "jamie");
        assert_eq!(kept[0].count, None);
    }

    #[test]
    fn apply_picks_puts_a_count_in_range() {
        let known = [list("jamie", "a"), list("jamie", "b")];
        let kept = apply_picks(
            &known,
            vec![pick("jamie", "a", Some(0)), pick("jamie", "b", Some(900))],
        );
        assert_eq!(kept[0].count, Some(1));
        assert_eq!(kept[1].count, Some(50));
    }
}
