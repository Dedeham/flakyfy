"""End to end: the yt-dlp fallback (FLA-21), against the stubbed slskd and yt-dlp shim.

Needs the seeded stack (./run.sh). What a run cannot do is wait: three counted tries six hours
apart. So each test writes the counting store the way it would stand after two counted tries
(two tries, the last seven hours ago), restarts Octo so it reads it, and then hearts the song, which
is the third. The unit tests (octo.Tests/YtDlpFallback) cover the counting with a fake clock.

The tests run in numbered order: the outage test goes last, because a heart held for Soulseek
stays held until the stack is torn down.
"""
import base64
import datetime
import json
import tempfile
import time
import unittest
import urllib.error
import urllib.parse
import urllib.request

import harness as h

SONG_ID_PREFIX = "ext-soulseek-song-"


def b64(text):
    return base64.urlsafe_b64encode(text.encode()).decode().rstrip("=")


def song_id(artist, title, seconds=200):
    """An outside song id as Octo's registry would hand one out: yt|<video>|<artist>|<title>|<length>."""
    return f"yt||{b64(artist)}|{b64(title)}|{seconds}"


def iso(delta_hours):
    t = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(hours=delta_hours)
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def store_after_two_counted_tries(ext_id, requested_by="alice"):
    return {"givenUp": 0, "entries": [{
        "provider": "soulseek", "externalId": ext_id, "requestedBy": requested_by, "origin": "heart",
        "firstMissUtc": iso(-14), "countedAttempts": 2, "lastCountedUtc": iso(-7),
        # Just run, so Octo's own worker leaves the song alone for its first 30 seconds and the heart
        # below is what runs the chain.
        "lastRunUtc": iso(0),
    }]}


def stub_log():
    return h.http_json("GET", h.STUB + "/__log")


def stub_calls(kind):
    return [e for e in stub_log() if e["kind"] == kind]


def set_soulseek(state):
    h.http_json("POST", h.STUB + "/__control", {"soulseek": state})


def write_store(service, data):
    """Put the counting store into the container, owner-only, and restart Octo so it reads it."""
    name = h.container(service)
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
        json.dump(data, fh)
    h.docker("cp", fh.name, f"{name}:/app/config/ytdlp-fallback.json")
    h.docker("exec", name, "chmod", "600", "/app/config/ytdlp-fallback.json")
    h.docker("restart", name)
    base = h.OCTO if service == "octo" else h.OCTO_YT
    h.wait_for(lambda: h.http_ok(base + "/rest/ping?u=x&p=x&v=1.16.1&c=probe&f=json"), service, 120)


def read_store(service):
    out = h.docker("exec", h.container(service), "cat", "/app/config/ytdlp-fallback.json", check=False)
    return json.loads(out) if out.strip() else None


def library_files(artist):
    """MP3 files Octo placed in its library for this artist, outside the staging folder."""
    out = h.docker("exec", h.container("octo"), "find", "/music", "-name", "*.mp3",
                   "-not", "-path", "*/.octo-incoming/*", check=False)
    return [line for line in out.splitlines() if artist in line]


def tags_of(path):
    out = h.docker("exec", h.container("octo"), "ffprobe", "-v", "quiet", "-print_format", "json",
                   "-show_entries", "format_tags", path)
    return {k.upper(): v for k, v in (json.loads(out).get("format", {}).get("tags") or {}).items()}


def staged_files():
    out = h.docker("exec", h.container("octo"), "sh", "-c",
                   "ls /music/.octo-incoming 2>/dev/null | head -20", check=False)
    return out.split()


def heart(base, user, ext_id):
    return h.Subsonic(base, user, h.USERS[user]).call("star", {"id": SONG_ID_PREFIX + ext_id})


def wait_until(fn, what, seconds=90):
    h.wait_for(fn, what, seconds)


