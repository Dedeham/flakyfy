#!/usr/bin/env python3
"""Copies a sample of songs out of Navidrome into a bench folder, as the "right" files the
verify bench starts from (see README.md next to this script).

    python3 -I fetch_corpus.py [--bench <checkout>/bench-data] [--songs 60]

Reads <bench>/.env (keep it chmod 600):

    NAVIDROME_URL=http://<LAN address>:4533
    NAVIDROME_USER=<a read-only user>
    NAVIDROME_PASSWORD=<its password>

Only a private IP address is accepted as the host, never a name, so no DNS lookup is made and
nothing leaves the local network. The password is never printed; requests use Subsonic's salted
token. Files are saved by their Navidrome id, so no song name ends up in a file name.

For each picked song it also tries to copy:
- another song by the same artist (the "other song" a wrong file most often is),
- a song by another artist with the same title (a namesake),
- another version of the song by the same artist (live, remaster, acoustic...).
"""

import argparse
import hashlib
import ipaddress
import json
import random
import re
import secrets
import sys
import urllib.parse
import urllib.request
from pathlib import Path

CLIENT = "octo-bench"
# The bench folder inside the checkout; ignored by git (see .gitignore and the pre-commit hook).
BENCH_DATA = Path(__file__).resolve().parents[2] / "bench-data"
API_VERSION = "1.16.1"
MAX_FILE_BYTES = 250 * 1024 * 1024
MIN_SECONDS = 90
MAX_SECONDS = 900
VERSION_WORDS = re.compile(r"\b(live|remaster(ed)?|acoustic|demo|unplugged|session|version|edit|mix)\b", re.I)


def read_env(path):
    values = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip("'\"")
    return values


def checked_base(url):
    """The server's base URL, only when its host is a private IP literal."""
    parts = urllib.parse.urlsplit(url)
    if parts.scheme not in ("http", "https") or not parts.hostname:
        sys.exit("NAVIDROME_URL must be http(s)://<private IP>:<port>")
    try:
        address = ipaddress.ip_address(parts.hostname)
    except ValueError:
        sys.exit("NAVIDROME_URL must use an IP address, not a name (no DNS lookups)")
    if not address.is_private:
        sys.exit("NAVIDROME_URL must be a private (LAN) address")
    return f"{parts.scheme}://{parts.netloc}"


class Subsonic:
    def __init__(self, base, user, password):
        self.base = base
        self.user = user
        self.password = password
        # No proxy: a proxy setting in the environment would send the LAN request elsewhere.
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def _url(self, method, **params):
        salt = secrets.token_hex(8)
        token = hashlib.md5((self.password + salt).encode()).hexdigest()
        query = {"u": self.user, "t": token, "s": salt, "v": API_VERSION, "c": CLIENT, "f": "json"}
        query.update({k: v for k, v in params.items() if v is not None})
        return f"{self.base}/rest/{method}.view?{urllib.parse.urlencode(query)}"

    def call(self, method, **params):
        with self.opener.open(self._url(method, **params), timeout=60) as response:
            body = json.load(response)["subsonic-response"]
        if body.get("status") != "ok":
            # The error text never carries the token.
            raise RuntimeError(f"{method}: {body.get('error', {}).get('message', 'failed')}")
        return body

    def download(self, song_id, dest):
        with self.opener.open(self._url("download", id=song_id), timeout=300) as response:
            if response.headers.get_content_type().startswith(("text/", "application/json")):
                raise RuntimeError("download answered with an error page")
            written = 0
            with open(dest, "wb") as out:
                while chunk := response.read(1 << 20):
                    written += len(chunk)
                    if written > MAX_FILE_BYTES:
                        raise RuntimeError("file too large")
                    out.write(chunk)


def norm(text):
    text = re.sub(r"\(.*?\)|\[.*?\]", " ", (text or "").lower())
    return re.sub(r"[^a-z0-9]+", " ", text).strip()


def usable(song):
    return (
        song.get("id")
        and song.get("artist")
        and song.get("title")
        and MIN_SECONDS <= int(song.get("duration") or 0) <= MAX_SECONDS
        and not song.get("isVideo")
    )


def entry(song, role, of=None):
    return {
        "id": song["id"],
        "role": role,
        "of": of,
        "artist": song.get("artist", ""),
        "title": song.get("title", ""),
        "album": song.get("album", ""),
        "seconds": int(song.get("duration") or 0),
        "genre": song.get("genre", ""),
        "suffix": song.get("suffix", "bin"),
        "musicbrainz": bool(song.get("musicBrainzId")),
    }


