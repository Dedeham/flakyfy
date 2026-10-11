"""End to end (FLA-19): what an import does for the person who made it, and for nobody else.

  * order: a kept playlist follows the source file's order, also when a song arrives later;
  * favorites: the liked songs become favorites for the importer only, set on the importer's next
    request through Octo (Octo keeps song ids, never a sign-in);
  * privacy: the playlist is private and belongs to the importer.

Lists belong to the person signed in on the dashboard, and the dashboard is for Navidrome admins,
so the importer here is a throwaway admin account made for this test. bob and carol are the
seeded listeners who must see no trace of it. Needs the seeded stack (./run.sh).
"""
import io
import json
import os
import subprocess
import sys
import time
import unittest
import urllib.error
import urllib.request
import uuid

import harness as h

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "seed"))
import generate_library  # noqa: E402

IMPORTER, IMPORTER_PW = "ivy", "ivy-test-pw"
LIBRARY_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".data", "music")
PROJECT = os.environ.get("HARNESS_PROJECT", "fla12-it")

# Source order on purpose unlike the library's: three songs the library has, one it gets later,
# then two more it has.
ROAD_TRIP = [("Concert A", "Tone Generator", "Sine Waves"), ("Hush", "Silent Partners", "Quiet Room"),
             ("Root", "Tone Generator", "Fifths"), ("Late Arrival", "Tone Generator", "Latecomers"),
             ("Fifth", "Tone Generator", "Fifths"), ("Still", "Silent Partners", "Quiet Room")]
LIKED = [("Low A", "Tone Generator", "Sine Waves"), ("Root", "Tone Generator", "Fifths"),
         ("Two Kilohertz", "Test Pattern", "Calibration")]


def csv_of(rows):
    lines = ["Track Name,Artist Name(s),Album Name"]
    lines += [",".join(f'"{x}"' for x in row) for row in rows]
    return "\n".join(lines) + "\n"


def sign_in_dashboard(user, password):
    req = urllib.request.Request(h.OCTO + "/api/admin/browse/auth", method="POST",
                                 data=json.dumps({"username": user, "password": password}).encode(),
                                 headers={"Content-Type": "application/json", "X-Octo-Admin": "1"})
    with urllib.request.urlopen(req, timeout=30) as r:
        cookies = r.headers.get_all("Set-Cookie") or []
    token = next(c.split(";")[0].split("=", 1)[1] for c in cookies if c.startswith("octo_browse="))
    return {"X-Octo-Browse-Token": token, "X-Octo-Admin": "1"}


def dashboard_post(headers, path, body=None):
    req = urllib.request.Request(h.OCTO + path, method="POST", data=json.dumps(body or {}).encode(),
                                 headers={"Content-Type": "application/json", **headers})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.loads(r.read())


def dashboard_upload(headers, path, filename, text):
    boundary = uuid.uuid4().hex
    body = (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{filename}"\r\n'
            f"Content-Type: text/csv\r\n\r\n{text}\r\n--{boundary}--\r\n").encode()
    req = urllib.request.Request(h.OCTO + path, method="POST", data=body,
                                 headers={"Content-Type": f"multipart/form-data; boundary={boundary}", **headers})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.loads(r.read())


def native_admin_token():
    return h.http_json("POST", f"{h.NAVIDROME}/auth/login", {"username": h.ADMIN[0], "password": h.ADMIN[1]})["token"]


def make_importer():
    try:
        h.http_json("POST", f"{h.NAVIDROME}/api/user", headers={"x-nd-authorization": f"Bearer {native_admin_token()}"},
                    body={"userName": IMPORTER, "name": IMPORTER.title(), "password": IMPORTER_PW, "isAdmin": True})
    except urllib.error.HTTPError:
        pass  # left over from an earlier run on the same stack


def client(base, name, password=None):
    return h.Subsonic(base, name, password or h.USERS[name])


def titles_of(playlist):
    return [song["title"] for song in playlist.get("entry", [])]


def song_id(user, title):
    return next(s["id"] for s in user.all_songs() if s["title"] == title)


