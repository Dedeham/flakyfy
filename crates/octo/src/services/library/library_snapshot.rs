//! Port of `Services/Library/LibrarySnapshot.cs` (f49fb80).

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, TimeDelta, Utc};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use octo_core::common::clock::Clock;
use parking_lot::Mutex;
use tracing::debug;

use super::album_ownership::LibraryAlbumIndex;
use super::navidrome_playlist_api::NavidromePlaylistApi;
use super::quality_upgrade_worker::LibrarySongRow;

/// One page of the library, with how many rows Navidrome's page held; None when Navidrome could
/// not be read. Tests answer from a list.
pub type ReadPage =
    Arc<dyn Fn(i32, i32) -> BoxFuture<'static, Option<(Vec<LibrarySongRow>, usize)>> + Send + Sync>;

type Reading = Shared<BoxFuture<'static, Option<Arc<LibraryAlbumIndex>>>>;

#[derive(Default)]
struct State {
    current: Option<(Arc<LibraryAlbumIndex>, DateTime<Utc>)>,
    reading: Option<Reading>,
    failed_at: Option<DateTime<Utc>>,
}

struct Inner {
    read_page: ReadPage,
    clock: Clock,
    state: Mutex<State>,
}

/// The whole library, read through Navidrome's own song list and kept, for the questions a
/// search asks of every row at once (which outside albums the library already holds). Reading it
/// is a few large requests, so it is never done on a search's own time once there is a copy: a
/// copy older than [`FRESH_FOR`](Self::FRESH_FOR) is still answered with while a new one is read,
/// and only one read runs at a time. Needs the Navidrome admin sign-in, as Spotify import does;
/// with none the answer is None and searches go out as before.
#[derive(Clone)]
pub struct LibrarySnapshot {
    inner: Arc<Inner>,
}

impl LibrarySnapshot {
    /// A copy this young is answered with as it is.
    pub const FRESH_FOR: TimeDelta = TimeDelta::minutes(5);

    /// A copy older than this is not answered with at all: the next caller waits for a read.
    pub const USABLE_FOR: TimeDelta = TimeDelta::hours(1);

    /// After a read that failed, how long callers are answered "unknown" before another is tried.
    pub const RETRY_AFTER: TimeDelta = TimeDelta::minutes(1);

    const PAGE_SIZE: i32 = 1000;

    /// The library read through Navidrome's native song list, as the admin.
    pub fn new(navidrome: Arc<NavidromePlaylistApi>) -> Self {
        Self::with(
            Arc::new(move |start, count| {
                let navidrome = Arc::clone(&navidrome);
                Box::pin(async move { navidrome.list_songs(start, count).await })
            }),
            Clock::system(),
        )
    }

    /// Pages from `read_page` and the time from `clock` (the C# test seams).
    pub fn with(read_page: ReadPage, clock: Clock) -> Self {
        Self {
            inner: Arc::new(Inner {
                read_page,
                clock,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// The library as last read, reading it first when there is no usable copy.
    pub async fn current(&self) -> Option<Arc<LibraryAlbumIndex>> {
        let reading = {
            let mut state = self.inner.state.lock();
            let now = self.inner.clock.now();
            if let Some((index, at)) = &state.current {
                let age = now - *at;
                if age < Self::FRESH_FOR {
                    return Some(Arc::clone(index));
                }
                if age < Self::USABLE_FOR {
                    let index = Arc::clone(index);
                    if state.reading.is_none() {
                        state.reading = Some(self.start_read());
                    }
                    return Some(index);
                }
            }
            if state.reading.is_none()
                && let Some(failed) = state.failed_at
                && now - failed < Self::RETRY_AFTER
            {
                return None;
            }
            match &state.reading {
                Some(reading) => reading.clone(),
                None => {
                    let reading = self.start_read();
                    state.reading = Some(reading.clone());
                    reading
                }
            }
        };
        reading.await
    }

    /// When the copy answered with now was read, or None before the first read.
    pub fn read_at_utc(&self) -> Option<DateTime<Utc>> {
        self.inner.state.lock().current.as_ref().map(|(_, at)| *at)
    }

    /// A copy read from now on, for a caller that knows the library changed (Library health's
    /// "Check again", after its own fixes). A read already running is joined rather than doubled.
    pub async fn refresh(&self) -> Option<Arc<LibraryAlbumIndex>> {
        let reading = {
            let mut state = self.inner.state.lock();
            state.failed_at = None;
            match &state.reading {
                Some(reading) => reading.clone(),
                None => {
                    let reading = self.start_read();
                    state.reading = Some(reading.clone());
                    reading
                }
            }
        };
        reading.await
    }

    /// A read on a task of its own and without any caller: one caller giving up does not stop
    /// a read every later caller will use. Called with the state locked, so the read cannot
    /// finish (and clear `reading`) before it is stored.
    fn start_read(&self) -> Reading {
        let inner = Arc::clone(&self.inner);
        let task = tokio::spawn(async move {
            let _done = ReadDone(Arc::clone(&inner));
            read(&inner).await
        });
        task.map(Result::unwrap_or_default).boxed().shared()
    }
}

/// Lets go of the read when it ends, however it ends.
struct ReadDone(Arc<Inner>);

impl Drop for ReadDone {
    fn drop(&mut self) {
        self.0.state.lock().reading = None;
    }
}

async fn read(inner: &Inner) -> Option<Arc<LibraryAlbumIndex>> {
    let clock = Instant::now();
    let mut rows: Vec<LibrarySongRow> = Vec::new();
    let mut start = 0;
    loop {
        let Some((page, count)) = (inner.read_page)(start, LibrarySnapshot::PAGE_SIZE).await else {
            debug!("Could not read the library for album ownership");
            inner.state.lock().failed_at = Some(inner.clock.now());
            return None;
        };
        rows.extend(page);
        if (count as i32) < LibrarySnapshot::PAGE_SIZE {
            break;
        }
        start += LibrarySnapshot::PAGE_SIZE;
    }
    let read = rows.len();
    let index = Arc::new(LibraryAlbumIndex::new(rows));
    {
        let mut state = inner.state.lock();
        state.current = Some((Arc::clone(&index), inner.clock.now()));
        state.failed_at = None;
    }
    debug!(
        "Read {read} library songs for album ownership in {} ms",
        clock.elapsed().as_millis()
    );
    Some(index)
}
