"""The 403 ladder's last rung: update yt-dlp, and keep that update across a
container recreate. YTDLP_VERSION=latest is resolved at image build time, so
without this a stack that is never rebuilt runs one frozen version forever."""
import os
import sys
import time

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

app = pytest.importorskip("app")


def _write(path, text, mtime=None):
    with open(path, "w") as fh:
        fh.write(text)
    if mtime is not None:
        os.utime(path, (mtime, mtime))


class TestSeeding:
    def test_seeds_when_the_volume_is_empty(self, tmp_path, monkeypatch):
        dist = tmp_path / "dist" / "yt-dlp"
        dist.parent.mkdir()
        _write(dist, "image-copy")
        live = tmp_path / "vol" / "yt-dlp"
        monkeypatch.setattr(app, "YTDLP_DIST", str(dist))
        monkeypatch.setattr(app, "YTDLP", str(live))

        app._seed_writable_ytdlp()

        assert live.read_text() == "image-copy"
        assert os.access(live, os.X_OK) or os.name == "nt"

    def test_a_rebuild_wins_over_an_older_self_update(self, tmp_path, monkeypatch):
        """Image copy newer than the volume copy means someone rebuilt; take it."""
        dist = tmp_path / "dist" / "yt-dlp"
        dist.parent.mkdir()
        live = tmp_path / "vol" / "yt-dlp"
        live.parent.mkdir()
        _write(live, "self-updated", mtime=time.time() - 600)
        _write(dist, "freshly-built", mtime=time.time())
        monkeypatch.setattr(app, "YTDLP_DIST", str(dist))
        monkeypatch.setattr(app, "YTDLP", str(live))

        app._seed_writable_ytdlp()

        assert live.read_text() == "freshly-built"

    def test_a_self_update_is_not_clobbered_by_an_older_image(self, tmp_path, monkeypatch):
        """The common case on every restart: the volume is ahead, leave it be."""
        dist = tmp_path / "dist" / "yt-dlp"
        dist.parent.mkdir()
        live = tmp_path / "vol" / "yt-dlp"
        live.parent.mkdir()
        _write(dist, "baked-old", mtime=time.time() - 600)
        _write(live, "self-updated-new", mtime=time.time())
        monkeypatch.setattr(app, "YTDLP_DIST", str(dist))
        monkeypatch.setattr(app, "YTDLP", str(live))

        app._seed_writable_ytdlp()

        assert live.read_text() == "self-updated-new"

    def test_is_a_no_op_without_a_separate_writable_path(self, tmp_path, monkeypatch):
        same = tmp_path / "yt-dlp"
        _write(same, "only-copy")
        monkeypatch.setattr(app, "YTDLP_DIST", str(same))
        monkeypatch.setattr(app, "YTDLP", str(same))

        app._seed_writable_ytdlp()  # must not raise or truncate

        assert same.read_text() == "only-copy"


class TestSelfUpdate:
    @pytest.fixture(autouse=True)
    def _reset_rate_limit(self, monkeypatch):
        monkeypatch.setattr(app, "_last_self_update", None)
        monkeypatch.setattr(app, "_SELF_UPDATE_ON_403", True)
        monkeypatch.setattr(app, "_SELF_UPDATE_MIN_INTERVAL_SEC", 21600)
        monkeypatch.setattr(app, "_purge_cache_contents", lambda: None)

    def test_reports_success_only_when_the_version_moves(self, monkeypatch):
        versions = iter(["2026.07.04", "2026.08.19"])
        monkeypatch.setattr(app, "_ytdlp_version", lambda: next(versions))
        monkeypatch.setattr(app, "_verified_update", lambda: True)

        assert app._maybe_self_update("test") is True

    def test_reports_failure_when_already_current(self, monkeypatch):
        monkeypatch.setattr(app, "_ytdlp_version", lambda: "2026.08.19")
        monkeypatch.setattr(app, "_verified_update", lambda: True)

        # Same version both sides means the refusal is something else, and the
        # caller must not retry on the strength of an update that did nothing.
        assert app._maybe_self_update("test") is False

    def test_rate_limit_blocks_a_second_attempt(self, monkeypatch):
        versions = iter(["a", "b", "c", "d"])
        monkeypatch.setattr(app, "_ytdlp_version", lambda: next(versions))
        monkeypatch.setattr(app, "_verified_update", lambda: True)

        assert app._maybe_self_update("first") is True
        # An outage means every play 403s; without this the shim would fetch a
        # binary per request.
        assert app._maybe_self_update("second") is False

    def test_first_attempt_is_allowed_on_a_freshly_booted_host(self, monkeypatch):
        """time.monotonic() is time since boot on Linux. A 0.0 sentinel would put
        a host up for under the interval inside its own rate-limit window and
        block the first update, which is the reboot-then-outage case."""
        monkeypatch.setattr(app, "_last_self_update", None)
        monkeypatch.setattr(app, "_SELF_UPDATE_MIN_INTERVAL_SEC", 10 ** 9)
        versions = iter(["old", "new"])
        monkeypatch.setattr(app, "_ytdlp_version", lambda: next(versions))
        monkeypatch.setattr(app, "_verified_update", lambda: True)

        assert app._maybe_self_update("first ever") is True

    def test_can_be_switched_off(self, monkeypatch):
        monkeypatch.setattr(app, "_SELF_UPDATE_ON_403", False)
        called = []
        monkeypatch.setattr(app, "_ytdlp_version", lambda: called.append(1) or "x")

        assert app._maybe_self_update("test") is False
        assert called == []
