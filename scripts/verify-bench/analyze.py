#!/usr/bin/env python3
"""Analysis step of the verify bench.

Reads <run>/signals.json, bench-data/cases.json and labels.json, and writes
<run>/analysis.json, <run>/summary.md and <run>/analysis.html (self-contained).

    python3 -I analyze.py [--bench <checkout>/bench-data] [--run <run dir>] [--baseline <run dir>]

Python standard library only; no network.  The decision rules below mirror the Rust code:
  preview_check.rs   judge(), MATCH_AT_MOST, MISMATCH_FROM
  same_recording.rs  SameRecording::from_similarity(), SAME_PERCENT, DIFFERS_BELOW_PERCENT
  library_review_sweep_worker.rs  PREVIEW_REPLACE_FROM, decide() (by_preview), preview_album_agrees
"""
import argparse
import html
import json
import math
import re
import sys
from pathlib import Path

# ---- today's constants (mirror the Rust) -------------------------------------------------------
MATCH_AT_MOST = 0.25
MISMATCH_FROM = 0.38
PREVIEW_REPLACE_FROM = 0.42
SAME_PERCENT = 85.0
DIFFERS_BELOW_PERCENT = 60.0

OFFSET_EDGE = 55.0
NEAR = 0.03

SLO = {
    "t1_max": 0,
    "t2_rate_max": 0.01,
    "t2_upper_max": 0.02,
    "t3_mismatch_max": 0.05,
    "flaky_max": 0.02,
    "coverage_min": 0.90,
}

Z = 1.959964


# ---- statistics --------------------------------------------------------------------------------
def wilson(k, n, z=Z):
    if n <= 0:
        return None, None
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return max(0.0, c - h), min(1.0, c + h)


def rate(k, n):
    lo, hi = wilson(k, n)
    out = {"n": n, "count": k, "rate": (k / n) if n else None, "lo": lo, "hi": hi}
    if k == 0 and n > 0:
        out["rule_of_three_upper"] = min(1.0, 3.0 / n)
    return out


def percentile(values, q):
    v = sorted(x for x in values if isinstance(x, (int, float)))
    if not v:
        return None
    pos = (len(v) - 1) * q
    lo, hi = int(math.floor(pos)), int(math.ceil(pos))
    return v[lo] + (v[hi] - v[lo]) * (pos - lo)


def num(x):
    return x if isinstance(x, (int, float)) and not isinstance(x, bool) else None


# ---- decisions (mirror the Rust) ---------------------------------------------------------------
def judge(bit_error, trusted, match_at=MATCH_AT_MOST, mismatch_from=MISMATCH_FROM):
    if bit_error <= match_at:
        return "match"
    if trusted and bit_error >= mismatch_from:
        return "mismatch"
    return "unclear"


def sweep_of(outcome, bit_error, trusted, album_agrees, replace_from=PREVIEW_REPLACE_FROM):
    if outcome == "mismatch" and bit_error >= replace_from and album_agrees:
        return "replace"
    if outcome == "match" and trusted and album_agrees:
        return "fine"
    return "ask"


def derive(run, match_at=MATCH_AT_MOST, mismatch_from=MISMATCH_FROM, replace_from=PREVIEW_REPLACE_FROM):
    """(outcome, sweep) of a preview run under the given constants."""
    be = num(run.get("bit_error"))
    if run.get("outcome") == "unavailable" or be is None:
        return run.get("outcome"), run.get("sweep")
    trusted = bool(run.get("trusted"))
    outcome = judge(be, trusted, match_at, mismatch_from)
    return outcome, sweep_of(outcome, be, trusted, bool(run.get("album_agrees")), replace_from)


def same_verdict(res, same_pct=SAME_PERCENT, differs_pct=DIFFERS_BELOW_PERCENT):
    v = res.get("verdict")
    s = num(res.get("score"))
    if v in ("length_differs", "cannot_tell") or s is None:
        return v
    pct = s * 100.0
    if pct >= same_pct:
        return "same"
    if pct < differs_pct:
        return "differs"
    return "unsure"


