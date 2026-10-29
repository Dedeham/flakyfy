//! Measures how often Octo's wrong-song checks decide right on a labelled set of files: the
//! YouTube preview check (with the Review sweep's auto-replace rule) and the Better quality
//! same-recording check. The files, cases and results stay in a bench folder outside git;
//! `scripts/verify-bench/` makes them and its README.md describes every file.
//!
//! The only step that needs the network is `collect`: it asks the shim for each song's preview
//! the way a check does and keeps what it got. `replay` then runs the real checks against those
//! previews with no network at all, as often as needed and after any code change.
//!
//! ```text
//! verify_bench collect --bench DIR [--shim http://127.0.0.1:18080] [--delay-ms 1500]
//! verify_bench replay  --bench DIR [--only preview|same_recording] [--jobs 3]
//! ```
//!
//! `cases.json` is `{"cases": [...]}`, paths relative to the bench folder:
//! - `{"id", "kind": "preview", "group", "song_id", "request_key", "file", "artist", "title",
//!   "album", "seconds", "expect"}`, `expect` being `match` or `not_match`.
//! - `{"id", "kind": "same_recording", "group", "song_id", "candidate", "original", "expect"}`,
//!   `expect` being `same` or `refused`.
//!
//! Each case comes out `pass`, `soft` (no harm done, but the check did not help: an unclear preview
//! or a missed upgrade) or `hard` (a right file refused or replaced, a wrong file let through).
//! `replay` exits with 1 when any case is `hard`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use async_trait::async_trait;
use futures::StreamExt;
use octo::services::fingerprint::preview_check::{
    self, Heard, PreviewFacts, PreviewFailure, PreviewOutcome, PreviewRequest, PreviewSource,
};
use octo::services::fingerprint::same_recording::{self, SameRecording};
use octo::services::library::library_review_sweep_worker::LibraryReviewSweepWorker;
use octo::services::you_tube::you_tube_resolver::{Failure, YouTubeResolver};
use octo_core::settings::SoulseekSettings;
use octo_media::audio::AudioFingerprinter;
use octo_media::tags::TagFile;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// How far same_recording slides one fingerprint against the other (its private `MAX_SHIFT`).
const MAX_SHIFT: usize = 100;

