# Verify bench

Measures how reliably Octo's wrong-song checks decide on real music. It runs the real code from
`crates/octo` against a labelled set of right and wrong files:

- **Preview check**: is a file the song its YouTube preview plays? This is the check behind
  AcoustID for a sound it has never heard. The bench also applies the Review sweep's auto-replace rule.
- **Same recording**: may a Better quality copy take an original's place?

The files come from your own library, so they, the case list and the report stay in a bench folder
outside the repository. Only the tools live here.

## Cases

| Check | Group | File | Expected |
|---|---|---|---|
| preview | `right` | the song's own file | match |
| preview | `reencoded` | the file as MP3 128k and Opus 96k | match |
| preview | `shifted` | 10 s of silence before it, or its first 20 s cut | match |
| preview | `other_song` | another song by the same artist, under this song's name | not a match |
| preview | `namesake` | another artist's song of the same title | not a match |
| preview | `synthetic` | silence, a 440 Hz tone, pink noise | not a match |
| same recording | `reencoded` | MP3 320k, Opus 160k | same |
| same recording | `speed` | 0.97×, 1.03×, 1.06×, 1.10× | refused |
| same recording | `shorter` | the last 30 s cut | refused |
| same recording | `other_song`, `other_version` | sibling, namesake, a live or remastered take | refused |
| same recording | `synthetic` | the file silenced | refused |

Each case is graded:

- **pass**: the check got it right.
- **soft**: nothing was lost, but the check did not help. Examples: an unclear preview, a missed
  upgrade, a preview that could not be had.
- **hard**: the decision costs a song. Examples: a right file heard as a mismatch or one the sweep
  would replace, a wrong file heard as a match, another recording taken as the same.

The bench exits with 1 on any hard case. Preview cases run `--repeat` times (default 3). A case
whose runs differ in verdict, sweep decision or picked video is reported as **flaky**.

`runs/<time>-<commit>/signals.json` holds every run: the bit error, the offset, whether the preview
was believed (and whether the length or the artist was why not), the picked video's title, channel,
album, length and lane, and every same-recording window's similarity.

## Leaks

- `fetch_corpus.py` talks only to Navidrome. It accepts only a private IP address, never a name, so
  no DNS lookup is made, and it ignores proxy settings.
- `make_cases.py`, `triage.py`, `analyze.py` and `verify_bench replay` need no network. Run replay
  under `unshare -rn` to make sure of it. `label_server.py` listens on 127.0.0.1 only.
- `collect-previews.sh` is the only step that goes out, and only to YouTube through a throwaway
  shim. Nothing here ever uses Soulseek.
  - By default the shim runs on this machine.
  - With `VPN_CONTAINER=<gluetun container>`, the shim and the bench run in that container's
    network namespace instead. The script then stops, before and after the run, unless:
    - the egress IP is not the host's;
    - DNS goes to Gluetun;
    - there is no IPv6.

## Running it

From the checkout, with the bench folder at `bench-data/`:

1. Write `bench-data/.env` (chmod 600) with `NAVIDROME_URL=http://<LAN IP>:4533`,
   `NAVIDROME_USER` and `NAVIDROME_PASSWORD` for a read-only user.
2. Run `python3 -I scripts/verify-bench/fetch_corpus.py --songs 300` to copy the songs plus a
   sibling, a namesake and another version of each, where the library has them.
3. Run `python3 -I scripts/verify-bench/make_cases.py` to write `cases.json` and `requests.json`.
4. Download the bench binary from the `Verify bench` workflow:
   `gh run download <run id> --repo <fork> -n verify_bench -D bench-data`, then `chmod +x`.
5. Run `scripts/verify-bench/collect-previews.sh` three times, hours apart, to measure whether
   YouTube's pick drifts.
6. Run `unshare -rn bench-data/verify_bench replay --bench bench-data`.
7. Run `python3 -I scripts/verify-bench/triage.py`, then `python3 -I scripts/verify-bench/label_server.py`
   and label the queue at http://127.0.0.1:8765/.
8. Run `python3 -I scripts/verify-bench/analyze.py` and read `analysis.html` and `summary.md` in the run
   folder. After a code change, replay again and compare with `--baseline <old run>`.

## Data contracts

Everything lives in `<checkout>/bench-data/`, which git ignores. A local pre-commit hook also
refuses it. Never run `git clean -x` in the checkout. Paths inside the files are relative to
`bench-data/`.

**`songs.json`** (fetch_corpus): `[{id, role: pick|sibling|namesake|version|swap, of, artist,
title, album, seconds, genre, suffix, musicbrainz, path}]`

**`cases.json`** (make_cases): `{cases: [...]}`, as at the top of `verify_bench.rs`. Every case also
carries `song_id`, the pick it is about.

**`requests.json`** (make_cases): `{requests: [{key, artist, title, album, seconds}]}`, one per
pick. It is the only file the VM needs.
- `key` is the first 16 hex characters of `sha1(artist + "\0" + title + "\0" + seconds)`.

**`previews/<collection>/<key>.json`** (`verify_bench collect`):
- `{key, request, found: "found"|"none"|"failed", why, facts: {video_id, title, artist, album,
  seconds, lane: "row"|"search"}, audio, fetch: "ok"|"not_sent"|"busy", seconds_taken, at}`
- `audio` is a file name beside the JSON, or null.
- `<collection>` is `c<n>-<UTC date and time>`.

**`runs/<UTC time>-<octo sha>/signals.json`** (`verify_bench replay`):
```
{manifest: {octo_commit, bench_sha256, cases_sha256, collections: [..], started, finished},
 results: [{id, kind, group, expect, song_id, grade: pass|soft|hard, flaky,
   request: {artist, title, album, seconds}, file | candidate+original,
   runs: [ // preview: one per collection
     {collection, outcome: match|unclear|mismatch|unavailable, why, bit_error, offset_seconds,
      trusted, trusted_if_length_equal, trusted_if_artist_equal, album_agrees,
      sweep: fine|replace|ask, length_gap, preview: {video_id, title, artist, album, seconds, lane},
      preview_audio, seconds_taken} ],
   // same_recording instead:
   verdict: same|differs|unsure|length_differs|cannot_tell, score,
   windows: [{start, similarity}], length_gap, seconds_taken }]}
```

**`label-queue.json`** (triage):
- `{run, items: [{case_id, kind, reasons: [hard|t1|t2|gray_zone|untrusted|flaky|unavailable|audit],
  priority, a: {path, offset}, b: {path, offset}, request, preview, hidden: {...signals}}]}`
- For preview items, `a` is the file and `b` the preview audio. For same_recording items, `a` is
  the candidate and `b` the original.

**`labels.json`** (label_server):
- `{labels: {<case_id>: {label, note, at}}}`
- `label` is one of `right|wrong|version|preview_wrong|cant_tell|skip`.
- For same_recording, `right` means the same recording.
- Every save is also copied to `~/.local/share/octo-bench/labels-<run>-<date>.json`.

**Truth** (analyze):
- A label wins over everything else.
- Otherwise a constructed case's `expect` decides: `match`/`same` count as right,
  `not_match`/`refused` as wrong.
- A library file's own case with no label is "presumed right" and is reported separately.
