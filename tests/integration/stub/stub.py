#!/usr/bin/env python3
"""Test-only stand-in for the outside services Octo talks to (FLA-21). Never part of a release.

One small HTTP server that answers like:
  * slskd:   session, application state (logged in or not), searches that always end empty
  * the yt-dlp shim: /search and /download (writes a tiny generated MP3 under /music/.octo-incoming)

Control and inspection, for the tests only:
  POST /__control {"soulseek": "up" | "down"}   switch the slskd state
  GET  /__log                                   every request the stub served, as a list

Nothing is looked up on the internet. Standard library only.
"""
import json
import os
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

LOCK = threading.Lock()
STATE = {"soulseek": "up"}
LOG = []

MUSIC_ROOT = os.path.realpath(os.environ.get("MUSIC_ROOT", "/music"))
STAGING = os.path.join(MUSIC_ROOT, ".octo-incoming")
VIDEO_ID = "stubVideo01"  # 11 characters of the video id alphabet


def mp3_bytes():
    """Twenty empty MPEG-1 layer 3 frames: enough for a tag library to call it an MP3."""
    frame = bytes([0xFF, 0xFB, 0x90, 0x64]) + bytes(413)
    return frame * 20


def logged_in_json():
    return {"server": {"state": "Connected, LoggedIn", "isConnected": True, "isConnecting": False,
                       "isLoggedIn": True, "isLoggingIn": False, "isTransitioning": False},
            "user": {"username": "stub"}, "connectionWatchdog": {"isEnabled": True, "isAttemptingConnection": False}}


def logged_out_json():
    return {"server": {"state": "Disconnecting", "isConnected": False, "isLoggedIn": False,
                       "isTransitioning": True},
            "user": {"username": "stub"}, "connectionWatchdog": {"isEnabled": True, "isAttemptingConnection": True}}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):  # keep the container log quiet
        pass

    def send_json(self, body, status=200):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def record(self, kind):
        with LOCK:
            LOG.append({"at": time.time(), "kind": kind, "method": self.command,
                        "path": urlparse(self.path).path})

    def read_body(self):
        length = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(length) if length else b""

    def do_GET(self):
        url = urlparse(self.path)
        path, query = url.path, parse_qs(url.query)
        if path == "/__log":
            with LOCK:
                return self.send_json(list(LOG))
        if path == "/api/v0/application":
            self.record("slskd")
            return self.send_json(logged_in_json() if STATE["soulseek"] == "up" else logged_out_json())
        if path == "/api/v0/options":
            self.record("slskd")
            return self.send_json({"directories": {"downloads": "/music"}})
        if re.fullmatch(r"/api/v0/searches/[^/]+", path):
            self.record("slskd")
            return self.send_json({"state": "Completed, TimedOut", "endedAt": "2026-10-11T00:00:00Z", "responseCount": 0})
        if re.fullmatch(r"/api/v0/searches/[^/]+/responses", path):
            self.record("slskd")
            return self.send_json([])
        if path.startswith("/api/v0/"):
            self.record("slskd")
            return self.send_json({})
        if path == "/search" or path == "/meta":
            self.record("shim-search")
            duration = query.get("duration", [""])[0]
            return self.send_json({"video_id": VIDEO_ID, "title": "stub", "duration": int(duration) if duration.isdigit() else 200})
        if path == "/download":
            self.record("shim-download")
            dest = os.path.realpath(query.get("dest", [""])[0])
            if not dest.startswith(STAGING + os.sep) or query.get("id", [""])[0] != VIDEO_ID:
                return self.send_json({"error": "refused"}, 400)
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            with open(dest + ".mp3", "wb") as fh:
                fh.write(mp3_bytes())
            return self.send_json({"path": dest + ".mp3"})
        self.record("unknown")
        self.send_json({"error": "not found"}, 404)

    def do_POST(self):
        path = urlparse(self.path).path
        body = self.read_body()
        if path == "/__control":
            wanted = json.loads(body or b"{}")
            with LOCK:
                STATE.update({k: v for k, v in wanted.items() if k in STATE})
            return self.send_json(STATE)
        if path == "/api/v0/session":
            self.record("slskd")
            return self.send_json({"token": "stub-token", "expires": int(time.time()) + 3600})
        if path == "/api/v0/searches":
            self.record("slskd")
            return self.send_json({})
        self.record("slskd" if path.startswith("/api/v0/") else "unknown")
        self.send_json({})

    def do_PUT(self):
        self.read_body()
        self.record("slskd")
        self.send_json({})

    def do_DELETE(self):
        self.record("slskd")
        self.send_json({})


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