// ----- input -----

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Request {
    key: String,
    artist: String,
    title: String,
    #[serde(default)]
    album: String,
    seconds: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct Requests {
    requests: Vec<Request>,
}

#[derive(Debug, Deserialize)]
struct CaseFile {
    cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Case {
    Preview {
        id: String,
        #[serde(default)]
        group: String,
        #[serde(default)]
        song_id: String,
        request_key: String,
        file: PathBuf,
        artist: String,
        title: String,
        #[serde(default)]
        album: String,
        seconds: Option<i32>,
        expect: Expect,
    },
    SameRecording {
        id: String,
        #[serde(default)]
        group: String,
        #[serde(default)]
        song_id: String,
        candidate: PathBuf,
        original: PathBuf,
        expect: Expect,
    },
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Expect {
    Match,
    NotMatch,
    Same,
    Refused,
}

impl Expect {
    fn name(self) -> &'static str {
        match self {
            Expect::Match => "match",
            Expect::NotMatch => "not_match",
            Expect::Same => "same",
            Expect::Refused => "refused",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Grade {
    Pass,
    Soft,
    Hard,
}

impl Grade {
    fn name(self) -> &'static str {
        match self {
            Grade::Pass => "pass",
            Grade::Soft => "soft",
            Grade::Hard => "hard",
        }
    }
}

// ----- what collect keeps -----

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Facts {
    video_id: String,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    seconds: Option<i32>,
    /// `row` when YouTube Music's row for the song was taken, `search` for yt-dlp's search. A row
    /// credits the artist asked for and names an album; a search hit names its channel.
    lane: String,
}

impl Facts {
    fn of(facts: &PreviewFacts, request: &Request) -> Self {
        let row = facts.album.is_some() || facts.artist.as_deref() == Some(request.artist.as_str());
        Facts {
            video_id: facts.video_id.clone(),
            title: facts.title.clone(),
            artist: facts.artist.clone(),
            album: facts.album.clone(),
            seconds: facts.seconds,
            lane: if row { "row" } else { "search" }.to_string(),
        }
    }

    fn preview(&self) -> PreviewFacts {
        PreviewFacts {
            video_id: self.video_id.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            seconds: self.seconds,
            album: self.album.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    key: String,
    request: Request,
    /// `found`, `none` or `failed`.
    found: String,
    why: Option<String>,
    facts: Option<Facts>,
    /// The preview's audio, a file name next to this record.
    audio: Option<String>,
    /// `ok`, `not_sent` or `busy`.
    fetch: Option<String>,
    seconds_taken: f64,
    at: String,
}

// ----- options -----

struct Options {
    command: String,
    bench: PathBuf,
    shim: String,
    delay: Duration,
    only: Option<String>,
    jobs: usize,
}

fn options() -> anyhow::Result<Options> {
    let mut args = std::env::args().skip(1);
    let command = args.next().context("collect or replay")?;
    if command != "collect" && command != "replay" {
        bail!("unknown command {command}");
    }
    let mut options = Options {
        command,
        bench: PathBuf::from("."),
        shim: "http://127.0.0.1:18080".to_string(),
        delay: Duration::from_millis(1500),
        only: None,
        jobs: 3,
    };
    while let Some(flag) = args.next() {
        let mut value = || args.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--bench" => options.bench = PathBuf::from(value()?),
            "--shim" => options.shim = value()?,
            "--delay-ms" => options.delay = Duration::from_millis(value()?.parse()?),
            "--only" => options.only = Some(value()?),
            "--jobs" => options.jobs = value()?.parse::<usize>()?.clamp(1, 8),
            other => bail!("unknown flag {other}"),
        }
    }
    // Absolute, since Octo runs fpcalc and ffmpeg from the temp folder.
    options.bench = std::path::absolute(&options.bench)?;
    Ok(options)
}

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn sha256_of(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| hex::encode(Sha256::digest(&bytes)))
        .unwrap_or_default()
}

fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path).with_context(|| format!("writing {}", path.display()))
}

// ----- collect -----

async fn collect(options: &Options) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(options.bench.join("requests.json")).context("requests.json")?;
    let requests: Requests = serde_json::from_str(&text).context("requests.json")?;
    let previews = options.bench.join("previews");
    std::fs::create_dir_all(&previews)?;
    let n = std::fs::read_dir(&previews)?
        .filter(|entry| entry.is_ok())
        .count()
        + 1;
    let collection = format!("c{n}-{}", chrono::Utc::now().format("%Y%m%dT%H%M%SZ"));
    let dir = previews.join(&collection);
    std::fs::create_dir_all(&dir)?;
    let resolver = YouTubeResolver::with_base_url(Some(&options.shim));
    let total = requests.requests.len();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (i, request) in requests.requests.into_iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(options.delay).await;
        }
        let started = Instant::now();
        let asked = PreviewRequest {
            artist: &request.artist,
            title: &request.title,
            seconds: request.seconds,
            video_id: None,
            background: false,
        };
        let (found, why, facts, audio, fetch) = match resolver.find(&asked).await {
            Ok(Some(preview)) => {
                let name = format!("{}.audio", request.key);
                let dest = dir.join(&name);
                let fetched = resolver.fetch_preview(&preview.video_id, &dest, false).await;
                let (audio, fetch) = match fetched {
                    Ok(()) => (Some(name), "ok"),
                    Err(failure) => {
                        let _ = std::fs::remove_file(&dest);
                        (
                            None,
                            if failure == PreviewFailure::Busy {
                                "busy"
                            } else {
                                "not_sent"
                            },
                        )
                    }
                };
                (
                    "found",
                    None,
                    Some(Facts::of(&preview, &request)),
                    audio,
                    Some(fetch.to_string()),
                )
            }
            Ok(None) => ("none", None, None, None, None),
            Err(failure) => ("failed", Some(format!("{failure:?}")), None, None, None),
        };
        let label = format!("{found}/{}", fetch.as_deref().unwrap_or("-"));
        *counts.entry(label.clone()).or_default() += 1;
        eprintln!("[{}/{total}] {} {label}", i + 1, request.key);
        let record = Record {
            key: request.key.clone(),
            request,
            found: found.to_string(),
            why,
            facts,
            audio,
            fetch,
            seconds_taken: started.elapsed().as_secs_f64(),
            at: now(),
        };
        write_json(&dir.join(format!("{}.json", record.key)), &record)?;
    }
    println!("collection {collection}: {counts:?}");
    Ok(())
}

