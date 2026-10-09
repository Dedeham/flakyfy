use super::*;

/// The shim, as a table: what /facts and /search answer, and what was asked.
#[derive(Default)]
struct FakeShim {
    facts: HashMap<String, StreamFacts>,
    hits: HashMap<String, YouTubeHit>,
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl StreamFactsSource for FakeShim {
    async fn facts(&self, video_id: &str) -> Option<StreamFacts> {
        self.asked.lock().push(format!("facts {video_id}"));
        self.facts.get(video_id).cloned()
    }

    async fn search(&self, query: &str, duration_hint: Option<i32>) -> Option<YouTubeHit> {
        self.asked
            .lock()
            .push(format!("search {query} {duration_hint:?}"));
        self.hits.get(query).cloned()
    }
}

fn song(artist: &str, title: &str) -> SoulseekRouting {
    SoulseekRouting {
        kind: RoutingKind::Song,
        artist: Some(artist.into()),
        title: Some(title.into()),
        duration: Some(417),
        ..Default::default()
    }
}

fn sized(size: u64) -> StreamFacts {
    StreamFacts {
        size: Some(size),
        itag: Some("140".into()),
        ext: Some("m4a".into()),
        bit_rate: Some(129),
        ..Default::default()
    }
}

fn warmer_over(shim: FakeShim) -> (Arc<ExternalIdRegistry>, Arc<FakeShim>, StreamSizeWarmer) {
    let registry = Arc::new(ExternalIdRegistry::in_memory());
    let shim = Arc::new(shim);
    let warmer = StreamSizeWarmer::new(registry.clone(), shim.clone());
    (registry, shim, warmer)
}

#[test]
fn the_rate_window_lets_thirty_a_minute_start() {
    let t0 = Instant::now();
    let mut window = RateWindow::new(PER_MINUTE);
    for i in 0..PER_MINUTE {
        assert_eq!(
            window.wait_before_next(t0 + Duration::from_millis(i as u64)),
            None,
            "start {i}"
        );
    }
    // The 31st in the same minute waits until the first start is a minute old.
    let at = t0 + Duration::from_secs(10);
    assert_eq!(window.wait_before_next(at), Some(Duration::from_secs(50)));
    assert_eq!(window.wait_before_next(t0 + Duration::from_secs(60)), None);
}

#[tokio::test]
async fn a_pinned_song_is_sized_from_its_videos_facts() {
    let mut shim = FakeShim::default();
    shim.facts.insert("vid-a".into(), sized(6_755_585));
    let (registry, shim, warmer) = warmer_over(shim);
    let id = registry.register(song("Daft Punk", "Emotion"));
    registry.set_you_tube_id(&id, "vid-a");

    assert!(warmer.want(&id));
    warmer.warm_one(&id).await;

    assert_eq!(registry.stream_facts(&id), Some(sized(6_755_585)));
    assert_eq!(*shim.asked.lock(), vec!["facts vid-a".to_string()]);
}

#[tokio::test]
async fn an_unpinned_song_gets_the_video_a_play_would_pick_and_its_size() {
    let mut shim = FakeShim::default();
    shim.hits.insert(
        "Daft Punk Emotion".into(),
        YouTubeHit {
            video_id: "vid-b".into(),
            duration: Some(416),
            stream: sized(7_000_000),
            ..Default::default()
        },
    );
    let (registry, shim, warmer) = warmer_over(shim);
    let id = registry.register(song("Daft Punk", "Emotion"));

    warmer.warm_one(&id).await;

    let r = registry.lookup(&id).expect("known").snapshot();
    assert_eq!(r.you_tube_id.as_deref(), Some("vid-b"));
    assert_eq!(r.duration, Some(416));
    assert_eq!(registry.stream_facts(&id).and_then(|f| f.size), Some(7_000_000));
    // The same query and length hint the play path searches with.
    assert_eq!(
        *shim.asked.lock(),
        vec!["search Daft Punk Emotion Some(417)".to_string()]
    );
}

#[tokio::test]
async fn sized_unknown_and_recently_tried_songs_are_not_queued_twice() {
    let (registry, _shim, warmer) = warmer_over(FakeShim::default());
    let id = registry.register(song("Daft Punk", "Emotion"));
    assert!(warmer.want(&id));
    // Already waiting.
    assert!(!warmer.want(&id));
    // Tried and given no answer: left alone for an hour.
    warmer.done(&id);
    assert!(!warmer.want(&id));

    let sized_id = registry.register(song("Justice", "Genesis"));
    registry.set_you_tube_id(&sized_id, "vid-c");
    registry.remember_stream(&sized_id, "vid-c", &sized(1));
    assert!(!warmer.want(&sized_id));
    assert!(!warmer.want("unknown"));
    let artist = registry.register(SoulseekRouting {
        kind: RoutingKind::Artist,
        artist: Some("Justice".into()),
        ..Default::default()
    });
    assert!(!warmer.want(&artist));
}

#[test]
fn a_full_queue_drops_new_rows() {
    let (registry, _shim, warmer) = warmer_over(FakeShim::default());
    for i in 0..QUEUE_CAPACITY {
        let id = registry.register(song("Artist", &format!("Song {i}")));
        assert!(warmer.want(&id), "row {i}");
    }
    let late = registry.register(song("Artist", "One too many"));
    assert!(!warmer.want(&late));
    // Dropped, not remembered: it can be queued again once there is room.
    assert!(!warmer.queued.lock().contains(&late));
}

#[tokio::test]
async fn the_worker_sizes_what_was_queued_and_stops_on_shutdown() {
    let mut shim = FakeShim::default();
    shim.facts.insert("vid-a".into(), sized(42));
    let (registry, _shim, warmer) = warmer_over(shim);
    let warmer = Arc::new(warmer);
    let id = registry.register(song("Daft Punk", "Emotion"));
    registry.set_you_tube_id(&id, "vid-a");
    warmer.want(&id);

    let token = CancellationToken::new();
    let worker = tokio::spawn(warmer.clone().run(token.clone()));
    tokio::time::timeout(Duration::from_secs(10), async {
        while registry.stream_facts(&id).is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("sized");
    token.cancel();
    worker.await.expect("joins").expect("ends cleanly");
}

#[tokio::test]
async fn a_played_song_asks_for_its_measured_loudness_even_with_its_size_known() {
    let mut shim = FakeShim::default();
    shim.facts.insert(
        "vid-a".into(),
        StreamFacts {
            track_gain: Some(-10.0),
            track_peak: Some(1.135011),
            ..sized(6_755_585)
        },
    );
    let (registry, shim, warmer) = warmer_over(shim);
    let id = registry.register(song("Daft Punk", "Emotion"));
    registry.set_you_tube_id(&id, "vid-a");
    registry.remember_stream(&id, "vid-a", &sized(6_755_585));

    // Sized: a listing alone asks nothing.
    assert!(!warmer.want(&id));
    assert!(warmer.want_loudness(&id));
    warmer.warm_one(&id).await;

    let facts = registry.stream_facts(&id).expect("facts");
    assert_eq!(
        (facts.track_gain, facts.track_peak),
        (Some(-10.0), Some(1.135011))
    );
    assert_eq!(*shim.asked.lock(), vec!["facts vid-a".to_string()]);
    // Known now: a later play asks nothing more.
    warmer.done(&id);
    assert!(!warmer.want_loudness(&id));
}

#[test]
fn a_song_with_no_pinned_video_has_no_loudness_to_ask_for() {
    let (registry, _shim, warmer) = warmer_over(FakeShim::default());
    let id = registry.register(song("Daft Punk", "Emotion"));
    assert!(!warmer.want_loudness(&id));
    assert!(!warmer.want_loudness("unknown"));
}
