//! Runs Octo's wrong-song checks against a labelled set of files and says how often each one is
//! right: the YouTube preview check (with the Review sweep's auto-replace rule) and the Better
//! quality same-recording check. The cases, the files and the report stay outside the repository;
//! `scripts/verify-bench/` makes them.
//!
//! ```text
//! verify_bench --cases cases.json --out report.json [--shim http://127.0.0.1:18080]
//!              [--repeat 3] [--only preview|same_recording] [--delay-ms 1500]
//! ```
//!
//! `cases.json` is `{"cases": [...]}`, paths relative to the file itself:
//! - `{"id", "kind": "preview", "group", "file", "artist", "title", "album", "seconds", "expect"}`,
//!   where `expect` is `match` (the file is the song) or `not_match` (it is something else).
//! - `{"id", "kind": "same_recording", "group", "candidate", "original", "expect"}`, where `expect`
//!   is `same` or `refused`.
//!
//! Each case comes out `pass`, `soft` (no harm done, but the check did not help: an unclear preview
//! or a missed upgrade) or `hard` (a right file refused or replaced, a wrong file let through).
//! The exit code is 1 when any case is `hard`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use octo::services::fingerprint::preview_check::{self, Heard, PreviewOutcome, PreviewRequest};
use octo::services::fingerprint::same_recording::{SameRecording, same_recording};
use octo::services::library::library_review_sweep_worker::LibraryReviewSweepWorker;
use octo::services::you_tube::you_tube_resolver::YouTubeResolver;
use octo_core::settings::SoulseekSettings;
use octo_media::audio::AudioFingerprinter;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct CaseFile {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Case {
    Preview {
        id: String,
        #[serde(default)]
        group: String,
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

struct Options {
    cases: PathBuf,
    out: PathBuf,
    shim: String,
    repeat: usize,
    only: Option<String>,
    delay: Duration,
}

fn options() -> anyhow::Result<Options> {
    let mut cases = None;
    let mut out = None;
    let mut shim = "http://127.0.0.1:18080".to_string();
    let mut repeat = 3usize;
    let mut only = None;
    let mut delay = Duration::from_millis(1500);
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--cases" => cases = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--shim" => shim = value()?,
            "--repeat" => repeat = value()?.parse::<usize>()?.max(1),
            "--only" => only = Some(value()?),
            "--delay-ms" => delay = Duration::from_millis(value()?.parse()?),
            other => bail!("unknown flag {other}"),
        }
    }
    Ok(Options {
        cases: cases.context("--cases is required")?,
        out: out.context("--out is required")?,
        shim,
        repeat,
        only,
        delay,
    })
}

/// One run of the preview check, as the report keeps it.
struct PreviewRun {
    heard: Option<Heard>,
    /// What the Review sweep would do with the song: `fine`, `replace` or `ask`.
    sweep: &'static str,
    video: Option<String>,
    json: Value,
}

fn heard_name(heard: Heard) -> &'static str {
    match heard {
        Heard::Match => "match",
        Heard::Unclear => "unclear",
        Heard::Mismatch => "mismatch",
    }
}