// ----- replay -----

/// One collection's answer for one song, served as the shim served it.
struct CachedSource {
    record: Record,
    dir: PathBuf,
}

#[async_trait]
impl PreviewSource for CachedSource {
    async fn find(&self, _request: &PreviewRequest<'_>) -> Result<Option<PreviewFacts>, Failure> {
        match self.record.found.as_str() {
            "found" => Ok(self.record.facts.as_ref().map(Facts::preview)),
            "none" => Ok(None),
            _ if self.record.why.as_deref() == Some("Busy") => Err(Failure::Busy),
            _ => Err(Failure::Unreachable),
        }
    }

    async fn fetch(&self, _video_id: &str, dest: &Path) -> bool {
        match &self.record.audio {
            Some(name) => std::fs::copy(self.dir.join(name), dest).is_ok(),
            None => false,
        }
    }

    async fn fetch_preview(
        &self,
        video_id: &str,
        dest: &Path,
        _background: bool,
    ) -> Result<(), PreviewFailure> {
        if self.record.fetch.as_deref() == Some("busy") {
            return Err(PreviewFailure::Busy);
        }
        if self.fetch(video_id, dest).await {
            Ok(())
        } else {
            Err(PreviewFailure::NotSent)
        }
    }
}

fn heard_name(heard: Heard) -> &'static str {
    match heard {
        Heard::Match => "match",
        Heard::Unclear => "unclear",
        Heard::Mismatch => "mismatch",
    }
}

/// One preview run as signals.json keeps it, with the outcome and sweep decision it came to.
struct PreviewRun {
    heard: Option<Heard>,
    sweep: &'static str,
    video: Option<String>,
    json: Value,
}

fn preview_run(
    outcome: &PreviewOutcome,
    request: &PreviewRequest<'_>,
    album: &str,
    record: Option<&Record>,
    collection: &str,
    seconds_taken: f64,
) -> PreviewRun {
    let lane = record.and_then(|r| r.facts.as_ref()).map(|f| f.lane.clone());
    let preview_audio = record
        .and_then(|r| r.audio.as_ref())
        .map(|audio| format!("previews/{collection}/{audio}"));
    let PreviewOutcome::Compared {
        heard,
        comparison,
        trusted,
        preview,
    } = outcome
    else {
        let why = match outcome {
            PreviewOutcome::NotAsked => "not asked".to_string(),
            PreviewOutcome::Unavailable(why) | PreviewOutcome::NotSent(why) => why.clone(),
            PreviewOutcome::Busy => "the shim was busy".to_string(),
            PreviewOutcome::Unreachable => "the shim could not be reached".to_string(),
            PreviewOutcome::FileUnreadable => "fpcalc could not read the file".to_string(),
            PreviewOutcome::Compared { .. } => unreachable!(),
        };
        return PreviewRun {
            heard: None,
            sweep: "ask",
            video: None,
            json: json!({
                "collection": collection,
                "outcome": "unavailable",
                "why": why,
                "preview_audio": preview_audio,
                "seconds_taken": seconds_taken,
            }),
        };
    };
    let album_agrees = LibraryReviewSweepWorker::preview_album_agrees(preview.album.as_deref(), album);
    // The sweep's own rules: a believed match from the file's album settles the song, and only a
    // clear mismatch against a row from that album is replaced (when the setting is on).
    let sweep = match heard {
        Heard::Match if *trusted && album_agrees => "fine",
        Heard::Mismatch
            if comparison.bit_error >= LibraryReviewSweepWorker::PREVIEW_REPLACE_FROM && album_agrees =>
        {
            "replace"
        }
        _ => "ask",
    };
    // Which part of trust failed: asked again with the length, then the artist, made to agree.
    let same_length = PreviewRequest {
        seconds: preview.seconds,
        ..*request
    };
    let same_artist = PreviewFacts {
        artist: Some(request.artist.to_string()),
        ..preview.clone()
    };
    let length_gap = match (preview.seconds, request.seconds) {
        (Some(got), Some(want)) => Some(got - want),
        _ => None,
    };
    PreviewRun {
        heard: Some(*heard),
        sweep,
        video: Some(preview.video_id.clone()),
        json: json!({
            "collection": collection,
            "outcome": heard_name(*heard),
            "why": Value::Null,
            "bit_error": comparison.bit_error,
            "offset_seconds": comparison.offset_seconds,
            "trusted": trusted,
            "trusted_if_length_equal": preview_check::trustworthy(&same_length, preview),
            "trusted_if_artist_equal": preview_check::trustworthy(request, &same_artist),
            "album_agrees": album_agrees,
            "sweep": sweep,
            "length_gap": length_gap,
            "preview": {
                "video_id": preview.video_id,
                "title": preview.title,
                "artist": preview.artist,
                "album": preview.album,
                "seconds": preview.seconds,
                "lane": lane,
            },
            "preview_audio": preview_audio,
            "seconds_taken": seconds_taken,
        }),
    }
}