class YtDlpFallback(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        set_soulseek("up")

    def test_1_a_song_missing_everywhere_arrives_through_the_shim_tagged_as_a_stopgap(self):
        artist, title = "Fallback Artist One", "Nowhere To Be Found"
        ext_id = song_id(artist, title)
        write_store("octo", store_after_two_counted_tries(ext_id))
        before = len(stub_calls("shim-download"))

        heart(h.OCTO, "alice", ext_id)

        wait_until(lambda: library_files(artist), "the fallback copy to land in the library")
        self.assertEqual(len(stub_calls("shim-download")), before + 1, "one download through the shim")
        path = library_files(artist)[0]
        tags = tags_of(path)
        self.assertEqual(tags.get("OCTO_SOURCE"), "youtube-fallback")
        self.assertEqual(tags.get("OCTO_SOURCE_ID"), "stubVideo01")
        self.assertTrue(path.endswith(".mp3"))
        # The tags say what the copy is and nothing about who asked for it.
        self.assertNotIn("alice", json.dumps(tags).lower())
        # The song is done, so it is no longer being counted.
        wait_until(lambda: not (read_store("octo") or {}).get("entries"), "the counting entry to go")

    def test_2_one_try_short_nothing_is_fetched(self):
        artist, title = "Fallback Artist Two", "Only Tried Once"
        ext_id = song_id(artist, title)
        store = store_after_two_counted_tries(ext_id)
        store["entries"][0].update(countedAttempts=1)
        write_store("octo", store)
        before = len(stub_calls("shim-download"))

        heart(h.OCTO, "alice", ext_id)

        # The chain ran (Soulseek was searched), counted a second try, and waits; it fetched nothing.
        wait_until(lambda: (read_store("octo") or {"entries": []})["entries"][0]["countedAttempts"] == 2,
                   "the second counted try")
        self.assertEqual(len(stub_calls("shim-download")), before)
        self.assertEqual(library_files(artist), [])

    def test_3_with_youtube_in_the_persons_own_list_the_fallback_does_nothing(self):
        artist, title = "Fallback Artist Three", "Chosen Youtube Myself"
        ext_id = song_id(artist, title)
        write_store("octo-yt", store_after_two_counted_tries(ext_id))

        heart(h.OCTO_YT, "alice", ext_id)

        # The person's own YouTube step fetches it, as it always did: a file with no stopgap mark.
        wait_until(lambda: library_files(artist), "the copy from the YouTube step")
        self.assertNotIn("OCTO_SOURCE", tags_of(library_files(artist)[0]))
        # And the fallback did not count a try for it.
        entries = (read_store("octo-yt") or {}).get("entries", [])
        self.assertTrue(all(e["countedAttempts"] == 2 for e in entries), "the fallback counted a try")

    def test_4_the_counting_is_private_and_the_admin_sees_counts_only(self):
        # Nobody without an admin sign-in reads the fallback's status, a listener included.
        for user in ("alice", "bob"):
            req = urllib.request.Request(h.OCTO + "/api/admin/ytdlp-fallback",
                                         headers={"Authorization": "Basic " + base64.b64encode(
                                             f"{user}:{h.USERS[user]}".encode()).decode()})
            with self.assertRaises(urllib.error.HTTPError) as caught:
                urllib.request.urlopen(req, timeout=10)
            self.assertEqual(caught.exception.code, 401, f"{user} must not read the status")
        # A listener's own sign-in does not open the admin API either.
        body = json.dumps({"username": "bob", "password": h.USERS["bob"]}).encode()
        req = urllib.request.Request(h.OCTO + "/api/admin/browse/auth", data=body, method="POST",
                                     headers={"Content-Type": "application/json", "X-Octo-Admin": "1"})
        with self.assertRaises(urllib.error.HTTPError) as caught:
            urllib.request.urlopen(req, timeout=10)
        self.assertEqual(caught.exception.code, 401)
        # The admin sees counts: no song, no person.
        jar = urllib.request.build_opener(urllib.request.HTTPCookieProcessor())
        admin = json.dumps({"username": h.ADMIN[0], "password": h.ADMIN[1]}).encode()
        jar.open(urllib.request.Request(h.OCTO + "/api/admin/browse/auth", data=admin, method="POST",
                                        headers={"Content-Type": "application/json", "X-Octo-Admin": "1"}), timeout=10)
        status = json.loads(jar.open(h.OCTO + "/api/admin/ytdlp-fallback", timeout=10).read())
        self.assertTrue(status["enabled"])
        self.assertEqual(set(status), {"enabled", "waiting", "givenUp", "lastGivenUpUtc", "failedAttempts",
                                       "attemptSpacingHours", "maxWaitHours", "line"})
        # The store file itself is owner-only.
        mode = h.docker("exec", h.container("octo"), "stat", "-c", "%a", "/app/config/ytdlp-fallback.json").strip()
        self.assertEqual(mode, "600")

    def test_9_during_a_soulseek_outage_nothing_is_taken_from_youtube(self):
        artist, title = "Fallback Artist Nine", "Waiting For Soulseek"
        ext_id = song_id(artist, title)
        # Even a song with all its counted tries made is not fetched while Soulseek is out.
        write_store("octo", store_after_two_counted_tries(ext_id))
        set_soulseek("down")
        searches = len(stub_calls("shim-search"))
        downloads = len(stub_calls("shim-download"))

        heart(h.OCTO, "alice", ext_id)
        wait_until(lambda: ext_id in h.docker("exec", h.container("octo"), "cat", "/app/config/soulseek-holds.json",
                                              check=False), "the heart to be held for Soulseek", 60)
        time.sleep(10)

        self.assertEqual(len(stub_calls("shim-search")), searches, "YouTube was searched during the outage")
        self.assertEqual(len(stub_calls("shim-download")), downloads, "YouTube was used during the outage")
        self.assertEqual(library_files(artist), [])
        entries = (read_store("octo") or {}).get("entries", [])
        self.assertTrue(all(e["countedAttempts"] <= 2 for e in entries), "an outage must not count as a try")
        # And the heart really is waiting for Soulseek, so this is not a heart that never ran.
        holds = h.docker("exec", h.container("octo"), "cat", "/app/config/soulseek-holds.json")
        self.assertIn(ext_id, holds)


if __name__ == "__main__":
    unittest.main()
