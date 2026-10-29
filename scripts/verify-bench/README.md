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

`report.json` holds every run: the bit error, the offset, whether the preview was believed, the
picked video's title, channel, album and length, and the similarity of every same-recording verdict.

## Leaks

- `fetch_corpus.py` talks only to Navidrome. It accepts only a private IP address, never a name, so
  no DNS lookup is made, and it ignores proxy settings.
- `make_cases.py` and the `--offline` run need no network.
- The preview run asks a throwaway shim that runs in the network namespace of a Gluetun container,
  next to the bench itself. Both leave only through the VPN and resolve through Gluetun.
  `run-bench-vm.sh` checks the following before and after the run, and stops if any of them fails:
  - the egress IP is not the host's;
  - DNS goes to Gluetun;
  - there is no IPv6.

## Running it

1. Make a bench folder with `.env` (chmod 600) holding `NAVIDROME_URL=http://<LAN IP>:4533`,
   `NAVIDROME_USER` and `NAVIDROME_PASSWORD` for a read-only user.
2. Run `python3 -I fetch_corpus.py --bench <folder>`. It copies about 60 songs, plus a sibling, a
   namesake and another version of each where the library has one.
3. Run `python3 -I make_cases.py --bench <folder>` to write `cases.json` and copy `run-bench-vm.sh`
   into the folder.
4. Put the bench binary into the folder as `verify_bench` (chmod +x). The `Verify bench` workflow
   builds it on Debian bookworm, the Octo image's base:
   `gh run download <run id> --repo <fork> -n verify_bench -D <folder>`.
5. Copy the folder to the Docker host without `.env`, and on the host run
   `VPN_CONTAINER=<gluetun container> ./run-bench-vm.sh`. Add `--repeat 1` for a quicker pass.
   `./run-bench-vm.sh --offline --only same_recording` runs the same-recording cases with no
   network at all.
6. Read the table it prints and look at every hard case in `report.json`. A right file the check
   refuses may itself be the wrong song.