fn grade_preview(expect: Expect, run: &PreviewRun) -> Grade {
    match (expect, run.heard) {
        (Expect::Match, Some(Heard::Match)) => Grade::Pass,
        (Expect::Match, Some(Heard::Mismatch)) => Grade::Hard,
        (Expect::Match, _) if run.sweep == "replace" => Grade::Hard,
        (Expect::Match, _) => Grade::Soft,
        (_, Some(Heard::Match)) => Grade::Hard,
        (_, Some(Heard::Mismatch)) => Grade::Pass,
        // Unclear or nothing to compare: the file goes to a person, as before.
        _ => Grade::Soft,
    }
}

fn same_name(same: SameRecording) -> (&'static str, Option<f64>) {
    match same {
        SameRecording::Same(s) => ("same", Some(s)),
        SameRecording::Differs(s) => ("differs", Some(s)),
        SameRecording::Unsure(s) => ("unsure", Some(s)),
        SameRecording::LengthDiffers(gap) => ("length_differs", Some(f64::from(gap))),
        SameRecording::CannotTell => ("cannot_tell", None),
    }
}

fn grade_same(expect: Expect, same: SameRecording) -> Grade {
    match (expect, same) {
        (Expect::Same, SameRecording::Same(_)) => Grade::Pass,
        // Remembered as a wrong file for every other upgrade too.
        (Expect::Same, SameRecording::Differs(_)) => Grade::Hard,
        (Expect::Same, _) => Grade::Soft,
        (_, SameRecording::Same(_)) => Grade::Hard,
        _ => Grade::Pass,
    }
}

fn length_of(path: &Path) -> Option<i32> {
    TagFile::open(path)
        .ok()
        .map(|file| file.duration_seconds())
        .filter(|seconds| *seconds > 0)
}

/// The windows same_recording compares (its `window_starts`), each with its similarity, so the
/// window that decided can be read later. Diagnostic only: the verdict is same_recording's own.
async fn windows(
    fingerprinter: &AudioFingerprinter,
    candidate: &Path,
    original: &Path,
    settings: &SoulseekSettings,
) -> (Option<i32>, Vec<Value>) {
    let clip = settings
        .effective_fingerprint_seconds()
        .min(same_recording::CLIP_SECONDS);
    let timeout = settings.effective_fingerprint_timeout_seconds();
    let (Some(new), Some(old)) = (length_of(candidate), length_of(original)) else {
        return (None, Vec::new());
    };
    let shorter = new.min(old);
    let last = shorter - clip;
    let starts = if last < clip {
        vec![0]
    } else if last < clip * 2 {
        vec![0, last]
    } else {
        vec![0, last / 2, last]
    };
    let mut out = Vec::new();
    for start in starts {
        let (a, b) = tokio::join!(
            fingerprinter.raw_fingerprint_window(candidate, start, clip, timeout),
            fingerprinter.raw_fingerprint_window(original, start, clip, timeout),
        );
        let similarity = match (a, b) {
            (Some(a), Some(b)) => same_recording::similarity(&a.items, &b.items, MAX_SHIFT),
            _ => None,
        };
        out.push(json!({ "start": start, "similarity": similarity }));
    }
    (Some(new - old), out)
}

