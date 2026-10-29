#!/usr/bin/env python3
"""Picks the cases of a verify bench run a person should listen to, and writes
<run>/label-queue.json (see "Data contracts" in README.md next to this script).

    python3 -I triage.py [--run <bench-data>/runs/<dir>] [--audit 30] [--max 150] [--seed 7]

Reads <run>/signals.json and <bench>/cases.json; needs no network. Priority 1 is every hard case
and every possible wrong decision (T1: a right file refused or replaced; T2: a wrong file taken),
priority 2 the gray zone, priority 3 a random audit of cases that looked fine. Synthetic cases
only get in as hard failures.
"""

import argparse
import json
import random
import sys
from pathlib import Path

BENCH_DATA = Path(__file__).resolve().parents[2] / "bench-data"
PREFERRED = ("right", "namesake", "swap", "other_song")
ORDER = ["hard", "t1", "t2", "gray_zone", "untrusted", "flaky", "unavailable", "audit"]
PRIORITY = {"hard": 1, "t1": 1, "t2": 1, "gray_zone": 2, "untrusted": 2, "flaky": 2, "unavailable": 2, "audit": 3}
BIT_ERROR_GRAY = (0.20, 0.45)
SCORE_GRAY = (0.60, 0.90)
PREVIEW_AT = 30


def newest_run(bench):
    runs = sorted(p for p in (bench / "runs").glob("*") if (p / "signals.json").exists())
    if not runs:
        sys.exit(f"no run with a signals.json in {bench / 'runs'}")
    return runs[-1]


def number(value):
    return value if isinstance(value, (int, float)) and not isinstance(value, bool) else None


def preview_reasons(result):
    """The reasons a preview result needs a listener."""
    runs = result.get("runs") or []
    group, expect = result.get("group"), result.get("expect")
    reasons = set()
    if any(r.get("sweep") == "replace" for r in runs) and expect == "match":
        reasons.add("t1")
    if group == "right" and any(r.get("outcome") == "mismatch" for r in runs):
        reasons.add("t1")
    if expect == "not_match" and any(r.get("outcome") == "match" for r in runs):
        reasons.add("t2")
    for r in runs:
        bit_error = number(r.get("bit_error"))
        if bit_error is not None and BIT_ERROR_GRAY[0] <= bit_error <= BIT_ERROR_GRAY[1]:
            reasons.add("gray_zone")
    if group == "right":
        if any(r.get("trusted") is False for r in runs):
            reasons.add("untrusted")
        if any(r.get("outcome") == "unavailable" for r in runs):
            reasons.add("unavailable")
    return reasons


def same_reasons(result):
    reasons = set()
    if result.get("expect") == "refused" and result.get("verdict") == "same":
        reasons.add("t2")
    score = number(result.get("score"))
    if score is not None and SCORE_GRAY[0] <= score <= SCORE_GRAY[1]:
        reasons.add("gray_zone")
    return reasons


def reasons_of(result):
    reasons = set()
    if result.get("grade") == "hard":
        reasons.add("hard")
    elif result.get("group") == "synthetic":
        return reasons
    reasons |= preview_reasons(result) if result.get("kind") == "preview" else same_reasons(result)
    if result.get("flaky"):
        reasons.add("flaky")
    return reasons


def confident(result):
    """A case that passed with no doubt: a clear outcome in every run, or a clear verdict."""
    if result.get("grade") != "pass" or result.get("flaky"):
        return False
    if result.get("kind") == "preview":
        runs = result.get("runs") or []
        return bool(runs) and all(r.get("outcome") in ("match", "mismatch") and r.get("trusted") is not False for r in runs)
    return result.get("verdict") in ("same", "differs")


def path_of(value, base):
    """`value` as a path relative to the bench folder, or None."""
    if not value:
        return None
    return str(Path(base) / value) if base else str(value)


def preview_item(result, case):
    runs = result.get("runs") or []
    seconds = number((result.get("request") or {}).get("seconds")) or number(case.get("seconds")) or 0
    with_audio = next((r for r in runs if r.get("preview_audio")), None)
    b = {"path": None, "offset": 0}
    if with_audio:
        audio = with_audio["preview_audio"]
        # A bare name sits beside the collection's JSON.
        base = f"previews/{with_audio['collection']}" if "/" not in audio and with_audio.get("collection") else None
        b = {
            "path": path_of(audio, base),
            "offset": max(0, PREVIEW_AT + (number(with_audio.get("offset_seconds")) or 0)),
        }
    shown = with_audio or (runs[0] if runs else {})
    return (
        {"path": result.get("file") or case.get("file"), "offset": PREVIEW_AT if seconds >= 60 else 0},
        b,
        result.get("request") or {k: case.get(k) for k in ("artist", "title", "album", "seconds")},
        shown.get("preview"),
    )


