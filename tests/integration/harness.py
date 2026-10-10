"""Shared helpers for the seed script and the tests. Standard library only."""
import hashlib
import json
import os
import secrets
import time
import urllib.error
import urllib.parse
import urllib.request

NAVIDROME = os.environ.get("HARNESS_NAVIDROME_URL", "http://127.0.0.1:14533")
OCTO = os.environ.get("HARNESS_OCTO_URL", "http://127.0.0.1:15274")
ADMIN = (os.environ.get("HARNESS_ADMIN_USER", "admin"),
         os.environ.get("HARNESS_ADMIN_PASSWORD", "harness-admin-pw"))
# Throwaway accounts that exist only inside the disposable test stack.
USERS = {"alice": "alice-test-pw", "bob": "bob-test-pw", "carol": "carol-test-pw"}


class SubsonicError(Exception):
    pass


def http_json(method, url, body=None, headers=None, timeout=30):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
                                 headers={"Content-Type": "application/json", **(headers or {})})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        raw = r.read()
    return json.loads(raw) if raw else None


def http_ok(url):
    with urllib.request.urlopen(url, timeout=10) as r:
        return r.status == 200


class Subsonic:
    """A Subsonic API client signed in as one user, talking to `base` (Octo or Navidrome)."""

    def __init__(self, base, user, password):
        self.base, self.user, self.password = base.rstrip("/"), user, password

    def call(self, endpoint, params=None):
        salt = secrets.token_hex(6)
        query = [("u", self.user), ("t", hashlib.md5((self.password + salt).encode()).hexdigest()),
                 ("s", salt), ("v", "1.16.1"), ("c", "fla12-harness"), ("f", "json")]
        for key, value in (params or {}).items():
            for item in (value if isinstance(value, (list, tuple)) else [value]):
                query.append((key, str(item)))
        url = f"{self.base}/rest/{endpoint}?{urllib.parse.urlencode(query)}"
        with urllib.request.urlopen(url, timeout=60) as r:
            payload = json.loads(r.read())["subsonic-response"]
        if payload.get("status") != "ok":
            raise SubsonicError(f"{endpoint}: {payload.get('error')}")
        return payload

    def all_songs(self):
        songs, offset = [], 0
        while True:
            page = self.call("search3", {"query": "", "songCount": 100, "songOffset": offset,
                                         "artistCount": 0, "albumCount": 0}).get("searchResult3", {}).get("song", [])
            songs += page
            if len(page) < 100:
                return songs
            offset += 100

    def playlists(self):
        return self.call("getPlaylists").get("playlists", {}).get("playlist", [])

    def playlist(self, playlist_id):
        return self.call("getPlaylist", {"id": playlist_id})["playlist"]

    def create_playlist(self, name, song_ids):
        return self.call("createPlaylist", {"name": name, "songId": song_ids})["playlist"]

    def starred_ids(self):
        return [s["id"] for s in self.call("getStarred2").get("starred2", {}).get("song", [])]


def wait_for(fn, what, seconds=180):
    deadline, last = time.time() + seconds, None
    while time.time() < deadline:
        try:
            if fn():
                return
        except Exception as e:  # not up yet
            last = e
        time.sleep(2)
    raise TimeoutError(f"timed out waiting for {what} ({last})")
