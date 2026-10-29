#!/usr/bin/env python3
"""Builds the right and wrong cases of the verify bench from the files fetch_corpus.py copied,
and writes <bench>/cases.json (see README.md next to this script).

    python3 -I make_cases.py [--bench <checkout>/bench-data] [--variant-songs 15] [--same-songs 20]

Needs ffmpeg. Works offline: everything is made from the files already in <bench>/corpus/right.
The derived files go to <bench>/corpus/derived and <bench>/corpus/synthetic, named by Navidrome
id and variant only. run-bench-vm.sh is copied next to cases.json, so the folder is all a run needs.
"""

import argparse
import json
import random
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
# The bench folder inside the checkout; ignored by git (see .gitignore and the pre-commit hook).
BENCH_DATA = HERE.parents[1] / "bench-data"
SYNTHETIC_SECONDS = 200


def ffmpeg(args, dest):
    if dest.exists() and dest.stat().st_size > 0:
        return True
    run = subprocess.run(
        ["ffmpeg", "-v", "error", "-nostdin", "-y", *args, str(dest)],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
    )
    if run.returncode != 0:
        dest.unlink(missing_ok=True)
        print(f"  ffmpeg failed for {dest.name}: {run.stderr.strip()[:200]}", file=sys.stderr)
        return False
    return True


def audio_from(source, *filters):
    """ffmpeg arguments that read the first audio stream of `source`, without its tags."""
    args = ["-i", str(source), "-map", "0:a:0", "-map_metadata", "-1", "-vn"]
    for f in filters:
        args += f
    return args


# name -> (extension, extra ffmpeg arguments)
VARIANTS = {
    "mp3-128": ("mp3", ["-c:a", "libmp3lame", "-b:a", "128k"]),
    "mp3-320": ("mp3", ["-c:a", "libmp3lame", "-b:a", "320k"]),
    "opus-96": ("opus", ["-c:a", "libopus", "-b:a", "96k"]),
    "opus-160": ("opus", ["-c:a", "libopus", "-b:a", "160k"]),
    "silence-10s": ("flac", ["-af", "adelay=10s:all=1"]),
    "speed-0.97": ("flac", ["-af", "atempo=0.97"]),
    "speed-1.03": ("flac", ["-af", "atempo=1.03"]),
    "speed-1.06": ("flac", ["-af", "atempo=1.06"]),
    "speed-1.10": ("flac", ["-af", "atempo=1.10"]),
}