struct Graded {
    kind: &'static str,
    group: String,
    grade: Grade,
    flaky: bool,
    json: Value,
}

struct Bench {
    bench: PathBuf,
    collections: Vec<String>,
    fingerprinter: AudioFingerprinter,
    settings: SoulseekSettings,
}

async fn replay_case(context: &Bench, case: Case) -> anyhow::Result<Graded> {
    let timeout = context.settings.effective_fingerprint_timeout_seconds();
    match case {
        Case::Preview {
            id,
            group,
            song_id,
            request_key,
            file,
            artist,
            title,
            album,
            seconds,
            expect,
        } => {
            let path = context.bench.join(&file);
            let request = PreviewRequest {
                artist: &artist,
                title: &title,
                seconds,
                video_id: None,
                background: false,
            };
            let mut runs = Vec::new();
            for collection in &context.collections {
                let dir = context.bench.join("previews").join(collection);
                let record = std::fs::read_to_string(dir.join(format!("{request_key}.json")))
                    .ok()
                    .and_then(|text| serde_json::from_str::<Record>(&text).ok());
                let started = Instant::now();
                let outcome = match &record {
                    Some(record) => {
                        let source = CachedSource {
                            record: record.clone(),
                            dir: dir.clone(),
                        };
                        preview_check::check(&source, &context.fingerprinter, &path, &request, timeout).await
                    }
                    None => PreviewOutcome::Unavailable("not collected".to_string()),
                };
                let seconds_taken = started.elapsed().as_secs_f64();
                runs.push(preview_run(
                    &outcome,
                    &request,
                    &album,
                    record.as_ref(),
                    collection,
                    seconds_taken,
                ));
            }
            // Judged by the collections that compared: a shim that was busy once says nothing about
            // the check, and analyze.py counts those runs on their own.
            let compared: Vec<&PreviewRun> = runs.iter().filter(|run| run.heard.is_some()).collect();
            let grade = compared
                .iter()
                .map(|run| grade_preview(expect, run))
                .max()
                .unwrap_or(Grade::Soft);
            let flaky = compared
                .windows(2)
                .any(|w| w[0].heard != w[1].heard || w[0].sweep != w[1].sweep || w[0].video != w[1].video);
            Ok(Graded {
                kind: "preview",
                group: group.clone(),
                grade,
                flaky,
                json: json!({
                    "id": id,
                    "kind": "preview",
                    "group": group,
                    "expect": expect.name(),
                    "song_id": song_id,
                    "grade": grade.name(),
                    "flaky": flaky,
                    "request": { "artist": artist, "title": title, "album": album, "seconds": seconds },
                    "file": file,
                    "runs": runs.into_iter().map(|run| run.json).collect::<Vec<_>>(),
                }),
            })
        }
        Case::SameRecording {
            id,
            group,
            song_id,
            candidate,
            original,
            expect,
        } => {
            let (new, old) = (context.bench.join(&candidate), context.bench.join(&original));
            let started = Instant::now();
            let same =
                same_recording::same_recording(&context.fingerprinter, &new, &old, &context.settings).await;
            let seconds_taken = started.elapsed().as_secs_f64();
            let (length_gap, windows) = windows(&context.fingerprinter, &new, &old, &context.settings).await;
            let (verdict, score) = same_name(same);
            let grade = grade_same(expect, same);
            Ok(Graded {
                kind: "same_recording",
                group: group.clone(),
                grade,
                flaky: false,
                json: json!({
                    "id": id,
                    "kind": "same_recording",
                    "group": group,
                    "expect": expect.name(),
                    "song_id": song_id,
                    "grade": grade.name(),
                    "flaky": false,
                    "candidate": candidate,
                    "original": original,
                    "verdict": verdict,
                    "score": score,
                    "windows": windows,
                    "length_gap": length_gap,
                    "seconds_taken": seconds_taken,
                }),
            })
        }
    }
}

