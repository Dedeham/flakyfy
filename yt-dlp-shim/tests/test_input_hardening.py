"""What the shim accepts from a request: video ids, queries, the download destination, and the
arguments that reach yt-dlp. Octo sends clean values; these tests cover what happens when
something else sends them."""
import logging
import os
import subprocess
import sys

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

app_module = pytest.importorskip("app")

GOOD_ID = "dQw4w9WgXcQ"


@pytest.fixture
def client(tmp_path, monkeypatch):
    root = os.path.realpath(tmp_path / "music")
    os.makedirs(os.path.join(root, ".octo-incoming"))
    monkeypatch.setattr(app_module, "_DOWNLOAD_ROOT", root)
    monkeypatch.setattr(app_module, "_INCOMING_ROOT", os.path.join(root, ".octo-incoming"))
    calls = []

    def fake_run(args, **kwargs):
        calls.append(list(args))
        return None

    monkeypatch.setattr(app_module, "_run", fake_run)
    app_module.app.testing = True
    c = app_module.app.test_client()
    c.calls = calls
    c.root = root
    return c


@pytest.mark.parametrize("bad", [
    "", "short", "twelvechars!", "dQw4w9WgXc", "dQw4w9WgXcQQ", "dQw4w9WgXc/", "dQw4w9WgX\nQ",
    "../../etc/pa", "dQw4w9 gXcQ", "dQw4w9WgXcé", "dQw4w9WgXc%",
])
def test_stream_refuses_a_malformed_video_id(client, bad):
    assert client.get("/stream", query_string={"id": bad}).status_code == 400
    assert client.calls == []


def test_a_video_id_that_starts_with_a_dash_is_a_valid_id_and_sits_after_double_dash(client):
    # "-" is part of the alphabet, so an id may begin with it; the URL carries it, and the
    # "--" keeps it from ever being read as an option.
    vid = "-abcdefghij"
    client.get("/stream", query_string={"id": vid})
    argv = client.calls[0]
    assert argv[-2] == "--"
    assert argv[-1] == f"https://www.youtube.com/watch?v={vid}"


@pytest.mark.parametrize("route", ["/search", "/meta"])
@pytest.mark.parametrize("bad", ["", "   ", "a" * 201, "line\nbreak", "tab\there", "nul\x00byte", "bell\x07"])
def test_search_and_meta_refuse_empty_long_or_control_character_queries(client, route, bad):
    assert client.get(route, query_string={"q": bad}).status_code == 400
    assert client.calls == []


@pytest.mark.parametrize("q", [
    "--exec rm -rf /", "-o /etc/passwd", "https://example.com/x", "ytsearch5:other", "\"quoted\"", "a'b",
])
def test_odd_queries_only_ever_follow_the_fixed_search_prefix(client, q):
    client.get("/search", query_string={"q": q})
    client.get("/meta", query_string={"q": q, "duration": "200"})
    assert client.calls, "the query should have reached yt-dlp, behind the prefix"
    for argv in client.calls:
        assert argv[0] in (f"ytsearch1:{q}", f"ytsearch5:{q}")
        # Nothing else in the argv came from the request.
        assert all(q not in a for a in argv[1:])


@pytest.mark.parametrize("hint", ["abc", "-5", "1e3", "0", "99999999999", "٣٠٠", "3.5"])
def test_a_bad_duration_hint_is_ignored_not_trusted(client, hint):
    assert app_module._parse_duration(hint) is None


def test_a_good_duration_hint_is_kept():
    assert app_module._parse_duration("215") == 215


def _download(client, dest, **extra):
    return client.get("/download", query_string={"id": GOOD_ID, "dest": dest, **extra})


def test_download_refuses_a_malformed_id(client):
    r = client.get("/download", query_string={"id": "bad id", "dest": os.path.join(client.root, ".octo-incoming", "x")})
    assert r.status_code == 400


@pytest.mark.parametrize("rel", [
    "../x",                      # above the root
    "x",                         # root itself
    "Artist/x",                  # elsewhere under the root
    ".octo-incoming-evil/x",     # sibling that shares the prefix
    ".octo-incoming",            # the folder itself
])
def test_download_dest_must_be_inside_the_staging_folder(client, rel):
    assert _download(client, os.path.join(client.root, rel)).status_code == 400
    assert client.calls == []


