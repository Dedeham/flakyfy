#!/usr/bin/env python3
"""Human labelling page for the verify bench.

Serves a single local page on 127.0.0.1 where a person listens to a library file next to
the preview Octo compared it with (or a candidate upgrade next to the original) and says
what it really is. Reads <run>/label-queue.json, writes <run>/labels.json.

    python3 -I label_server.py [--bench <checkout>/bench-data] [--run <run dir>] [--port 8765]

Python standard library plus ffmpeg only. The page needs no network: everything is inline.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import threading
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

LABELS = ("right", "wrong", "version", "preview_wrong", "cant_tell", "skip")
MAX_NOTE = 2000
MAX_BODY = 64 * 1024
MIN_LEN, MAX_LEN, DEFAULT_LEN = 1.0, 120.0, 20.0

LOCK = threading.RLock()
CLIP_LOCK = threading.Lock()


class Ctx:
    bench: Path
    run: Path
    port: int
    clip_ext = "webm"
    clip_mime = "audio/webm"


class HttpError(Exception):
    def __init__(self, code, msg):
        super().__init__(msg)
        self.code = code
        self.msg = msg


# ---------------------------------------------------------------- data

def read_json(path, default=None):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        if default is not None:
            return default
        raise HttpError(500, "missing file: %s" % path.name)
    except (OSError, ValueError) as e:
        raise HttpError(500, "cannot read %s: %s" % (path.name, e))


def load_queue():
    q = read_json(Ctx.run / "label-queue.json")
    items = q.get("items") if isinstance(q, dict) else None
    if not isinstance(items, list):
        raise HttpError(500, "label-queue.json has no items list")
    return q, [i for i in items if isinstance(i, dict) and isinstance(i.get("case_id"), str)]


def load_labels():
    data = read_json(Ctx.run / "labels.json", default={"labels": {}})
    labels = data.get("labels") if isinstance(data, dict) else None
    return labels if isinstance(labels, dict) else {}


def write_atomic(path, text, mode=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=path.name + ".", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(text)
            f.flush()
            os.fsync(f.fileno())
        if mode is not None:
            os.chmod(tmp, mode)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def save_labels(labels):
    text = json.dumps({"labels": labels}, indent=2, ensure_ascii=False, sort_keys=True) + "\n"
    write_atomic(Ctx.run / "labels.json", text)
    d = Path.home() / ".local" / "share" / "octo-bench"
    d.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(d, 0o700)
    except OSError:
        pass
    # One backup per run and day, so labelling a second queue never overwrites the first.
    day = datetime.now().strftime("%Y-%m-%d")
    write_atomic(d / ("labels-%s-%s.json" % (Ctx.run.name, day)), text, mode=0o600)


def public_item(item, labelled):
    """Hide everything that could bias the person until the item has a label."""
    out = {k: item.get(k) for k in ("case_id", "kind", "request", "preview")}
    out["has_a"] = isinstance(item.get("a"), dict) and bool(item["a"].get("path"))
    out["has_b"] = isinstance(item.get("b"), dict) and bool(item["b"].get("path"))
    if labelled:
        out["reasons"] = item.get("reasons")
        out["priority"] = item.get("priority")
        out["hidden"] = item.get("hidden")
    return out


# ---------------------------------------------------------------- clips

def safe_media_path(rel):
    """Resolve a queue path; it must end up inside the bench folder."""
    if not isinstance(rel, str) or not rel or "\0" in rel:
        raise HttpError(404, "no audio for this side")
    real = os.path.realpath(os.path.join(str(Ctx.bench), rel))
    root = os.path.realpath(str(Ctx.bench))
    if real != root and not real.startswith(root + os.sep):
        raise HttpError(403, "path outside the bench folder")
    if not os.path.isfile(real):
        raise HttpError(404, "audio file not found")
    return real


def clip_for(case_id, side, length, from_start):
    _, items = load_queue()
    item = next((i for i in items if i["case_id"] == case_id), None)
    if item is None:
        raise HttpError(404, "unknown item")
    spec = item.get(side)
    if not isinstance(spec, dict):
        raise HttpError(404, "no audio for this side")
    src = safe_media_path(spec.get("path"))
    try:
        offset = 0.0 if from_start else max(0.0, float(spec.get("offset") or 0))
    except (TypeError, ValueError):
        offset = 0.0
    slug = re.sub(r"[^A-Za-z0-9._-]", "_", case_id)[:80]
    if slug != case_id:
        slug += "_" + hashlib.sha1(case_id.encode()).hexdigest()[:8]
    name = "%s-%s-%g-l%g.%s" % (slug, side, offset, length, Ctx.clip_ext)
    clips = Ctx.run / "clips"
    out = clips / name
    with CLIP_LOCK:
        if not out.is_file() or out.stat().st_size == 0:
            clips.mkdir(parents=True, exist_ok=True)
            tmp = clips / (name + ".part." + Ctx.clip_ext)
            codec = (["-c:a", "libopus", "-b:a", "48k", "-f", "webm"] if Ctx.clip_ext == "webm"
                     else ["-c:a", "libmp3lame", "-b:a", "96k", "-f", "mp3"])
            cmd = ["ffmpeg", "-nostdin", "-v", "error", "-y", "-ss", "%g" % offset, "-t", "%g" % length,
                   "-i", src, "-vn", "-sn", "-dn", "-ac", "1"] + codec + [str(tmp)]
            try:
                r = subprocess.run(cmd, capture_output=True, text=True, timeout=90)
            except FileNotFoundError:
                raise HttpError(500, "ffmpeg is not installed")
            except subprocess.TimeoutExpired:
                raise HttpError(500, "ffmpeg timed out")
            if r.returncode != 0 or not tmp.is_file() or tmp.stat().st_size == 0:
                try:
                    tmp.unlink()
                except OSError:
                    pass
                raise HttpError(500, "ffmpeg failed: " + (r.stderr.strip()[-300:] or "no output"))
            os.replace(tmp, out)
    return out


def detect_codec():
    try:
        r = subprocess.run(["ffmpeg", "-hide_banner", "-encoders"], capture_output=True, text=True, timeout=20)
    except (OSError, subprocess.SubprocessError):
        return
    if "libopus" not in r.stdout:
        Ctx.clip_ext, Ctx.clip_mime = "mp3", "audio/mpeg"


# ---------------------------------------------------------------- http

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "labelserver"
    sys_version = ""

    def log_message(self, fmt, *args):
        sys.stderr.write("%s - %s\n" % (self.log_date_time_string(), fmt % args))

    # -- helpers
    def _allowed_hosts(self):
        return ("127.0.0.1:%d" % Ctx.port, "localhost:%d" % Ctx.port)

    def _headers_common(self):
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        self.send_header("X-Frame-Options", "DENY")

    def _send(self, code, body, ctype, extra=None):
        if isinstance(body, str):
            body = body.encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self._headers_common()
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def _json(self, code, obj):
        self._send(code, json.dumps(obj), "application/json; charset=utf-8")

    def _check_host(self):
        if self.headers.get("Host", "") not in self._allowed_hosts():
            raise HttpError(403, "bad Host header")

    def _check_post(self):
        ctype = self.headers.get("Content-Type", "").split(";")[0].strip().lower()
        if ctype != "application/json":
            raise HttpError(415, "Content-Type must be application/json")
        origin = self.headers.get("Origin")
        if origin is not None and origin not in ["http://" + h for h in self._allowed_hosts()]:
            raise HttpError(403, "bad Origin")

    def _dispatch(self, fn):
        try:
            self._check_host()
            fn()
        except HttpError as e:
            self.close_connection = self.close_connection or e.code >= 500
            self._json(e.code, {"error": e.msg})
        except (BrokenPipeError, ConnectionResetError):
            self.close_connection = True
        except Exception as e:  # shown in the page, never swallowed
            self._json(500, {"error": "%s: %s" % (type(e).__name__, e)})

    # -- verbs
    def do_GET(self):
        self._dispatch(self._get)

    do_HEAD = do_GET

    def do_POST(self):
        self._dispatch(self._post)

    def _get(self):
        u = urlsplit(self.path)
        qs = parse_qs(u.query)
        if u.path == "/":
            self._send(200, PAGE, "text/html; charset=utf-8", {
                "Content-Security-Policy": "default-src 'none'; script-src 'unsafe-inline'; "
                "style-src 'unsafe-inline'; media-src 'self'; connect-src 'self'; base-uri 'none'; "
                "form-action 'none'; frame-ancestors 'none'"})
        elif u.path == "/api/queue":
            self._queue()
        elif u.path == "/clip":
            self._clip(qs)
        else:
            raise HttpError(404, "not found")

    def _queue(self):
        with LOCK:
            q, items = load_queue()
            labels = load_labels()
        ids = {i["case_id"] for i in items}
        done = sum(1 for k in labels if k in ids)
        self._json(200, {
            "run": q.get("run") or Ctx.run.name,
            "items": [public_item(i, i["case_id"] in labels) for i in items],
            "labels": {k: v for k, v in labels.items() if k in ids},
            "progress": {"labelled": done, "total": len(items)},
            "labels_allowed": list(LABELS),
        })

    def _post(self):
        if urlsplit(self.path).path != "/api/label":
            raise HttpError(404, "not found")
        self._check_post()
        try:
            n = int(self.headers.get("Content-Length", ""))
        except ValueError:
            raise HttpError(411, "Content-Length required")
        if n < 0 or n > MAX_BODY:
            raise HttpError(413, "body too large")
        try:
            body = json.loads(self.rfile.read(n).decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            raise HttpError(400, "invalid JSON")
        if not isinstance(body, dict):
            raise HttpError(400, "body must be an object")
        case_id, label, note = body.get("case_id"), body.get("label"), body.get("note", "")
        if label not in LABELS:
            raise HttpError(400, "unknown label")
        if not isinstance(note, str):
            raise HttpError(400, "note must be a string")
        with LOCK:
            _, items = load_queue()
            item = next((i for i in items if i["case_id"] == case_id), None)
            if item is None:
                raise HttpError(404, "unknown case_id")
            labels = load_labels()
            labels[case_id] = {"label": label, "note": note.strip()[:MAX_NOTE],
                               "at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}
            save_labels(labels)
            done = sum(1 for k in labels if k in {i["case_id"] for i in items})
        self._json(200, {"ok": True, "entry": labels[case_id], "item": public_item(item, True),
                         "progress": {"labelled": done, "total": len(items)}})

    def _clip(self, qs):
        case_id = (qs.get("item") or [""])[0]
        side = (qs.get("side") or [""])[0]
        if side not in ("a", "b"):
            raise HttpError(400, "side must be a or b")
        try:
            length = float((qs.get("len") or [DEFAULT_LEN])[0])
        except ValueError:
            raise HttpError(400, "bad len")
        if not (MIN_LEN <= length <= MAX_LEN):
            raise HttpError(400, "len out of range")
        from_start = (qs.get("from") or [""])[0] == "start"
        path = clip_for(case_id, side, length, from_start)
        size = path.stat().st_size
        start, end, code = 0, size - 1, 200
        rng = self.headers.get("Range")
        extra = {"Accept-Ranges": "bytes"}
        if rng:
            m = re.fullmatch(r"bytes=(\d*)-(\d*)", rng.strip())
            if not m or (m.group(1) == "" and m.group(2) == ""):
                raise HttpError(416, "bad range")
            if m.group(1) == "":
                start = max(0, size - int(m.group(2)))
            else:
                start = int(m.group(1))
                if m.group(2):
                    end = min(end, int(m.group(2)))
            if start >= size or start > end:
                self.send_response(416)
                self.send_header("Content-Range", "bytes */%d" % size)
                self.send_header("Content-Length", "0")
                self._headers_common()
                self.end_headers()
                return
            code = 206
            extra["Content-Range"] = "bytes %d-%d/%d" % (start, end, size)
        length_out = end - start + 1
        self.send_response(code)
        self.send_header("Content-Type", Ctx.clip_mime)
        self.send_header("Content-Length", str(length_out))
        self.send_header("Cache-Control", "no-store")
        self._headers_common()
        for k, v in extra.items():
            self.send_header(k, v)
        self.end_headers()
        if self.command == "HEAD":
            return
        with open(path, "rb") as f:
            f.seek(start)
            left = length_out
            while left > 0:
                chunk = f.read(min(65536, left))
                if not chunk:
                    break
                self.wfile.write(chunk)
                left -= len(chunk)


# ---------------------------------------------------------------- page

PAGE = r"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Verify bench labelling</title>
<style>
:root{color-scheme:light dark;--bg:#f6f7f9;--card:#fff;--fg:#1b1f24;--mut:#5d6672;--line:#d6dae0;
--acc:#2457d6;--accfg:#fff;--ok:#1d7a3a;--bad:#b3261e;--warn:#8a5a00;--chip:#eceff3}
@media (prefers-color-scheme:dark){:root{--bg:#14161a;--card:#1d2026;--fg:#e8eaee;--mut:#9aa3b0;
--line:#333944;--acc:#7aa2ff;--accfg:#0b1020;--ok:#5fd085;--bad:#ff8a80;--warn:#f0b64a;--chip:#2a2f38}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.45 system-ui,sans-serif}
main{max-width:860px;margin:0 auto;padding:12px}
header{display:flex;flex-wrap:wrap;gap:8px 16px;align-items:center;justify-content:space-between;margin-bottom:8px}
h1{font-size:1.05rem;margin:0}
.prog{color:var(--mut)}
.bar{height:6px;background:var(--chip);border-radius:3px;overflow:hidden;margin-bottom:12px}
.bar i{display:block;height:100%;background:var(--acc);width:0}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:12px;margin-bottom:12px}
.card h2{font-size:.8rem;text-transform:uppercase;letter-spacing:.05em;color:var(--mut);margin:0 0 6px}
.cols{display:grid;grid-template-columns:1fr 1fr;gap:12px}
@media (max-width:600px){.cols{grid-template-columns:1fr}}
dl{margin:0;display:grid;grid-template-columns:auto 1fr;gap:2px 10px}
dt{color:var(--mut)}dd{margin:0;overflow-wrap:anywhere}
button{font:inherit;color:var(--fg);background:var(--chip);border:1px solid var(--line);border-radius:8px;
padding:10px 14px;min-height:44px;cursor:pointer}
button:hover{border-color:var(--acc)}
button:focus-visible,textarea:focus-visible,select:focus-visible{outline:3px solid var(--acc);outline-offset:1px}
button.on{background:var(--acc);color:var(--accfg);border-color:var(--acc);font-weight:600}
.players{display:grid;grid-template-columns:1fr 1fr;gap:12px}
@media (max-width:600px){.players{grid-template-columns:1fr}}
.row{display:flex;flex-wrap:wrap;gap:8px;align-items:center}
.labels{display:grid;grid-template-columns:repeat(3,1fr);gap:8px}
@media (max-width:600px){.labels{grid-template-columns:repeat(2,1fr)}}
.labels button{min-height:56px;font-size:1.05rem;text-align:left}
kbd{font:.8em ui-monospace,monospace;border:1px solid var(--line);border-radius:4px;padding:0 5px;margin-right:6px;background:var(--card);color:var(--mut)}
.on kbd{color:inherit;background:transparent;border-color:currentColor}
textarea{width:100%;font:inherit;color:var(--fg);background:var(--bg);border:1px solid var(--line);border-radius:8px;padding:8px;min-height:60px}
select{font:inherit;color:var(--fg);background:var(--chip);border:1px solid var(--line);border-radius:8px;padding:8px;min-height:40px}
audio{width:100%;margin-top:8px}
.err{background:color-mix(in srgb,var(--bad) 15%,var(--card));border:1px solid var(--bad);color:var(--fg);
border-radius:8px;padding:10px;margin-bottom:12px;white-space:pre-wrap}
.hint{color:var(--mut);font-size:.9rem}
.saved{color:var(--ok)}
pre{margin:6px 0 0;overflow:auto;max-height:260px;font-size:.85rem;background:var(--bg);padding:8px;border-radius:6px}
details summary{cursor:pointer;color:var(--mut)}
.nav{display:flex;gap:8px;flex-wrap:wrap;justify-content:space-between}
.chip{display:inline-block;background:var(--chip);border-radius:12px;padding:0 8px;margin:0 4px 0 0;font-size:.85rem}
</style>
</head>
<body>
<main>
<header><h1 id="title">Verify bench labelling</h1><div class="prog" id="prog"></div></header>
<div class="bar"><i id="barfill"></i></div>
<div id="errs"></div>
<div id="app" class="hint">Loading...</div>
</main>
<script>
"use strict";
var D = {items: [], labels: {}, progress: {labelled: 0, total: 0}}, idx = 0, len = 20;
var LABELS = ["right", "wrong", "version", "preview_wrong", "cant_tell", "skip"];
var TEXT_PREVIEW = ["right", "wrong", "version", "preview_wrong", "can't tell", "skip"];
var TEXT_PREVIEW_FULL = ["Right song", "Wrong song", "Other version", "Preview is wrong", "Can't tell", "Skip"];
var TEXT_SAME = ["Same recording", "Different song", "Other version", "Original is wrong", "Can't tell", "Skip"];
var players = {a: null, b: null};

function $(id){return document.getElementById(id);}
function el(tag, attrs, kids){
  var e = document.createElement(tag);
  for (var k in (attrs || {})) {
    if (k === "class") e.className = attrs[k];
    else if (k === "text") e.textContent = attrs[k];
    else if (k.slice(0, 2) === "on") e.addEventListener(k.slice(2), attrs[k]);
    else e.setAttribute(k, attrs[k]);
  }
  (kids || []).forEach(function(c){ if (c != null) e.appendChild(typeof c === "string" ? document.createTextNode(c) : c); });
  return e;
}
function showErr(msg){
  var box = el("div", {"class": "err", role: "alert", text: String(msg)});
  box.appendChild(el("button", {text: "Dismiss", onclick: function(){ box.remove(); }}));
  $("errs").appendChild(box);
}
function api(path, opts){
  return fetch(path, opts).then(function(r){
    return r.text().then(function(t){
      var j = null; try { j = JSON.parse(t); } catch (e) {}
      if (!r.ok) throw new Error(path + ": " + r.status + " " + ((j && j.error) || t.slice(0, 200)));
      if (j === null) throw new Error(path + ": response is not JSON");
      return j;
    });
  });
}
function fmtLen(s){
  if (s == null || s === "" || isNaN(Number(s))) return "unknown";
  s = Math.round(Number(s)); return Math.floor(s / 60) + ":" + ("0" + (s % 60)).slice(-2) + " (" + s + " s)";
}
function val(v){ return v == null || v === "" ? "-" : String(v); }
function facts(obj, rows){
  var dl = el("dl");
  rows.forEach(function(r){
    dl.appendChild(el("dt", {text: r[0]}));
    dl.appendChild(el("dd", {text: r[2] ? r[2](obj[r[1]]) : val(obj[r[1]])}));
  });
  return dl;
}
function cur(){ return D.items[idx]; }
function entry(it){ return it && D.labels[it.case_id]; }

function load(first){
  return api("/api/queue").then(function(d){
    D = d;
    if (first) {
      idx = 0;
      for (var i = 0; i < D.items.length; i++) if (!D.labels[D.items[i].case_id]) { idx = i; break; }
      if (D.items.length && D.progress.labelled >= D.progress.total) idx = 0;
    }
    render();
  }).catch(function(e){ $("app").textContent = ""; showErr(e.message); });
}

function stopAll(){
  ["a", "b"].forEach(function(s){ if (players[s]) { players[s].pause(); } });
}
function clipUrl(it, side, fromStart){
  return "/clip?item=" + encodeURIComponent(it.case_id) + "&side=" + side + "&len=" + len + (fromStart ? "&from=start" : "");
}
function play(side, fromStart){
  var it = cur(); if (!it) return;
  var a = players[side], other = players[side === "a" ? "b" : "a"];
  if (other) other.pause();
  if (!a) return;
  var url = clipUrl(it, side, fromStart);
  var wanted = location.origin + url;
  if (a.src !== wanted || a.error) { a.src = url; }
  else if (!a.paused && !fromStart) { a.pause(); return; }
  a.currentTime = 0;
  var p = a.play();
  if (p && p.catch) p.catch(function(e){ if (e.name !== "AbortError") probe(url, side, e); });
}
function probe(url, side, e){
  fetch(url, {headers: {Range: "bytes=0-0"}}).then(function(r){
    if (r.ok) { showErr("Cannot play " + side.toUpperCase() + ": " + e.message); return; }
    return r.text().then(function(t){ showErr("Clip " + side.toUpperCase() + ": " + r.status + " " + t.slice(0, 300)); });
  }).catch(function(x){ showErr("Clip " + side.toUpperCase() + ": " + x.message); });
}

function render(){
  var app = $("app"); app.textContent = ""; players = {a: null, b: null};
  var pr = D.progress;
  $("prog").textContent = pr.labelled + " of " + pr.total + " labelled";
  $("barfill").style.width = (pr.total ? 100 * pr.labelled / pr.total : 0) + "%";
  $("title").textContent = "Verify bench labelling" + (D.run ? " - " + D.run : "");
  var it = cur();
  if (!it) { app.appendChild(el("div", {"class": "card", text: "The queue is empty."})); return; }
  var same = it.kind === "same_recording", e = entry(it), req = it.request || {}, pv = it.preview;

  app.appendChild(el("div", {"class": "nav"}, [
    el("button", {text: "Previous (p)", onclick: function(){ go(-1); }}),
    el("div", {"class": "hint", text: "Item " + (idx + 1) + " of " + D.items.length + "  |  " + it.kind + "  |  " + it.case_id}),
    el("button", {text: "Next (n)", onclick: function(){ go(1); }})
  ]));
  app.appendChild(el("div", {style: "height:12px"}));

  var reqRows = [["Artist", "artist"], ["Title", "title"], ["Album", "album"], ["Length", "seconds", fmtLen]];
  var cols = el("div", {"class": "cols"});
  cols.appendChild(el("section", {"class": "card"}, [
    el("h2", {text: same ? "Original (what the library has)" : "Requested (the library file)"}), facts(req, reqRows)]));
  if (same) {
    cols.appendChild(el("section", {"class": "card"}, [
      el("h2", {text: "Candidate upgrade"}), el("div", {"class": "hint", text: "Listen to A (candidate) against B (original)."})]));
  } else {
    var pc = el("section", {"class": "card"}, [el("h2", {text: "Preview Octo compared it with"})]);
    if (pv && typeof pv === "object") {
      pc.appendChild(facts(pv, [["Title", "title"], ["Channel/artist", "artist"], ["Album", "album"],
        ["Length", "seconds", fmtLen], ["Lane", "lane"]]));
    } else pc.appendChild(el("div", {"class": "hint", text: "No preview facts."}));
    cols.appendChild(pc);
  }
  app.appendChild(cols);
  app.appendChild(el("div", {style: "height:12px"}));

  var pl = el("div", {"class": "players"});
  [["a", same ? "A: candidate" : "A: library file", it.has_a], ["b", same ? "B: original" : "B: preview", it.has_b]].forEach(function(s){
    var side = s[0], a = el("audio", {controls: "controls", preload: "none"});
    a.addEventListener("error", function(){ if (a.getAttribute("src")) probe(a.getAttribute("src"), side, new Error("media error")); });
    players[side] = a;
    var box = el("section", {"class": "card"}, [el("h2", {text: s[1]})]);
    if (!s[2]) { box.appendChild(el("div", {"class": "hint", text: "No audio for this side."})); pl.appendChild(box); return; }
    box.appendChild(el("div", {"class": "row"}, [
      el("button", {text: side === "a" ? "Play / pause (space)" : "Play / pause (b)", onclick: function(){ play(side, false); }}),
      el("button", {text: "From start", onclick: function(){ play(side, true); }})]));
    box.appendChild(a);
    pl.appendChild(box);
  });
  app.appendChild(pl);
  var sel = el("select", {id: "len", "aria-label": "Clip length", onchange: function(){ len = Number(this.value); }});
  [10, 20, 40, 60].forEach(function(n){ var o = el("option", {value: n, text: n + " s"}); if (n === len) o.selected = true; sel.appendChild(o); });
  app.appendChild(el("div", {"class": "row hint", style: "margin:8px 0"}, [el("span", {text: "Clip length"}), sel,
    el("span", {text: "Clips start at the place Octo compared."})]));

  var texts = same ? TEXT_SAME : TEXT_PREVIEW_FULL;
  var lb = el("div", {"class": "labels", role: "group", "aria-label": "Label"});
  LABELS.forEach(function(l, i){
    lb.appendChild(el("button", {"class": e && e.label === l ? "on" : "", "data-label": l,
      onclick: function(){ save(l); }}, [el("kbd", {text: String(i + 1)}), texts[i]]));
  });
  var note = el("textarea", {id: "note", placeholder: "Optional note", "aria-label": "Note"});
  note.value = e ? (e.note || "") : "";
  note.addEventListener("keydown", function(ev){ ev.stopPropagation(); });
  var card = el("section", {"class": "card"}, [el("h2", {text: "What is it really?"}), lb,
    el("div", {style: "height:8px"}), note,
    el("div", {"class": "hint", id: "status", text: e ? "Saved: " + e.label + " (change it by choosing again; the note is saved with the label)." :
      "Press 1-6, or n / p to move. Space plays A, b plays B."})]);
  app.appendChild(card);

  if (e) {
    var h = el("section", {"class": "card"}, [el("h2", {text: "Signals (revealed after labelling)"})]);
    if (it.reasons) h.appendChild(el("div", {}, it.reasons.map(function(r){ return el("span", {"class": "chip", text: r}); })));
    var det = el("details", {open: "open"}, [el("summary", {text: "hidden signals, priority " + val(it.priority)}),
      el("pre", {text: JSON.stringify(it.hidden, null, 2)})]);
    h.appendChild(det);
    app.appendChild(h);
  }
}

function go(d){
  if (!D.items.length) return;
  var n = idx + d; if (n < 0 || n >= D.items.length) return;
  stopAll(); idx = n; render();
}
function save(label){
  var it = cur(); if (!it) return;
  var note = $("note") ? $("note").value : "";
  api("/api/label", {method: "POST", headers: {"Content-Type": "application/json"},
    body: JSON.stringify({case_id: it.case_id, label: label, note: note})}).then(function(r){
    D.labels[it.case_id] = r.entry; D.items[idx] = r.item; D.progress = r.progress;
    var keepNote = $("note") ? $("note").value : "";
    render(); $("note").value = keepNote;
    var s = $("status"); s.textContent = "Saved: " + label; s.className = "hint saved";
  }).catch(function(e){ showErr("NOT SAVED: " + e.message); });
}

document.addEventListener("keydown", function(ev){
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
  var t = ev.target && ev.target.tagName;
  if (t === "TEXTAREA" || t === "SELECT") return;
  var k = ev.key;
  if (k >= "1" && k <= "6") { ev.preventDefault(); save(LABELS[Number(k) - 1]); }
  else if (k === "n" || k === "ArrowRight") { ev.preventDefault(); go(1); }
  else if (k === "p" || k === "ArrowLeft") { ev.preventDefault(); go(-1); }
  else if (k === " ") { ev.preventDefault(); play("a", false); }
  else if (k === "b") { ev.preventDefault(); play("b", false); }
});
window.addEventListener("error", function(ev){ showErr("Script error: " + ev.message); });
window.addEventListener("unhandledrejection", function(ev){ showErr("Error: " + (ev.reason && ev.reason.message || ev.reason)); });
load(true);
</script>
</body>
</html>
"""


# ---------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description="Human labelling page for the verify bench.")
    ap.add_argument("--bench", type=Path, default=Path(__file__).resolve().parents[2] / "bench-data")
    ap.add_argument("--run", type=Path, default=None, help="run dir (default: newest under <bench>/runs)")
    ap.add_argument("--port", type=int, default=8765)
    args = ap.parse_args()

    bench = args.bench.resolve()
    if not bench.is_dir():
        sys.exit("bench folder not found: %s" % bench)
    run = args.run
    if run is None:
        runs = sorted(p for p in (bench / "runs").glob("*") if (p / "label-queue.json").is_file())
        if not runs:
            sys.exit("no run with label-queue.json under %s" % (bench / "runs"))
        run = runs[-1]
    run = run.resolve()
    if not (run / "label-queue.json").is_file():
        sys.exit("no label-queue.json in %s" % run)
    Ctx.bench, Ctx.run, Ctx.port = bench, run, args.port
    detect_codec()
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    srv.daemon_threads = True
    print("Labelling %s\nOpen http://127.0.0.1:%d/  (Ctrl-C to stop)" % (run, args.port), flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        srv.server_close()


if __name__ == "__main__":
    main()
