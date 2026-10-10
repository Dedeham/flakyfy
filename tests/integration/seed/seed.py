#!/usr/bin/env python3
"""Seed a fresh Navidrome: admin, three users, a scanned library, playlists, favorites, plays.

Run after the stack is up (run.sh does it). Safe to run once on an empty stack.
"""
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import harness as h  # noqa: E402


def native_login():
    body = {"username": h.ADMIN[0], "password": h.ADMIN[1]}
    try:
        return h.http_json("POST", f"{h.NAVIDROME}/auth/createAdmin", body)["token"]
    except Exception:  # already created: sign in instead
        return h.http_json("POST", f"{h.NAVIDROME}/auth/login", body)["token"]


def main():
    h.wait_for(lambda: h.http_ok(f"{h.NAVIDROME}/ping"), "navidrome")
    token = native_login()
    for name, pw in h.USERS.items():
        try:
            h.http_json("POST", f"{h.NAVIDROME}/api/user", headers={"x-nd-authorization": f"Bearer {token}"},
                        body={"userName": name, "name": name.title(), "password": pw, "isAdmin": False})
        except Exception as e:
            print(f"user {name}: {e}")

    admin = h.Subsonic(h.NAVIDROME, *h.ADMIN)
    admin.call("startScan", {"fullScan": "true"})
    h.wait_for(lambda: not admin.call("getScanStatus")["scanStatus"]["scanning"]
               and len(admin.all_songs()) >= 12, "library scan")
    songs = sorted(admin.all_songs(), key=lambda s: (s["artist"], s["album"], s.get("track", 0)))
    ids = [s["id"] for s in songs]
    print(f"library: {len(ids)} songs")

    clients = {n: h.Subsonic(h.NAVIDROME, n, pw) for n, pw in h.USERS.items()}
    # Overlapping tastes on purpose: isolation tests must not pass just because lists differ in shape.
    plan = {
        "alice": {"playlists": {"Alice Morning": ids[0:4], "Alice Focus": ids[4:7]}, "stars": ids[0:3], "plays": ids[0:5]},
        "bob": {"playlists": {"Bob Workout": ids[2:8]}, "stars": ids[3:5], "plays": ids[2:9]},
        "carol": {"playlists": {"Carol Quiet": ids[7:10], "Carol Shared Songs": ids[0:2]}, "stars": ids[8:11], "plays": ids[6:12]},
    }
    for name, c in clients.items():
        for pl, song_ids in plan[name]["playlists"].items():
            c.create_playlist(pl, song_ids)
        for sid in plan[name]["stars"]:
            c.call("star", {"id": sid})
        for sid in plan[name]["plays"]:
            c.call("scrobble", {"id": sid, "time": int(time.time() * 1000), "submission": "true"})
        print(f"{name}: {len(plan[name]['playlists'])} playlists, {len(plan[name]['stars'])} favorites, "
              f"{len(plan[name]['plays'])} plays")


if __name__ == "__main__":
    main()