def pick_songs(api, count, rng, skip_ids=(), skip_artists=()):
    """`count` new songs spread over genres and artists, songs with a MusicBrainz id first.
    Songs and artists in `skip_*` (the picks already made) are left out."""
    pool = {}
    for round_ in range(30):
        if round_ >= 4 and len(pool) >= 6 * count:
            break
        for song in api.call("getRandomSongs", size=500).get("randomSongs", {}).get("song", []):
            if usable(song) and song["id"] not in skip_ids and norm(song["artist"]) not in skip_artists:
                pool[song["id"]] = song
    by_genre = {}
    for song in pool.values():
        by_genre.setdefault(norm(song.get("genre")) or "?", []).append(song)
    for songs in by_genre.values():
        rng.shuffle(songs)
        songs.sort(key=lambda s: not s.get("musicBrainzId"))
    picked, artists = [], set(skip_artists)
    while len(picked) < count and any(by_genre.values()):
        for songs in by_genre.values():
            while songs:
                song = songs.pop(0)
                if norm(song["artist"]) not in artists:
                    picked.append(song)
                    artists.add(norm(song["artist"]))
                    break
            if len(picked) >= count:
                break
    return picked


def artist_songs(api, song):
    artist_id = song.get("artistId")
    if not artist_id:
        return []
    songs = []
    for album in api.call("getArtist", id=artist_id).get("artist", {}).get("album", [])[:6]:
        songs += api.call("getAlbum", id=album["id"]).get("album", {}).get("song", [])
    return [s for s in songs if usable(s)]


def artist_search(api, song):
    """Songs by the same artist found by searching its name, when getArtist gave too little."""
    hits = api.call("search3", query=song["artist"], songCount=50, artistCount=0, albumCount=0)
    return [
        s for s in hits.get("searchResult3", {}).get("song", [])
        if usable(s) and norm(s["artist"]) == norm(song["artist"])
    ]


def relatives(api, song, rng):
    """Another song by the same artist, a namesake by another artist, another version."""
    title = norm(song["title"])
    found = {}
    siblings = []
    versions = []

    def sort(others):
        for other in others:
            if other["id"] == song["id"]:
                continue
            other_title = norm(other["title"])
            if other_title == title:
                if VERSION_WORDS.search(f"{other['title']} {other.get('album', '')}"):
                    versions.append(other)
            elif title not in other_title and other_title not in title:
                siblings.append(other)

    sort(artist_songs(api, song))
    if not siblings or not versions:
        sort(artist_search(api, song))
    if siblings:
        found["sibling"] = rng.choice(siblings)
    if versions:
        found["version"] = versions[0]
    hits = api.call("search3", query=song["title"], songCount=40, artistCount=0, albumCount=0)
    for other in hits.get("searchResult3", {}).get("song", []):
        if usable(other) and norm(other["title"]) == title and norm(other["artist"]) != norm(song["artist"]):
            found["namesake"] = other
            break
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--bench", default=str(BENCH_DATA))
    parser.add_argument("--songs", type=int, default=60)
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args()

    bench = Path(args.bench).expanduser()
    env = read_env(bench / ".env")
    api = Subsonic(checked_base(env["NAVIDROME_URL"]), env["NAVIDROME_USER"], env["NAVIDROME_PASSWORD"])
    api.call("ping")
    rng = random.Random(args.seed)

    right = bench / "corpus" / "right"
    right.mkdir(parents=True, exist_ok=True)
    entries = {}
    songs_file = bench / "songs.json"
    if songs_file.exists():
        # Earlier picks stay picks: the new ones are added on top.
        for item in json.loads(songs_file.read_text()):
            if (bench / item["path"]).exists():
                entries[item["id"]] = item

    def keep(song, role, of=None):
        if song["id"] in entries:
            return
        item = entry(song, role, of)
        path = right / f"{song['id']}.{item['suffix']}"
        if not path.exists():
            try:
                api.download(song["id"], path)
            except Exception as error:  # one failed copy only drops that song
                path.unlink(missing_ok=True)
                print(f"  skipped {song['id']}: {error}", file=sys.stderr)
                return
        item["path"] = str(path.relative_to(bench))
        entries[song["id"]] = item

    old = [item for item in entries.values() if item["role"] == "pick"]
    picked = pick_songs(
        api,
        max(0, args.songs - len(old)),
        rng,
        skip_ids={item["id"] for item in entries.values()},
        skip_artists={norm(item["artist"]) for item in old},
    )
    # Old picks come first so a missing sibling or version is looked for again.
    todo = [api.call("getSong", id=item["id"]).get("song") or item for item in old]
    todo += picked
    for n, song in enumerate(todo, 1):
        print(f"[{n}/{len(todo)}] {song['id']}", file=sys.stderr)
        keep(song, "pick")
        if song["id"] not in entries:
            continue
        have = {item["role"] for item in entries.values() if item["of"] == song["id"]}
        for role, other in relatives(api, song, rng).items():
            if role not in have:
                keep(other, role, song["id"])

    (bench / "songs.json").write_text(json.dumps(list(entries.values()), indent=1, ensure_ascii=False))
    roles = {}
    for item in entries.values():
        roles[item["role"]] = roles.get(item["role"], 0) + 1
    print(f"saved {len(entries)} files to {right}: {roles}", file=sys.stderr)


if __name__ == "__main__":
    main()