SYNTHETIC = {
    "silence": f"anullsrc=r=44100:cl=stereo:d={SYNTHETIC_SECONDS}",
    "tone-440": f"sine=f=440:r=44100:d={SYNTHETIC_SECONDS}",
    "pink-noise": f"anoisesrc=color=pink:r=44100:a=0.3:d={SYNTHETIC_SECONDS}",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--bench", default=str(BENCH_DATA))
    parser.add_argument("--variant-songs", type=int, default=15, help="songs that also get re-encoded and shifted preview cases")
    parser.add_argument("--same-songs", type=int, default=20, help="songs that get same-recording cases")
    parser.add_argument("--synthetic-labels", type=int, default=4, help="songs each synthetic file is labelled as")
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args()
    if not shutil.which("ffmpeg"):
        sys.exit("ffmpeg is needed")

    bench = Path(args.bench).expanduser()
    songs = json.loads((bench / "songs.json").read_text())
    picks = [s for s in songs if s["role"] == "pick"]
    relatives = {}
    for s in songs:
        if s["of"]:
            relatives.setdefault(s["of"], {})[s["role"]] = s
    derived = bench / "corpus" / "derived"
    synthetic = bench / "corpus" / "synthetic"
    derived.mkdir(parents=True, exist_ok=True)
    synthetic.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    cases = []

    def variant(song, name):
        ext, extra = VARIANTS[name]
        dest = derived / f"{song['id']}-{name}.{ext}"
        return dest if ffmpeg(audio_from(bench / song["path"], extra), dest) else None

    def cut(song, name, extra):
        dest = derived / f"{song['id']}-{name}.flac"
        return dest if ffmpeg(audio_from(bench / song["path"], extra), dest) else None

    def preview(song, group, name, path, expect):
        cases.append({
            "id": f"p-{song['id']}-{name}",
            "kind": "preview",
            "group": group,
            "file": str(Path(path).relative_to(bench)),
            "artist": song["artist"],
            "title": song["title"],
            "album": song["album"],
            "seconds": song["seconds"] or None,
            "expect": expect,
        })

    def same(song, group, name, candidate, expect):
        cases.append({
            "id": f"s-{song['id']}-{name}",
            "kind": "same_recording",
            "group": group,
            "candidate": str(Path(candidate).relative_to(bench)),
            "original": song["path"],
            "expect": expect,
        })

    for n, song in enumerate(picks):
        print(f"[{n + 1}/{len(picks)}] {song['id']}", file=sys.stderr)
        own = bench / song["path"]
        kin = relatives.get(song["id"], {})
        # The file is the song: the preview should hear it.
        preview(song, "right", "own", own, "match")
        # Another file under this song's name: never a match.
        if "sibling" in kin:
            preview(song, "other_song", "sibling", bench / kin["sibling"]["path"], "not_match")
        if "namesake" in kin:
            preview(song, "namesake", "namesake", bench / kin["namesake"]["path"], "not_match")
        if n < args.variant_songs:
            for name in ("mp3-128", "opus-96"):
                if path := variant(song, name):
                    preview(song, "reencoded", name, path, "match")
            if path := variant(song, "silence-10s"):
                preview(song, "shifted", "silence-10s", path, "match")
            if path := cut(song, "cut-20s", ["-ss", "20"]):
                preview(song, "shifted", "cut-20s", path, "match")
        if n < args.same_songs:
            for name in ("mp3-320", "opus-160"):
                if path := variant(song, name):
                    same(song, "reencoded", name, path, "same")
            for name in ("speed-0.97", "speed-1.03", "speed-1.06", "speed-1.10"):
                if path := variant(song, name):
                    same(song, "speed", name, path, "refused")
            if song["seconds"] > 60 and (path := cut(song, "short-30s", ["-t", str(song["seconds"] - 30)])):
                same(song, "shorter", "short-30s", path, "refused")
            for role in ("sibling", "namesake", "version"):
                if role in kin:
                    cases.append({
                        "id": f"s-{song['id']}-{role}",
                        "kind": "same_recording",
                        "group": "other_version" if role == "version" else "other_song",
                        "candidate": kin[role]["path"],
                        "original": song["path"],
                        "expect": "refused",
                    })
            if path := cut(song, "silent", ["-af", "volume=0", "-c:a", "flac"]):
                same(song, "synthetic", "silent", path, "refused")

    # Sound that is no song at all, named as a few of the songs.
    for name, source in SYNTHETIC.items():
        dest = synthetic / f"{name}.flac"
        if not ffmpeg(["-f", "lavfi", "-i", source, "-c:a", "flac"], dest):
            continue
        for song in rng.sample(picks, min(args.synthetic_labels, len(picks))):
            cases.append({
                "id": f"p-{song['id']}-synthetic-{name}",
                "kind": "preview",
                "group": "synthetic",
                "file": str(dest.relative_to(bench)),
                "artist": song["artist"],
                "title": song["title"],
                "album": song["album"],
                "seconds": song["seconds"] or None,
                "expect": "not_match",
            })

    (bench / "cases.json").write_text(json.dumps({"cases": cases}, indent=1, ensure_ascii=False))
    shutil.copy2(HERE / "run-bench-vm.sh", bench / "run-bench-vm.sh")
    kinds = {}
    for case in cases:
        key = f"{case['kind']}/{case['group']}"
        kinds[key] = kinds.get(key, 0) + 1
    print(f"wrote {len(cases)} cases to {bench / 'cases.json'}", file=sys.stderr)
    for key in sorted(kinds):
        print(f"  {key}: {kinds[key]}", file=sys.stderr)


if __name__ == "__main__":
    main()