fn preview_run(outcome: &PreviewOutcome, album: &str) -> PreviewRun {
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
            json: json!({ "outcome": "unavailable", "why": why }),
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
    PreviewRun {
        heard: Some(*heard),
        sweep,
        video: Some(preview.video_id.clone()),
        json: json!({
            "outcome": heard_name(*heard),
            "trusted": trusted,
            "bit_error": comparison.bit_error,
            "offset_seconds": comparison.offset_seconds,
            "album_agrees": album_agrees,
            "sweep": sweep,
            "preview": {
                "video_id": preview.video_id,
                "title": preview.title,
                "artist": preview.artist,
                "album": preview.album,
                "seconds": preview.seconds,
            },
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let options = match options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!(
                "{error}\nusage: verify_bench --cases cases.json --out report.json [--shim URL] [--repeat N] [--only preview|same_recording] [--delay-ms N]"
            );
            std::process::exit(2);
        }
    };
    // Absolute, since Octo runs fpcalc and ffmpeg from the temp folder.
    let base = std::path::absolute(&options.cases)?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let text = std::fs::read_to_string(&options.cases)
        .with_context(|| format!("reading {}", options.cases.display()))?;
    let file: CaseFile = serde_json::from_str(&text).context("cases.json")?;

    let fingerprinter = AudioFingerprinter::new();
    let resolver = YouTubeResolver::with_base_url(Some(&options.shim));
    let settings = SoulseekSettings::default();
    let timeout = settings.effective_fingerprint_timeout_seconds();

    let mut results = Vec::new();
    // (kind, group) -> [pass, soft, hard, flaky]
    let mut tally: BTreeMap<(String, String), [usize; 4]> = BTreeMap::new();
    let total = file.cases.len();
    for (n, case) in file.cases.into_iter().enumerate() {
        let (kind, id, group, grade, flaky, detail) = match case {
            Case::Preview {
                id,
                group,
                file,
                artist,
                title,
                album,
                seconds,
                expect,
            } => {
                if options.only.as_deref().is_some_and(|only| only != "preview") {
                    continue;
                }
                let path = base.join(&file);
                let request = PreviewRequest {
                    artist: &artist,
                    title: &title,
                    seconds,
                    video_id: None,
                    background: false,
                };
                let mut runs = Vec::new();
                for attempt in 0..options.repeat {
                    if n + attempt > 0 {
                        tokio::time::sleep(options.delay).await;
                    }
                    let outcome =
                        preview_check::check(&resolver, &fingerprinter, &path, &request, timeout).await;
                    runs.push(preview_run(&outcome, &album));
                }
                let grade = runs
                    .iter()
                    .map(|run| grade_preview(expect, run))
                    .max()
                    .unwrap_or(Grade::Soft);
                let flaky = runs.windows(2).any(|w| {
                    w[0].heard != w[1].heard || w[0].sweep != w[1].sweep || w[0].video != w[1].video
                });
                let detail = json!({
                    "expect": expect.name(),
                    "runs": runs.iter().map(|run| run.json.clone()).collect::<Vec<_>>(),
                });
                ("preview", id, group, grade, flaky, detail)
            }
            Case::SameRecording {
                id,
                group,
                candidate,
                original,
                expect,
            } => {
                if options
                    .only
                    .as_deref()
                    .is_some_and(|only| only != "same_recording")
                {
                    continue;
                }
                let same = same_recording(
                    &fingerprinter,
                    &base.join(&candidate),
                    &base.join(&original),
                    &settings,
                )
                .await;
                let (verdict, score) = same_name(same);
                let detail = json!({
                    "expect": expect.name(),
                    "verdict": verdict,
                    "score": score,
                });
                (
                    "same_recording",
                    id,
                    group,
                    grade_same(expect, same),
                    false,
                    detail,
                )
            }
        };
        eprintln!(
            "[{}/{total}] {kind} {id} ({group}): {}{}",
            n + 1,
            grade.name(),
            if flaky { ", flaky" } else { "" }
        );
        let counts = tally.entry((kind.to_string(), group.clone())).or_default();
        counts[grade as usize] += 1;
        counts[3] += usize::from(flaky);
        results.push(json!({
            "id": id,
            "kind": kind,
            "group": group,
            "grade": grade.name(),
            "flaky": flaky,
            "detail": detail,
        }));
    }

    let summary: Vec<Value> = tally
        .iter()
        .map(|((kind, group), [pass, soft, hard, flaky])| {
            json!({ "kind": kind, "group": group, "pass": pass, "soft": soft, "hard": hard, "flaky": flaky })
        })
        .collect();
    let report = json!({ "summary": summary, "results": results });
    std::fs::write(&options.out, serde_json::to_string_pretty(&report)?)
        .with_context(|| format!("writing {}", options.out.display()))?;

    println!(
        "{:<15} {:<16} {:>5} {:>5} {:>5} {:>6}",
        "kind", "group", "pass", "soft", "hard", "flaky"
    );
    let mut hard_total = 0;
    for ((kind, group), [pass, soft, hard, flaky]) in &tally {
        println!("{kind:<15} {group:<16} {pass:>5} {soft:>5} {hard:>5} {flaky:>6}");
        hard_total += hard;
    }
    println!("report: {}", options.out.display());
    if hard_total > 0 {
        println!("FAIL: {hard_total} case(s) a check got wrong in a way that costs a song");
        std::process::exit(1);
    }
    println!("OK: no case cost a song");
    Ok(())
}