# ---- loading -----------------------------------------------------------------------------------
def load_json(p):
    try:
        with open(p, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def newest_run(bench):
    runs = [d for d in (bench / "runs").glob("*") if (d / "signals.json").is_file()] if (bench / "runs").is_dir() else []
    return sorted(runs, key=lambda d: d.name)[-1] if runs else None


def load_labels(run, bench):
    for p in (run / "labels.json", bench / "labels.json"):
        d = load_json(p)
        if isinstance(d, dict):
            raw = d.get("labels", d)
            out = {}
            for k, v in (raw or {}).items():
                lab = v.get("label") if isinstance(v, dict) else v
                if isinstance(lab, str):
                    out[k] = lab
            return out, str(p)
    return {}, None


def load_cases(bench):
    d = load_json(bench / "cases.json")
    items = d.get("cases", []) if isinstance(d, dict) else (d if isinstance(d, list) else [])
    return {c["id"]: c for c in items if isinstance(c, dict) and "id" in c}


# ---- per-case classification -------------------------------------------------------------------
def kind_of(res):
    k = str(res.get("kind") or "")
    if "preview" in k:
        return "preview"
    if "same" in k:
        return "same_recording"
    if isinstance(res.get("runs"), list):
        return "preview"
    if "verdict" in res:
        return "same_recording"
    return k or "unknown"


def norm(s):
    return re.sub(r"\s+", " ", str(s or "").strip().casefold())


def truth_of(res, case, labels):
    """(truth|None, label|None, presumed, wrong_pick)"""
    lab = labels.get(res["id"])
    if lab in ("right", "wrong", "version"):
        return lab, lab, False, False
    expect = res.get("expect") or case.get("expect")
    t = {"match": "right", "same": "right", "not_match": "wrong", "refused": "wrong"}.get(expect)
    # A file built to be wrong (another song under this one's name) stays wrong when the label only
    # says the preview is wrong too, or that the listener could not tell.
    constructed_wrong = t == "wrong" and res.get("group") != "right"
    if lab == "preview_wrong":
        return ("wrong" if constructed_wrong else "right"), lab, False, True
    if lab in ("cant_tell", "skip"):
        return (t if constructed_wrong else None), lab, False, False
    presumed = res["kind"] == "preview" and res.get("group") == "right" and t == "right"
    return t, None, presumed, False


def tags_for(info, res, run):
    """Root-cause tags for a case, judged on the run that made it a problem."""
    t = []
    group, truth = info["group"], info["truth"]
    if info["kind"] == "preview":
        prev = (run or {}).get("preview") or {}
        req = res.get("request") or {}
        lane = "search" if prev.get("lane") == "search" else "row"
        differs = bool(prev) and (
            (prev.get("title") is not None and norm(prev.get("title")) != norm(req.get("title")))
            or (prev.get("artist") is not None and norm(prev.get("artist")) != norm(req.get("artist")))
        )
        if info["wrong_pick"] or (truth == "right" and differs):
            t.append("wrong_pick_" + lane)
        if run:
            trusted = run.get("trusted")
            if trusted is False:
                l, a = bool(run.get("trusted_if_length_equal")), bool(run.get("trusted_if_artist_equal"))
                if l:
                    t.append("untrusted_length")
                if a:
                    t.append("untrusted_artist")
                if not l and not a:
                    t.append("untrusted_title")
            if run.get("album_agrees") is False:
                t.append("album_disagrees")
            off = num(run.get("offset_seconds"))
            if off is not None and abs(off) >= OFFSET_EDGE:
                t.append("offset_edge")
            be = num(run.get("bit_error"))
            if be is not None and any(abs(be - c) <= NEAR for c in (MATCH_AT_MOST, MISMATCH_FROM, PREVIEW_REPLACE_FROM)):
                t.append("near_threshold")
            if "too little audio" in str(run.get("why") or "").lower() or be is None:
                t.append("low_information")
    else:
        s = num(res.get("score"))
        if s is not None and any(abs(s * 100 - c) <= NEAR * 100 for c in (SAME_PERCENT, DIFFERS_BELOW_PERCENT)):
            t.append("near_threshold")
        if s is None:
            t.append("low_information")
    if group in ("reencoded", "shifted"):
        t.append("encoding")
    if truth == "version":
        t.append("version_confusion")
    if info.get("flaky"):
        t.append("flaky")
    return t


def what_happened(info, res, run):
    head = "%s [%s/%s] truth=%s%s" % (info["id"], info["kind"], info["group"], info["truth"],
                                      " (presumed)" if info["presumed"] else "")
    if info["kind"] == "same_recording":
        return "%s: verdict=%s score=%s length_gap=%s" % (head, res.get("verdict"), res.get("score"), res.get("length_gap"))
    req = res.get("request") or {}
    if not run:
        return head + ": no runs"
    prev = run.get("preview") or {}
    bits = ["outcome=" + str(run.get("outcome"))]
    if run.get("why"):
        bits.append("why=" + str(run.get("why")))
    for k in ("bit_error", "offset_seconds", "trusted", "album_agrees", "sweep"):
        if run.get(k) is not None:
            bits.append("%s=%s" % (k, run.get(k)))
    if run.get("trusted") is False:
        bits.append("trusted_if_length_equal=%s trusted_if_artist_equal=%s" % (
            run.get("trusted_if_length_equal"), run.get("trusted_if_artist_equal")))
    if prev:
        bits.append("preview=%r by %r (%s, %ss) vs asked %r by %r" % (
            prev.get("title"), prev.get("artist"), prev.get("lane"), prev.get("seconds"),
            req.get("title"), req.get("artist")))
    if info["flaky"]:
        bits.append("flaky across %d runs" % len(res.get("runs") or []))
    return head + ": " + "; ".join(bits)


def classify(res, case, labels):
    res = dict(res)
    res["kind"] = kind_of(res)
    for k in ("group", "expect", "song_id"):
        if res.get(k) is None and case.get(k) is not None:
            res[k] = case[k]
    truth, label, presumed, wrong_pick = truth_of(res, case, labels)
    info = {"id": res["id"], "kind": res["kind"], "group": res.get("group") or "?", "truth": truth,
            "label": label, "presumed": presumed, "wrong_pick": wrong_pick, "grade": res.get("grade"),
            "t1": False, "t2": False, "t3": [], "version_match": False, "flaky": False,
            "flaky_recorded": bool(res.get("flaky")), "flaky_computed": False, "replace_flip": False,
            "n_runs": 0, "cov_runs": 0, "unavailable_whys": [], "anomalies": []}
    focus = None
    if res["kind"] == "preview":
        runs = [r for r in (res.get("runs") or []) if isinstance(r, dict)]
        info["n_runs"] = len(runs)
        sig = {(r.get("outcome"), r.get("sweep"), (r.get("preview") or {}).get("video_id")) for r in runs}
        info["flaky_computed"] = len(runs) > 1 and len(sig) > 1
        info["flaky"] = info["flaky_computed"] or info["flaky_recorded"]
        # The one change between collections that costs a song: a right file the sweep replaces in
        # one collection and not in another.
        sweeps = {r.get("sweep") for r in runs if r.get("outcome") != "unavailable"}
        info["replace_flip"] = truth == "right" and "replace" in sweeps and len(sweeps) > 1
        avail = [r for r in runs if r.get("outcome") != "unavailable"]
        for r in runs:
            if r.get("outcome") == "unavailable":
                info["unavailable_whys"].append(str(r.get("why") or "unknown"))
                if r.get("sweep") == "replace":
                    info["anomalies"].append("unavailable run carries sweep=replace (not counted as T1)")
        # What the person is spared: the sweep settles the song as fine and asks nothing.
        info["cov_runs"] = sum(1 for r in runs if r.get("sweep") == "fine")
        if truth == "right":
            rep = [r for r in avail if r.get("sweep") == "replace"]
            info["t1"] = bool(rep)
            for o in ("mismatch", "unclear", "unavailable"):
                if any(r.get("outcome") == o for r in runs):
                    info["t3"].append(o)
            focus = (rep or [r for o in ("mismatch", "unclear", "unavailable") for r in runs if r.get("outcome") == o] or runs or [None])[0]
        elif truth == "wrong":
            # Silent harm is a wrong file the sweep settles as fine; a match it does not believe
            # (another channel, a search hit) is still asked about.
            mat = [r for r in avail if r.get("sweep") == "fine"]
            info["t2"] = bool(mat)
            focus = (mat or runs or [None])[0]
        elif truth == "version":
            mat = [r for r in avail if r.get("outcome") == "match"]
            info["version_match"] = bool(mat)
            focus = (mat or runs or [None])[0]
        if focus is None and info["flaky"] and runs:
            focus = runs[0]
        if focus is None and runs:
            focus = runs[0]
    else:
        v = res.get("verdict")
        if truth in ("wrong", "version") and v == "same":
            info["t1"] = True
        if truth == "right" and v != "same":
            info["t3"].append("missed_upgrade")
    info["tier"] = ("T1" if info["t1"] else "T2" if info["t2"] else "T3" if info["t3"]
                    else "flaky" if info["flaky"] else "excluded" if truth is None else "ok")
    flagged = info["tier"] in ("T1", "T2", "T3", "flaky") or info["version_match"]
    if flagged:
        info["tags"] = tags_for(info, res, focus)
        info["what"] = what_happened(info, res, focus)
    return info, res


# ---- metrics -----------------------------------------------------------------------------------
def preview_metrics(infos):
    right = [i for i in infos if i["truth"] == "right"]
    wrong = [i for i in infos if i["truth"] == "wrong"]
    version = [i for i in infos if i["truth"] == "version"]
    right_runs = sum(i["n_runs"] for i in right)
    m = {
        "cases": len(infos),
        "t1_destructive": rate(sum(i["t1"] for i in right), len(right)),
        "t2_silent_harm": rate(sum(i["t2"] for i in wrong), len(wrong)),
        "t3_mismatch_on_right": rate(sum("mismatch" in i["t3"] for i in right), len(right)),
        "t3_unclear_on_right": rate(sum("unclear" in i["t3"] for i in right), len(right)),
        "t3_unavailable_on_right": rate(sum("unavailable" in i["t3"] for i in right), len(right)),
        "coverage": rate(sum(i["cov_runs"] for i in right), right_runs),
        "version_match": rate(sum(i["version_match"] for i in version), len(version)),
        "flaky_recorded": rate(sum(i["flaky_recorded"] for i in infos), len(infos)),
        "flaky_computed": rate(sum(i["flaky_computed"] for i in infos), len(infos)),
        "flaky": rate(sum(i["flaky"] for i in infos), len(infos)),
        "replace_flip_on_right": rate(sum(i["replace_flip"] for i in right), len(right)),
    }
    return m


def same_metrics(infos):
    right = [i for i in infos if i["truth"] == "right"]
    bad = [i for i in infos if i["truth"] in ("wrong", "version")]
    return {
        "cases": len(infos),
        "t1_destructive": rate(sum(i["t1"] for i in bad), len(bad)),
        "t3_missed_upgrade": rate(sum(bool(i["t3"]) for i in right), len(right)),
    }


def metrics_block(infos, include_presumed):
    use = [i for i in infos if include_presumed or not i["presumed"]]
    block = {}
    for kind, fn in (("preview", preview_metrics), ("same_recording", same_metrics)):
        sub = [i for i in use if i["kind"] == kind]
        entry = {"overall": fn(sub), "groups": {}}
        for g in sorted({i["group"] for i in sub}):
            entry["groups"][g] = fn([i for i in sub if i["group"] == g])
        block[kind] = entry
    return block


def slo_rows(block):
    p, s = block["preview"]["overall"], block["same_recording"]["overall"]

    def t1(r):
        if r["n"] == 0:
            return None, "no data"
        ok = r["count"] == 0
        return ok, "%d/%d" % (r["count"], r["n"]) + ("" if not ok else ", 3/n = %.2f%%" % (100 * r["rule_of_three_upper"]))

    def pct(r):
        return "n/a" if r["rate"] is None else "%.2f%% (%d/%d, 95%% %.2f-%.2f%%)" % (
            100 * r["rate"], r["count"], r["n"], 100 * r["lo"], 100 * r["hi"])

    rows = []
    ok, txt = t1(p["t1_destructive"])
    rows.append(("T1 preview destructive == 0", ok, txt))
    ok, txt = t1(s["t1_destructive"])
    rows.append(("T1 same_recording destructive == 0", ok, txt))
    t2 = p["t2_silent_harm"]
    if t2["n"] == 0:
        rows.append(("T2 silent harm <= 1% (upper <= 2%)", None, "no data"))
    else:
        ok = t2["rate"] <= SLO["t2_rate_max"] and t2["hi"] <= SLO["t2_upper_max"]
        rows.append(("T2 silent harm <= 1% (upper <= 2%)", ok, pct(t2)))
    t3 = p["t3_mismatch_on_right"]
    rows.append(("T3 mismatch on right <= 5%", None if t3["n"] == 0 else t3["rate"] <= SLO["t3_mismatch_max"], pct(t3)))
    # Gated: a right file must never be replaced in one collection and kept in another. Any other
    # change between collections (fine, ask, unclear, another video) only changes what is asked,
    # so it is reported, not gated.
    ok, txt = t1(p["replace_flip_on_right"])
    rows.append(("Stability: no flip into replace on a right file == 0", ok, txt))
    fl = p["flaky"]
    rows.append(("Flaky, any change (reported, target <= 2%)", None if fl["n"] == 0 else fl["rate"] <= SLO["flaky_max"], pct(fl)))
    cv = p["coverage"]
    rows.append(("Coverage (settled as fine) >= 90%", None if cv["n"] == 0 else cv["rate"] >= SLO["coverage_min"], pct(cv)))
    return rows


def verdict_of(rows_by_variant):
    keys = ("T1 preview", "T1 same_recording", "T2", "Stability", "Coverage")
    failing = []
    for v, rows in rows_by_variant.items():
        for name, ok, _ in rows:
            if name.startswith(keys) and ok is not True:
                failing.append("%s [%s]%s" % (name, v, " (no data)" if ok is None else ""))
    return ("works out of the box" if not failing else "does not work out of the box"), failing


# ---- threshold sweeps --------------------------------------------------------------------------
def frange(lo, hi):
    return [k / 100.0 for k in range(int(round(lo * 100)), int(round(hi * 100)) + 1)]


def preview_sweep(results, truths, param, values, today):
    right = [r for r in results if truths[r["id"]] == "right"]
    wrong = [r for r in results if truths[r["id"]] == "wrong"]
    rows = []
    for v in values:
        kw = {param: v}
        t1 = t2 = mism = caught = cov = nruns = 0
        for r in right:
            runs = r["runs"]
            d = [derive(x, **kw) for x in runs]
            nruns += len(runs)
            cov += sum(1 for x, (o, _) in zip(runs, d) if o == "match" and x.get("trusted"))
            av = [(x, o, s) for x, (o, s) in zip(runs, d) if x.get("outcome") != "unavailable"]
            t1 += any(s == "replace" for _, _, s in av)
            mism += any(o == "mismatch" for _, o, _ in av)
        for r in wrong:
            av = [derive(x, **kw) for x in r["runs"] if x.get("outcome") != "unavailable"]
            t2 += any(o == "match" for o, _ in av)
            caught += any(o == "mismatch" for o, _ in av)
        n_r, n_w = len(right), len(wrong)
        t2r = (t2 / n_w) if n_w else 0.0
        rows.append({
            "value": v, "today": abs(v - today) < 1e-9,
            "t1": t1, "t1_rate": (t1 / n_r) if n_r else None,
            "t2": t2, "t2_rate": t2r if n_w else None,
            "t3_mismatch": mism, "t3_mismatch_rate": (mism / n_r) if n_r else None,
            "coverage": (cov / nruns) if nruns else None,
            "wrong_caught": (caught / n_w) if n_w else None,
            "safe": t1 == 0 and t2r <= SLO["t2_rate_max"],
        })
    return rows


def recommend(rows, today):
    safe = [r for r in rows if r["safe"]]
    if not safe:
        return {"value": None, "note": "no value in the sweep is safe (T1 = 0 and T2 <= 1%)"}
    best = max(safe, key=lambda r: (r["coverage"] or 0, r["wrong_caught"] or 0, -abs(r["value"] - today)))
    idx = rows.index(best)
    gaps = [abs(rows.index(r) - idx) for r in rows if not r["safe"]]
    return {"value": best["value"], "headroom_steps": min(gaps) if gaps else None,
            "today_is_safe": any(r["today"] and r["safe"] for r in rows),
            "note": "maximizes coverage (then wrong files caught, then closeness to today's) inside the safe region"}


def same_sweep(results, truths, param, values, today):
    right = [r for r in results if truths[r["id"]] == "right"]
    bad = [r for r in results if truths[r["id"]] in ("wrong", "version")]
    rows = []
    for k in values:
        kw = {"same_pct": float(k)} if param == "same" else {"differs_pct": float(k)}
        t1 = sum(same_verdict(r, **kw) == "same" for r in bad)
        miss = sum(same_verdict(r, **kw) != "same" for r in right)
        dr = sum(same_verdict(r, **kw) == "differs" for r in right)
        db = sum(same_verdict(r, **kw) == "differs" for r in bad)
        rows.append({"value": k / 100.0, "today": float(k) == today, "t1": t1,
                     "t1_rate": (t1 / len(bad)) if bad else None,
                     "t3_missed": miss, "t3_missed_rate": (miss / len(right)) if right else None,
                     "differs_on_right": dr, "differs_on_wrong": db, "safe": t1 == 0})
    return rows


def recommend_same(rows, param, today):
    if param == "differs":
        return {"value": today / 100.0, "note": "DIFFERS_BELOW does not change what may replace a file (only differs vs unsure); keep today's"}
    safe = [r for r in rows if r["safe"]]
    if not safe:
        return {"value": None, "note": "no SAME value gives T1 = 0"}
    best = min(safe, key=lambda r: (r["t3_missed"], abs(r["value"] * 100 - today)))
    idx = rows.index(best)
    gaps = [abs(rows.index(r) - idx) for r in rows if not r["safe"]]
    return {"value": best["value"], "headroom_steps": min(gaps) if gaps else None,
            "today_is_safe": any(r["today"] and r["safe"] for r in rows),
            "note": "fewest missed upgrades with no wrong/version file taken as the same, then closest to today's"}


# ---- analysis ----------------------------------------------------------------------------------
def analyse(run, bench):
    sig = load_json(run / "signals.json") or {}
    cases = load_cases(bench)
    labels, labels_src = load_labels(run, bench)
    infos, raws = [], {}
    for res in sig.get("results", []):
        if not isinstance(res, dict) or "id" not in res:
            continue
        info, full = classify(res, cases.get(res["id"], {}), labels)
        infos.append(info)
        raws[info["id"]] = full

    out = {"run": run.name, "manifest": sig.get("manifest"), "labels_source": labels_src,
           "constants": {"MATCH_AT_MOST": MATCH_AT_MOST, "MISMATCH_FROM": MISMATCH_FROM,
                         "PREVIEW_REPLACE_FROM": PREVIEW_REPLACE_FROM, "SAME_PERCENT": SAME_PERCENT,
                         "DIFFERS_BELOW_PERCENT": DIFFERS_BELOW_PERCENT}}
    out["truth_counts"] = {
        "cases": len(infos),
        "excluded": sum(i["truth"] is None for i in infos),
        "presumed_right": sum(i["presumed"] for i in infos),
        "labelled": sum(i["label"] is not None for i in infos),
        "by_truth": {t: sum(i["truth"] == t for i in infos) for t in ("right", "wrong", "version")},
    }
    out["metrics"] = {"labelled_and_constructed": metrics_block(infos, False),
                      "including_presumed": metrics_block(infos, True)}
    out["slo"] = {}
    rows_by = {}
    for v, block in out["metrics"].items():
        rows = slo_rows(block)
        rows_by[v] = rows
        out["slo"][v] = [{"name": n, "pass": ok, "detail": d} for n, ok, d in rows]
    verdict, failing = verdict_of(rows_by)
    out["verdict"] = {"text": verdict, "failing": failing,
                      "rule": "T1, T2, stability and coverage must pass in both variants"}

    # robustness and performance
    prev_runs = [(i, r) for i in infos if i["kind"] == "preview" for r in (raws[i["id"]].get("runs") or []) if isinstance(r, dict)]
    whys = {}
    for _, r in prev_runs:
        if r.get("outcome") == "unavailable":
            w = str(r.get("why") or "unknown")
            whys[w] = whys.get(w, 0) + 1
    out["robustness"] = {
        "runs": len(prev_runs),
        "unavailable_why": {w: rate(c, len(prev_runs)) for w, c in sorted(whys.items())},
        "unavailable_total": rate(sum(whys.values()), len(prev_runs)),
        "unavailable_counted_as_harm": 0,
        "anomalies": [{"id": i["id"], "what": a} for i in infos for a in i["anomalies"]],
        "flaky_recorded_vs_computed_disagree": [i["id"] for i in infos
                                                if i["kind"] == "preview" and i["flaky_recorded"] != i["flaky_computed"]],
    }
    out["performance"] = {
        "preview": {"p50": percentile([num(r.get("seconds_taken")) for _, r in prev_runs], 0.5),
                    "p95": percentile([num(r.get("seconds_taken")) for _, r in prev_runs], 0.95)},
        "same_recording": {"p50": percentile([num(raws[i["id"]].get("seconds_taken")) for i in infos if i["kind"] == "same_recording"], 0.5),
                           "p95": percentile([num(raws[i["id"]].get("seconds_taken")) for i in infos if i["kind"] == "same_recording"], 0.95)},
    }

    # re-derivation at today's constants
    diffs = []
    for i, r in prev_runs:
        if num(r.get("bit_error")) is None or r.get("outcome") == "unavailable":
            continue
        o, s = derive(r)
        if o != r.get("outcome") or (r.get("sweep") is not None and s != r.get("sweep")):
            diffs.append({"id": i["id"], "recorded": [r.get("outcome"), r.get("sweep")], "derived": [o, s]})
    sdiffs = []
    for i in infos:
        if i["kind"] == "same_recording":
            res = raws[i["id"]]
            if num(res.get("score")) is not None and same_verdict(res) != res.get("verdict"):
                sdiffs.append({"id": i["id"], "recorded": res.get("verdict"), "derived": same_verdict(res)})
    out["rederivation"] = {"preview_mismatches": diffs, "same_recording_mismatches": sdiffs}

    # sweeps
    out["sweeps"] = {}
    for variant, inc in (("labelled_and_constructed", False), ("including_presumed", True)):
        use_prev = [raws[i["id"]] for i in infos if i["kind"] == "preview" and i["truth"] in ("right", "wrong") and (inc or not i["presumed"])]
        use_prev = [dict(r, runs=[x for x in (r.get("runs") or []) if isinstance(x, dict)]) for r in use_prev]
        tr = {i["id"]: i["truth"] for i in infos}
        use_same = [raws[i["id"]] for i in infos if i["kind"] == "same_recording" and i["truth"] is not None]
        v = {}
        for param, lo, hi, today in (("match_at", 0.15, 0.35, MATCH_AT_MOST),
                                     ("mismatch_from", 0.30, 0.50, MISMATCH_FROM),
                                     ("replace_from", 0.35, 0.55, PREVIEW_REPLACE_FROM)):
            rows = preview_sweep(use_prev, tr, param, frange(lo, hi), today)
            v["preview_" + param] = {"today": today, "rows": rows, "recommended": recommend(rows, today)}
        for param, lo, hi, today in (("same", 75, 95, SAME_PERCENT), ("differs", 50, 70, DIFFERS_BELOW_PERCENT)):
            rows = same_sweep(use_same, tr, param, list(range(lo, hi + 1)), today)
            v["same_" + param] = {"today": today / 100.0, "rows": rows, "recommended": recommend_same(rows, param, today)}
        out["sweeps"][variant] = v
    out["sweeps_note"] = "Each constant is swept with the others held at today's values; the recommendation uses the including_presumed variant."

    # histogram of bit_error by truth (per run)
    edges = [k / 50.0 for k in range(0, 31)]
    hist = {t: [0] * 30 for t in ("right", "wrong", "version")}
    for i, r in prev_runs:
        be = num(r.get("bit_error"))
        if be is not None and i["truth"] in hist:
            hist[i["truth"]][min(29, int(be * 50))] += 1
    out["histogram"] = {"bin_width": 0.02, "edges": edges, "counts": hist, "last_bin_open": True}

    # tags and case lists
    flagged = [i for i in infos if "tags" in i]
    tag_counts = {}
    for i in flagged:
        for t in set(i["tags"]):
            tag_counts[t] = tag_counts.get(t, 0) + 1
    out["root_causes"] = dict(sorted(tag_counts.items(), key=lambda kv: (-kv[1], kv[0])))
    out["cases"] = {
        "t1": [i["id"] for i in infos if i["t1"]],
        "t2": [i["id"] for i in infos if i["t2"]],
        "version_match": [i["id"] for i in infos if i["version_match"]],
        "flaky": [i["id"] for i in infos if i["flaky"]],
        "flagged": [{k: i[k] for k in ("id", "kind", "group", "truth", "label", "presumed", "tier", "grade", "tags", "what")} for i in flagged],
    }
    out["tiers"] = {i["id"]: {"tier": i["tier"], "grade": i["grade"]} for i in infos}
    return out


# ---- baseline ----------------------------------------------------------------------------------
def flatten(metrics):
    flat = {}

    def walk(node, path):
        if isinstance(node, dict):
            if "rate" in node and "n" in node:
                flat["/".join(path)] = node
            else:
                for k, v in node.items():
                    walk(v, path + [k])
    walk(metrics, [])
    return flat


def compare(now, base):
    fn, fb = flatten(now["metrics"]), flatten(base["metrics"])
    deltas = []
    for k in sorted(set(fn) | set(fb)):
        a, b = fn.get(k), fb.get(k)
        ra = a["rate"] if a else None
        rb = b["rate"] if b else None
        deltas.append({"metric": k, "now": ra, "base": rb,
                       "delta_rate": (ra - rb) if ra is not None and rb is not None else None,
                       "now_count": a["count"] if a else None, "base_count": b["count"] if b else None,
                       "delta_count": (a["count"] - b["count"]) if a and b else None})
    changed = []
    for cid in sorted(set(now["tiers"]) | set(base["tiers"])):
        x, y = now["tiers"].get(cid), base["tiers"].get(cid)
        if x != y:
            changed.append({"id": cid, "base": y, "now": x})
    return {"baseline_run": base["run"], "metric_deltas": deltas, "changed_cases": changed}


# ---- rendering: summary.md ---------------------------------------------------------------------
def pc(x, d=2):
    return "n/a" if x is None else "%.*f%%" % (d, 100 * x)


def mark(ok):
    return "PASS" if ok is True else "FAIL" if ok is False else "NO DATA"


def render_md(a):
    L = ["# Verify bench analysis", "", "Run: `%s`" % a["run"], ""]
    tc = a["truth_counts"]
    L += ["Cases: %d (right %d, wrong %d, version %d, excluded %d); labelled %d; presumed right %d." % (
        tc["cases"], tc["by_truth"]["right"], tc["by_truth"]["wrong"], tc["by_truth"]["version"],
        tc["excluded"], tc["labelled"], tc["presumed_right"]), ""]
    L += ["## Verdict: %s" % a["verdict"]["text"], ""]
    if a["verdict"]["failing"]:
        L += ["Failing: " + "; ".join(a["verdict"]["failing"]), ""]
    L += ["## SLO", ""]
    v1, v2 = "labelled_and_constructed", "including_presumed"
    L += ["| SLO | labelled + constructed only | including presumed right |", "|---|---|---|"]
    for x, y in zip(a["slo"][v1], a["slo"][v2]):
        L.append("| %s | %s %s | %s %s |" % (x["name"], mark(x["pass"]), x["detail"], mark(y["pass"]), y["detail"]))
    L += ["", "## Top root causes", ""]
    if a["root_causes"]:
        for t, c in list(a["root_causes"].items())[:8]:
            L.append("- %s: %d cases" % (t, c))
    else:
        L.append("- none")
    L += ["", "## Thresholds: recommended vs today's", "", "| constant | today | recommended | headroom (steps) | note |", "|---|---|---|---|---|"]
    names = {"preview_match_at": "MATCH_AT_MOST", "preview_mismatch_from": "MISMATCH_FROM",
             "preview_replace_from": "PREVIEW_REPLACE_FROM", "same_same": "SAME", "same_differs": "DIFFERS_BELOW"}
    for k, s in a["sweeps"][v2].items():
        r = s["recommended"]
        L.append("| %s | %s | %s | %s | %s |" % (names[k], s["today"], r["value"], r.get("headroom_steps"), r["note"]))
    L += ["", "## T1 / T2 cases", ""]
    L.append("- T1: " + (", ".join(a["cases"]["t1"]) or "none"))
    L.append("- T2: " + (", ".join(a["cases"]["t2"]) or "none"))
    if a["cases"]["version_match"]:
        L.append("- Version files heard as a match (reported, not T2): " + ", ".join(a["cases"]["version_match"]))
    rd = a["rederivation"]
    L += ["", "Re-derivation at today's constants: %d preview and %d same_recording differences from the recorded values." % (
        len(rd["preview_mismatches"]), len(rd["same_recording_mismatches"]))]
    if "baseline" in a:
        b = a["baseline"]
        L += ["", "## Baseline `%s`" % b["baseline_run"], "", "| metric | base | now | delta |", "|---|---|---|---|"]
        for d in b["metric_deltas"]:
            if d["delta_rate"]:
                L.append("| %s | %s | %s | %+.2f pp |" % (d["metric"], pc(d["base"]), pc(d["now"]), 100 * d["delta_rate"]))
        L += ["", "Cases whose grade or tier changed: %d" % len(b["changed_cases"])]
        for c in b["changed_cases"]:
            L.append("- %s: %s -> %s" % (c["id"], c["base"], c["now"]))
    return "\n".join(L) + "\n"


# ---- rendering: html ---------------------------------------------------------------------------
E = html.escape
CSS = """
:root{--bg:#fff;--fg:#1b1f24;--mut:#5c6670;--line:#d5dae0;--card:#f6f8fa;--s0:#0072B2;--s1:#D55E00;--s2:#CC79A7;--s3:#009E73;--safe:#009E7326;--ok:#0a7a3d;--bad:#b3261e}
@media (prefers-color-scheme:dark){:root{--bg:#14181c;--fg:#e6e9ec;--mut:#9aa5b0;--line:#313840;--card:#1c2127;--s0:#56B4E9;--s1:#E69F00;--s2:#CC79A7;--s3:#3ddba3;--safe:#3ddba326;--ok:#5fd68a;--bad:#ff8a80}}
body{background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif;margin:0;padding:16px;max-width:980px;margin-inline:auto}
h1,h2{line-height:1.2}table{border-collapse:collapse;width:100%;margin:8px 0;font-size:14px}
th,td{border-bottom:1px solid var(--line);padding:4px 8px;text-align:left;vertical-align:top}
.pass{color:var(--ok);font-weight:600}.fail{color:var(--bad);font-weight:600}.nd{color:var(--mut)}
.card{background:var(--card);border-radius:8px;padding:8px 12px;margin:12px 0;overflow-x:auto}
svg{width:100%;height:auto;max-width:680px}svg text{fill:var(--fg);font-size:11px}
.ax{stroke:var(--mut);stroke-width:1}.grid{stroke:var(--line);stroke-width:1}.tod{stroke:var(--mut);stroke-dasharray:4 3}
.safe{fill:var(--safe)}.l0{stroke:var(--s0)}.l1{stroke:var(--s1)}.l2{stroke:var(--s2)}.l3{stroke:var(--s3)}
.f0{fill:var(--s0)}.f1{fill:var(--s1)}.f2{fill:var(--s2)}.f3{fill:var(--s3)}
.ln{fill:none;stroke-width:2}.d1{stroke-dasharray:6 3}.d2{stroke-dasharray:2 3}.d3{stroke-dasharray:8 3 2 3}
code{background:var(--card);padding:0 4px;border-radius:4px}
.lg{display:flex;gap:12px;flex-wrap:wrap;font-size:13px}
"""


def line_chart(title, xs, series, today, safe_x, xlab, ymax=1.0):
    W, H, ml, mr, mt, mb = 680, 280, 44, 12, 12, 40
    pw, ph = W - ml - mr, H - mt - mb
    x0, x1 = xs[0], xs[-1]

    def X(v):
        return ml + (v - x0) / (x1 - x0) * pw

    def Y(v):
        return mt + ph - min(v, ymax) / ymax * ph
    s = ['<svg viewBox="0 0 %d %d" role="img" aria-label="%s">' % (W, H, E(title))]
    for k in range(0, 5):
        yv = ymax * k / 4
        s.append('<line class="grid" x1="%d" x2="%d" y1="%.1f" y2="%.1f"/><text x="%d" y="%.1f" text-anchor="end">%d%%</text>' % (
            ml, W - mr, Y(yv), Y(yv), ml - 4, Y(yv) + 4, round(yv * 100)))
    # safe region
    runs, cur = [], None
    for x, ok in zip(xs, safe_x):
        if ok and cur is None:
            cur = [x, x]
        elif ok:
            cur[1] = x
        elif cur:
            runs.append(cur)
            cur = None
    if cur:
        runs.append(cur)
    for a, b in runs:
        s.append('<rect class="safe" x="%.1f" y="%d" width="%.1f" height="%d"/>' % (X(a), mt, max(2, X(b) - X(a)), ph))
    for k in range(0, 6):
        xv = x0 + (x1 - x0) * k / 5
        s.append('<text x="%.1f" y="%d" text-anchor="middle">%.2f</text>' % (X(xv), H - mb + 14, xv))
    s.append('<line class="ax" x1="%d" x2="%d" y1="%d" y2="%d"/><line class="ax" x1="%d" x2="%d" y1="%d" y2="%d"/>' % (
        ml, W - mr, mt + ph, mt + ph, ml, ml, mt, mt + ph))
    s.append('<line class="tod" x1="%.1f" x2="%.1f" y1="%d" y2="%d"/><text x="%.1f" y="%d">today %.2f</text>' % (
        X(today), X(today), mt, mt + ph, X(today) + 3, mt + 10, today))
    s.append('<text x="%d" y="%d" text-anchor="middle">%s</text>' % (ml + pw / 2, H - 6, E(xlab)))
    for idx, (label, ys) in enumerate(series):
        pts = " ".join("%.1f,%.1f" % (X(x), Y(y)) for x, y in zip(xs, ys) if y is not None)
        if pts:
            s.append('<polyline class="ln l%d d%d" points="%s"/>' % (idx, idx, pts))
    s.append("</svg>")
    leg = '<div class="lg">' + "".join(
        '<span><svg viewBox="0 0 24 8" width="24" height="8"><line class="ln l%d d%d" x1="0" x2="24" y1="4" y2="4"/></svg> %s</span>' % (i, i, E(l))
        for i, (l, _) in enumerate(series)) + '<span><svg viewBox="0 0 24 8" width="24" height="8"><rect class="safe" width="24" height="8"/></svg> safe region</span></div>'
    return "<h3>%s</h3>%s%s" % (E(title), "".join(s), leg)


def hist_chart(h, thresholds):
    W, H, ml, mr, mt, mb = 680, 280, 40, 12, 12, 40
    pw, ph = W - ml - mr, H - mt - mb
    n = len(h["counts"]["right"])
    tot = [sum(h["counts"][t][b] for t in h["counts"]) for b in range(n)]
    ymax = max(tot + [1])
    bw = pw / n
    s = ['<svg viewBox="0 0 %d %d" role="img" aria-label="bit error histogram">' % (W, H)]
    for k in range(0, 5):
        yv = ymax * k / 4
        y = mt + ph - yv / ymax * ph
        s.append('<line class="grid" x1="%d" x2="%d" y1="%.1f" y2="%.1f"/><text x="%d" y="%.1f" text-anchor="end">%d</text>' % (ml, W - mr, y, y, ml - 4, y + 4, round(yv)))
    for b in range(n):
        base = 0
        for idx, t in enumerate(("right", "wrong", "version")):
            c = h["counts"][t][b]
            if c:
                hh = c / ymax * ph
                s.append('<rect class="f%d" x="%.1f" y="%.1f" width="%.1f" height="%.1f"><title>%s %.2f-%.2f: %d</title></rect>' % (
                    [0, 1, 2][idx], ml + b * bw + 1, mt + ph - base - hh, bw - 2, hh, t, b * 0.02, (b + 1) * 0.02, c))
                base += hh
    for k in range(0, 7):
        xv = 0.1 * k
        s.append('<text x="%.1f" y="%d" text-anchor="middle">%.1f</text>' % (ml + xv / 0.6 * pw, H - mb + 14, xv))
    s.append('<line class="ax" x1="%d" x2="%d" y1="%d" y2="%d"/>' % (ml, W - mr, mt + ph, mt + ph))
    for i, (name, v) in enumerate(thresholds):
        x = ml + v / 0.6 * pw
        s.append('<line class="tod" x1="%.1f" x2="%.1f" y1="%d" y2="%d"/><text x="%.1f" y="%d">%s %.2f</text>' % (x, x, mt, mt + ph, x + 3, mt + 10 + 12 * i, name, v))
    s.append('<text x="%d" y="%d" text-anchor="middle">bit_error per run (last bin is 0.58 and up)</text></svg>' % (ml + pw / 2, H - 6))
    leg = '<div class="lg"><span style="color:var(--s0)">&#9632; right</span><span style="color:var(--s1)">&#9632; wrong</span><span style="color:var(--s2)">&#9632; version</span></div>'
    return "<h3>bit_error by truth</h3>" + "".join(s) + leg


def render_html(a):
    v2 = "including_presumed"
    P = ['<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">',
         "<title>Verify bench analysis</title><style>%s</style></head><body>" % CSS,
         "<h1>Verify bench analysis</h1><p>Run <code>%s</code></p>" % E(a["run"])]
    cls = "pass" if not a["verdict"]["failing"] else "fail"
    P.append('<h2>Verdict: <span class="%s">%s</span></h2>' % (cls, E(a["verdict"]["text"])))
    if a["verdict"]["failing"]:
        P.append("<p>Failing: %s</p>" % E("; ".join(a["verdict"]["failing"])))
    P.append('<div class="card"><table><tr><th>SLO</th><th>labelled + constructed</th><th>including presumed right</th></tr>')
    for x, y in zip(a["slo"]["labelled_and_constructed"], a["slo"][v2]):
        def cell(z):
            c = "pass" if z["pass"] is True else "fail" if z["pass"] is False else "nd"
            return '<span class="%s">%s</span> %s' % (c, mark(z["pass"]), E(z["detail"]))
        P.append("<tr><td>%s</td><td>%s</td><td>%s</td></tr>" % (E(x["name"]), cell(x), cell(y)))
    P.append("</table></div>")
    sw = a["sweeps"][v2]
    names = {"preview_match_at": "MATCH_AT_MOST", "preview_mismatch_from": "MISMATCH_FROM",
             "preview_replace_from": "PREVIEW_REPLACE_FROM", "same_same": "SAME", "same_differs": "DIFFERS_BELOW"}
    P.append('<h2>Thresholds</h2><div class="card"><table><tr><th>constant</th><th>today</th><th>recommended</th><th>headroom</th><th>note</th></tr>')
    for k, s in sw.items():
        r = s["recommended"]
        P.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td></tr>" % (names[k], s["today"], r["value"], r.get("headroom_steps"), E(r["note"])))
    P.append("</table></div><h2>Charts</h2>")
    P.append(hist_chart(a["histogram"], [("match", MATCH_AT_MOST), ("mismatch", MISMATCH_FROM), ("replace", PREVIEW_REPLACE_FROM)]))
    for k in ("preview_match_at", "preview_mismatch_from", "preview_replace_from"):
        rows = sw[k]["rows"]
        xs = [r["value"] for r in rows]
        series = [("T2 silent harm (of wrong)", [r["t2_rate"] for r in rows]),
                  ("T3 mismatch on right", [r["t3_mismatch_rate"] for r in rows]),
                  ("coverage", [r["coverage"] for r in rows]),
                  ("T1 destructive (of right)", [r["t1_rate"] for r in rows])]
        P.append(line_chart(names[k], xs, series, sw[k]["today"], [r["safe"] for r in rows], names[k]))
    for k in ("same_same", "same_differs"):
        rows = sw[k]["rows"]
        xs = [r["value"] for r in rows]
        series = [("T1 destructive (of wrong/version)", [r["t1_rate"] for r in rows]),
                  ("T3 missed upgrade (of right)", [r["t3_missed_rate"] for r in rows])]
        P.append(line_chart(names[k], xs, series, sw[k]["today"], [r["safe"] for r in rows], names[k] + " (score)"))
    P.append("<h2>Root causes</h2><div class='card'><table><tr><th>tag</th><th>cases</th></tr>")
    for t, c in a["root_causes"].items():
        P.append("<tr><td>%s</td><td>%d</td></tr>" % (E(t), c))
    P.append("</table></div><h2>Cases that need a look</h2><div class='card'><table><tr><th>tier</th><th>what happened</th><th>tags</th></tr>")
    for c in a["cases"]["flagged"]:
        P.append("<tr><td>%s</td><td>%s</td><td>%s</td></tr>" % (E(c["tier"]), E(c["what"]), E(", ".join(c["tags"]))))
    P.append("</table></div>")
    rb = a["robustness"]
    P.append("<h2>Robustness and performance</h2><div class='card'><p>Unavailable runs: %d of %d; never counted as T1 or T2.</p><ul>" % (
        rb["unavailable_total"]["count"], rb["runs"]))
    for w, r in rb["unavailable_why"].items():
        P.append("<li>%s: %d (%s)</li>" % (E(w), r["count"], pc(r["rate"])))
    P.append("</ul><p>seconds_taken: %s</p></div>" % E(json.dumps(a["performance"])))
    if "baseline" in a:
        b = a["baseline"]
        P.append("<h2>Baseline <code>%s</code></h2><div class='card'><table><tr><th>metric</th><th>base</th><th>now</th><th>delta</th></tr>" % E(b["baseline_run"]))
        for d in b["metric_deltas"]:
            if d["delta_rate"]:
                P.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%+.2f pp</td></tr>" % (E(d["metric"]), pc(d["base"]), pc(d["now"]), 100 * d["delta_rate"]))
        P.append("</table><p>Changed cases: %d</p><ul>" % len(b["changed_cases"]))
        for c in b["changed_cases"]:
            P.append("<li>%s: %s &rarr; %s</li>" % (E(c["id"]), E(json.dumps(c["base"])), E(json.dumps(c["now"]))))
        P.append("</ul></div>")
    P.append("</body></html>")
    return "".join(P)


# ---- main --------------------------------------------------------------------------------------
def main(argv=None):
    ap = argparse.ArgumentParser(description="Analyse a verify bench run.")
    ap.add_argument("--bench", type=Path, default=Path(__file__).resolve().parents[2] / "bench-data")
    ap.add_argument("--run", type=Path)
    ap.add_argument("--baseline", type=Path)
    args = ap.parse_args(argv)
    bench = args.bench
    run = args.run or newest_run(bench)
    if run is None or not (run / "signals.json").is_file():
        print("no run with a signals.json found (use --run)", file=sys.stderr)
        return 2
    a = analyse(run, bench)
    if args.baseline:
        if not (args.baseline / "signals.json").is_file():
            print("baseline has no signals.json", file=sys.stderr)
            return 2
        a["baseline"] = compare(a, analyse(args.baseline, bench))
    (run / "analysis.json").write_text(json.dumps(a, indent=2), encoding="utf-8")
    (run / "summary.md").write_text(render_md(a), encoding="utf-8")
    (run / "analysis.html").write_text(render_html(a), encoding="utf-8")
    print("verdict: %s" % a["verdict"]["text"])
    print("T1: %s  T2: %s" % (a["cases"]["t1"] or "none", a["cases"]["t2"] or "none"))
    print("wrote %s/{analysis.json,summary.md,analysis.html}" % run)
    return 0


if __name__ == "__main__":
    sys.exit(main())