async fn replay(options: &Options) -> anyhow::Result<bool> {
    let started = now();
    let cases_path = options.bench.join("cases.json");
    let text = std::fs::read_to_string(&cases_path).context("cases.json")?;
    let file: CaseFile = serde_json::from_str(&text).context("cases.json")?;
    let mut collections: Vec<String> = std::fs::read_dir(options.bench.join("previews"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    collections.sort();
    let wanted = |case: &Case| match (&options.only, case) {
        (None, _) => true,
        (Some(only), Case::Preview { .. }) => only == "preview",
        (Some(only), Case::SameRecording { .. }) => only == "same_recording",
    };
    let cases: Vec<Case> = file.cases.into_iter().filter(wanted).collect();
    let total = cases.len();
    let context = Arc::new(Bench {
        bench: options.bench.clone(),
        collections: collections.clone(),
        fingerprinter: AudioFingerprinter::new(),
        settings: SoulseekSettings::default(),
    });
    let mut done = 0usize;
    let mut results: Vec<Graded> = Vec::with_capacity(total);
    let mut stream = futures::stream::iter(cases.into_iter().map(|case| {
        let context = context.clone();
        async move { replay_case(&context, case).await }
    }))
    .buffer_unordered(options.jobs);
    while let Some(graded) = stream.next().await {
        let graded = graded?;
        done += 1;
        eprintln!(
            "[{done}/{total}] {} {} ({}): {}{}",
            graded.kind,
            graded.json["id"].as_str().unwrap_or("?"),
            graded.group,
            graded.grade.name(),
            if graded.flaky { ", flaky" } else { "" }
        );
        results.push(graded);
    }
    results.sort_by(|a, b| a.json["id"].as_str().cmp(&b.json["id"].as_str()));

    // (kind, group) -> [pass, soft, hard, flaky]
    let mut tally: BTreeMap<(String, String), [usize; 4]> = BTreeMap::new();
    for graded in &results {
        let counts = tally
            .entry((graded.kind.to_string(), graded.group.clone()))
            .or_default();
        counts[graded.grade as usize] += 1;
        counts[3] += usize::from(graded.flaky);
    }
    let commit = option_env!("GITHUB_SHA").unwrap_or("local");
    let run_dir = options.bench.join("runs").join(format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        &commit[..commit.len().min(8)]
    ));
    std::fs::create_dir_all(&run_dir)?;
    let manifest = json!({
        "octo_commit": commit,
        "bench_sha256": std::env::current_exe().map(|exe| sha256_of(&exe)).unwrap_or_default(),
        "cases_sha256": sha256_of(&cases_path),
        "collections": collections,
        "only": options.only,
        "started": started,
        "finished": now(),
    });
    let summary: Vec<Value> = tally
        .iter()
        .map(|((kind, group), [pass, soft, hard, flaky])| {
            json!({ "kind": kind, "group": group, "pass": pass, "soft": soft, "hard": hard, "flaky": flaky })
        })
        .collect();
    let signals = json!({
        "manifest": manifest,
        "summary": summary,
        "results": results.iter().map(|graded| graded.json.clone()).collect::<Vec<_>>(),
    });
    write_json(&run_dir.join("signals.json"), &signals)?;

    println!(
        "{:<15} {:<16} {:>5} {:>5} {:>5} {:>6}",
        "kind", "group", "pass", "soft", "hard", "flaky"
    );
    let mut hard_total = 0;
    for ((kind, group), [pass, soft, hard, flaky]) in &tally {
        println!("{kind:<15} {group:<16} {pass:>5} {soft:>5} {hard:>5} {flaky:>6}");
        hard_total += hard;
    }
    println!("signals: {}", run_dir.join("signals.json").display());
    if hard_total > 0 {
        println!("FAIL: {hard_total} case(s) a check got wrong in a way that costs a song");
    } else {
        println!("OK: no case cost a song");
    }
    Ok(hard_total == 0)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let options = match options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!(
                "{error}\nusage: verify_bench collect|replay --bench DIR [--shim URL] [--delay-ms N] \
                 [--only preview|same_recording] [--jobs N]"
            );
            std::process::exit(2);
        }
    };
    if options.command == "collect" {
        return collect(&options).await;
    }
    if !replay(&options).await? {
        std::process::exit(1);
    }
    Ok(())
}