def same_item(result, case):
    windows = [w for w in (result.get("windows") or []) if number(w.get("similarity")) is not None]
    start = 0
    if windows:
        start = number(min(windows, key=lambda w: w["similarity"]).get("start")) or 0
    return (
        {"path": result.get("candidate") or case.get("candidate"), "offset": start},
        {"path": result.get("original") or case.get("original"), "offset": start},
        result.get("request"),
        None,
    )


HIDDEN_RUN = (
    "collection", "outcome", "why", "bit_error", "offset_seconds", "trusted", "trusted_if_length_equal",
    "trusted_if_artist_equal", "album_agrees", "sweep", "length_gap", "preview", "seconds_taken",
)


def hidden_of(result):
    hidden = {k: result.get(k) for k in ("group", "expect", "grade", "flaky")}
    if result.get("kind") == "preview":
        hidden["runs"] = [{k: r.get(k) for k in HIDDEN_RUN} for r in result.get("runs") or []]
    else:
        for k in ("verdict", "score", "windows", "length_gap"):
            hidden[k] = result.get(k)
    return hidden


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--run", help="a bench-data/runs/<dir>; the newest by default")
    parser.add_argument("--bench", default=str(BENCH_DATA))
    parser.add_argument("--audit", type=int, default=30, help="confident cases added at random")
    parser.add_argument("--max", type=int, default=150, help="most items in the queue, audit included")
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args()

    bench = Path(args.bench).expanduser()
    run = Path(args.run).expanduser() if args.run else newest_run(bench)
    signals = json.loads((run / "signals.json").read_text())
    cases = {c["id"]: c for c in json.loads((bench / "cases.json").read_text()).get("cases", [])}
    rng = random.Random(args.seed)

    flagged, calm = [], []
    for result in signals.get("results", []):
        reasons = reasons_of(result)
        (flagged if reasons else calm).append((result, reasons))

    def rank(entry):
        result, reasons = entry
        best = min(ORDER.index(r) for r in reasons)
        preferred = result.get("group") not in PREFERRED
        return (best, preferred, -len(reasons), result.get("id", ""))

    flagged.sort(key=rank)
    chosen = flagged[: args.max]

    # The audit: mostly right files, the rest from the other preferred groups.
    pool = [(r, {"audit"}) for r, _ in calm if confident(r) and r.get("group") in PREFERRED]
    right = [e for e in pool if e[0].get("group") == "right"]
    rest = [e for e in pool if e[0].get("group") != "right"]
    room = max(0, min(args.audit, args.max - len(chosen)))
    want_right = min(len(right), round(room * 2 / 3))
    audit = rng.sample(right, want_right)
    audit += rng.sample(rest, min(len(rest), room - len(audit)))
    if len(audit) < room:
        left = [e for e in right if e not in audit]
        audit += rng.sample(left, min(len(left), room - len(audit)))
    chosen += sorted(audit, key=lambda e: e[0].get("id", ""))

    items = []
    for result, reasons in chosen:
        case = cases.get(result.get("id"), {})
        a, b, request, preview = (preview_item if result.get("kind") == "preview" else same_item)(result, case)
        items.append({
            "case_id": result.get("id"),
            "kind": result.get("kind"),
            "reasons": sorted(reasons, key=ORDER.index),
            "priority": min(PRIORITY[r] for r in reasons),
            "a": a,
            "b": b,
            "request": request,
            "preview": preview,
            "hidden": hidden_of(result),
        })

    (run / "label-queue.json").write_text(json.dumps({"run": run.name, "items": items}, indent=1, ensure_ascii=False))
    counts = {}
    for item in items:
        for reason in item["reasons"]:
            counts[reason] = counts.get(reason, 0) + 1
    print(f"wrote {len(items)} items to {run / 'label-queue.json'} ({len(flagged)} flagged): {counts}", file=sys.stderr)
    if len(flagged) > args.max:
        print(f"  {len(flagged) - args.max} flagged cases did not fit under --max", file=sys.stderr)


if __name__ == "__main__":
    main()