def test_download_refuses_a_symlinked_staging_parent(client, tmp_path):
    outside = tmp_path / "elsewhere"
    outside.mkdir()
    link = os.path.join(client.root, ".octo-incoming", "link")
    os.symlink(outside, link)
    assert _download(client, os.path.join(link, "x")).status_code == 400
    assert client.calls == []


def test_download_refuses_a_symlinked_staging_folder(client, tmp_path):
    outside = tmp_path / "elsewhere"
    outside.mkdir()
    staging = os.path.join(client.root, ".octo-incoming")
    os.rmdir(staging)
    os.symlink(outside, staging)
    assert _download(client, os.path.join(staging, "x")).status_code == 400
    assert client.calls == []


def test_download_refuses_a_nul_in_dest(client):
    assert _download(client, os.path.join(client.root, ".octo-incoming", "x\0y")).status_code == 400


def test_download_escapes_percent_and_puts_double_dash_before_the_url(client):
    dest = os.path.join(client.root, ".octo-incoming", "%(title)s-%(id)s")
    _download(client, dest)
    argv = client.calls[0]
    template = argv[argv.index("-o") + 1]
    assert "%(title)s" not in template.replace("%%(title)s", "")
    assert template.endswith(".%(ext)s")
    assert template.count("%%") == 2
    assert argv[-2] == "--"
    assert argv[-1] == f"https://www.youtube.com/watch?v={GOOD_ID}"
    assert "--match-filters" in argv and "--max-filesize" in argv


def test_download_refuses_control_characters_in_artist_and_title(client):
    dest = os.path.join(client.root, ".octo-incoming", "x")
    assert _download(client, dest, artist="a\nb", title="t").status_code == 400
    assert _download(client, dest, artist="a", title="t" * 201).status_code == 400
    assert client.calls == []


def test_an_oversized_result_is_deleted_and_fails(client, monkeypatch):
    dest = os.path.join(client.root, ".octo-incoming", "big")

    def fake_run(args, **kwargs):
        with open(dest + ".mp3", "wb") as fh:
            fh.write(b"x" * 10)
        return subprocess.CompletedProcess(args, 0, "", "")

    monkeypatch.setattr(app_module, "_run", fake_run)
    monkeypatch.setattr(app_module, "_DOWNLOAD_MAX_BYTES", 5)
    r = _download(client, dest)
    assert r.status_code == 502
    assert not os.path.exists(dest + ".mp3")


def test_the_run_logs_never_carry_the_search_text(monkeypatch, caplog):
    class Done:
        returncode = 1
        stdout = ""
        stderr = "ERROR: could not find secret-song-text"

    monkeypatch.setattr(app_module.subprocess, "run", lambda *a, **k: Done())
    with caplog.at_level(logging.DEBUG):
        app_module._run(["ytsearch1:secret-song-text"], label="search+resolve")
    assert "secret-song-text" not in caplog.text


@pytest.mark.parametrize("bad", ["", "short", "dQw4w9WgXcQQ", "dQw4w9WgXc/", "../../x", "dQw4w9WgX\nQ"])
def test_ytm_radio_refuses_a_malformed_video_id(client, monkeypatch, bad):
    called = []
    monkeypatch.setattr(app_module.ytm, "radio", lambda *a, **k: called.append(a))
    assert client.get("/ytm/radio", query_string={"videoId": bad}).status_code == 400
    assert called == []


def test_ytm_radio_takes_a_good_id_and_a_failure_logs_no_id(client, monkeypatch, caplog):
    def boom(*a, **k):
        raise RuntimeError("secret detail dQw4w9WgXcQ")

    monkeypatch.setattr(app_module.ytm, "radio", boom)
    app_module._YTM_CACHE["radio"].clear()
    with caplog.at_level(logging.DEBUG):
        assert client.get("/ytm/radio", query_string={"videoId": GOOD_ID}).status_code == 502
    assert GOOD_ID not in caplog.text
    assert "secret detail" not in caplog.text