class ImportFidelity(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        make_importer()
        cls.admin = h.Subsonic(h.NAVIDROME, *h.ADMIN)
        cls.ivy_nd = client(h.NAVIDROME, IMPORTER, IMPORTER_PW)
        cls.ivy = client(h.OCTO, IMPORTER, IMPORTER_PW)
        cls.bob, cls.carol = client(h.OCTO, "bob"), client(h.OCTO, "carol")
        cls.bob_nd, cls.carol_nd = client(h.NAVIDROME, "bob"), client(h.NAVIDROME, "carol")
        cls.stars_before = {n: set(c.starred_ids()) for n, c in (("bob", cls.bob_nd), ("carol", cls.carol_nd))}
        cls.headers = sign_in_dashboard(IMPORTER, IMPORTER_PW)

        added = dashboard_upload(cls.headers, "/api/admin/imports/file", "Liked Songs.csv", csv_of(LIKED))
        cls.liked_id = added.get("listId") or added.get("ListId")
        trip = dashboard_upload(cls.headers, "/api/admin/imports/file", "Road Trip.csv", csv_of(ROAD_TRIP))
        cls.trip_id = trip.get("listId") or trip.get("ListId")
        dashboard_post(cls.headers, f"/api/admin/imports/lists/{cls.trip_id}/playlist", {"On": True})
        dashboard_post(cls.headers, f"/api/admin/imports/lists/{cls.liked_id}/playlist", {"On": True})
        # Nothing has been favorited yet: the import has no sign-in of the importer's to do it with.
        cls.stars_after_import = set(cls.ivy_nd.starred_ids())
        # The importer's app talks to Octo; that is the moment the favorites are set.
        cls.ivy.playlists()
        h.wait_for(lambda: len(cls.ivy_nd.starred_ids()) >= len(LIKED), "the importer's favorites", 60)

    def playlist_named(self, user, name):
        return next((p for p in user.playlists() if p["name"] == name), None)

    # ---- order (passes on the old code too: a lock) ------------------------------------------

    def test_the_playlist_has_the_songs_in_the_source_order(self):
        found = self.playlist_named(self.ivy, "Road Trip")
        self.assertIsNotNone(found, "the kept playlist is not there")
        have = [t for t, _, _ in ROAD_TRIP if t != "Late Arrival"]
        self.assertEqual(have, titles_of(self.ivy.playlist(found["id"])))

    # ---- favorites (fails on the old code: nothing was ever starred) --------------------------

    def test_the_import_alone_favorites_nothing(self):
        self.assertEqual(set(), self.stars_after_import)

    def test_liked_songs_become_the_importers_favorites(self):
        wanted = {song_id(self.ivy_nd, t) for t, _, _ in LIKED}
        self.assertEqual(wanted, set(self.ivy_nd.starred_ids()))

    def test_nobody_else_gets_those_favorites(self):
        self.assertEqual(self.stars_before["bob"], set(self.bob_nd.starred_ids()))
        self.assertEqual(self.stars_before["carol"], set(self.carol_nd.starred_ids()))

    # ---- privacy (passes on the old code too: a lock) ----------------------------------------

    def test_the_playlist_is_private_to_the_importer(self):
        mine = self.playlist_named(self.ivy, "Road Trip")
        self.assertIsNotNone(mine)
        self.assertEqual(IMPORTER, mine["owner"])
        self.assertFalse(mine.get("public", False))
        for who, base in (("bob", self.bob), ("carol", self.carol_nd)):
            self.assertIsNone(self.playlist_named(base, "Road Trip"), f"{who} can see the importer's playlist")
        with self.assertRaises(Exception):
            self.bob.playlist(mine["id"])
        with self.assertRaises(Exception):
            self.bob_nd.playlist(mine["id"])

    # ---- order after a song arrives later (named to run last) --------------------------------

    def test_zz_a_song_that_arrives_later_takes_its_place(self):
        folder = os.path.join(LIBRARY_DIR, "Tone Generator", "Latecomers")
        os.makedirs(folder, exist_ok=True)
        from pathlib import Path
        tags = [(b"INAM", "Late Arrival"), (b"IART", "Tone Generator"), (b"IPRD", "Latecomers"),
                (b"ITRK", "1"), (b"ICRD", "2024"), (b"IGNR", "Test")]
        generate_library.write_wav(Path(folder) / "01 Late Arrival.wav", 330, tags)
        self.admin.call("startScan", {"fullScan": "true"})
        h.wait_for(lambda: not self.admin.call("getScanStatus")["scanStatus"]["scanning"]
                   and any(s["title"] == "Late Arrival" for s in self.admin.all_songs()), "the late song in Navidrome")
        # Octo reads the library once and keeps it a few minutes; a restart is the quick way to a fresh read.
        subprocess.run(["docker", "restart", f"{PROJECT}-octo"], check=True, capture_output=True)
        h.wait_for(lambda: h.http_ok(h.OCTO + "/rest/ping?u=x&p=x&v=1.16.1&c=probe&f=json"), "octo")
        headers = sign_in_dashboard(IMPORTER, IMPORTER_PW)
        dashboard_post(headers, f"/api/admin/imports/lists/{self.trip_id}/refresh")
        found = self.playlist_named(self.ivy, "Road Trip")
        self.assertEqual([t for t, _, _ in ROAD_TRIP], titles_of(self.ivy.playlist(found["id"])))


if __name__ == "__main__":
    unittest.main()
