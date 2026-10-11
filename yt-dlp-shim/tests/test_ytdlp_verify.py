"""yt-dlp is installed only when its SHA2-256 matches the list the project publishes with the
release, on the self-update path and for the copy that runs at startup."""
import hashlib
import os
import sys

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import ytdlp_verify as v

app = pytest.importorskip("app")

BODY = b"#!/bin/sh\necho 2026.10.01\n"
DIGEST = hashlib.sha256(BODY).hexdigest()


class FakeResp:
    def __init__(self, data, status=200):
        self.status_code = status
        self._data = data

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def iter_content(self, n):
        for i in range(0, len(self._data), n):
            yield self._data[i:i + n]


class FakeSession:
    def __init__(self, files):
        self.files = files
        self.urls = []

    def get(self, url, **kw):
        self.urls.append(url)
        name = url.rsplit("/", 1)[-1]
        if name not in self.files:
            return FakeResp(b"", 404)
        return FakeResp(self.files[name])


def sums(digest=DIGEST, name="yt-dlp"):
    return f"{digest}  {name}\n{'0' * 64}  yt-dlp_linux\n".encode()


class TestParse:
    def test_finds_the_listed_asset_only(self):
        assert v.parse_sums(sums().decode()) == DIGEST
        assert v.parse_sums(sums(name="other").decode()) is None

    def test_ignores_malformed_lines(self):
        assert v.parse_sums("nothex  yt-dlp\n" + "a" * 63 + "  yt-dlp\n") is None


class TestDownload:
    def test_a_matching_release_is_kept_executable(self, tmp_path):
        session = FakeSession({"SHA2-256SUMS": sums(), "yt-dlp": BODY})
        path, digest = v.download_verified(str(tmp_path), session, base="https://example.test/r")
        assert digest == DIGEST
        assert open(path, "rb").read() == BODY
        assert os.access(path, os.X_OK)
        assert session.urls[0].endswith("latest/download/SHA2-256SUMS")

    def test_a_mismatch_is_refused_and_leaves_nothing_behind(self, tmp_path):
        session = FakeSession({"SHA2-256SUMS": sums("1" * 64), "yt-dlp": BODY})
        with pytest.raises(v.VerifyError):
            v.download_verified(str(tmp_path), session, base="https://example.test/r")
        assert list(tmp_path.iterdir()) == []

    def test_no_listed_checksum_is_refused(self, tmp_path):
        session = FakeSession({"SHA2-256SUMS": sums(name="other"), "yt-dlp": BODY})
        with pytest.raises(v.VerifyError):
            v.download_verified(str(tmp_path), session, base="https://example.test/r")

    def test_a_missing_checksum_file_is_refused(self, tmp_path):
        with pytest.raises(v.VerifyError):
            v.download_verified(str(tmp_path), FakeSession({"yt-dlp": BODY}), base="https://example.test/r")

    def test_an_oversize_download_is_refused(self, tmp_path, monkeypatch):
        monkeypatch.setattr(v, "MAX_BINARY_BYTES", 4)
        session = FakeSession({"SHA2-256SUMS": sums(), "yt-dlp": BODY})
        with pytest.raises(v.VerifyError):
            v.download_verified(str(tmp_path), session, base="https://example.test/r")


class TestSelfUpdate:
    def _setup(self, tmp_path, monkeypatch, files):
        live = tmp_path / "vol" / "yt-dlp"
        live.parent.mkdir()
        live.write_text("old")
        monkeypatch.setattr(app, "YTDLP", str(live))
        monkeypatch.setattr(v, "RELEASES", "https://example.test/r")
        monkeypatch.setattr(app, "_SESSION", FakeSession(files))
        # download_verified's default base is bound at definition, so pass it through.
        orig = v.download_verified
        monkeypatch.setattr(v, "download_verified",
                            lambda d, s=None, base="https://example.test/r", tag="latest": orig(d, s, base, tag))
        return live

    def test_a_verified_release_replaces_the_binary_and_is_recorded(self, tmp_path, monkeypatch):
        live = self._setup(tmp_path, monkeypatch, {"SHA2-256SUMS": sums(), "yt-dlp": BODY})
        assert app._verified_update() is True
        assert live.read_bytes() == BODY
        assert v.read_recorded(str(live)) == DIGEST

    def test_a_tampered_release_leaves_the_binary_alone(self, tmp_path, monkeypatch):
        live = self._setup(tmp_path, monkeypatch, {"SHA2-256SUMS": sums("2" * 64), "yt-dlp": BODY})
        assert app._verified_update() is False
        assert live.read_text() == "old"
        assert [p.name for p in live.parent.iterdir()] == ["yt-dlp"]


class TestStartup:
    @pytest.fixture(autouse=True)
    def _trusted(self, monkeypatch):
        monkeypatch.setattr(app, "_BINARY_TRUSTED", True)

    def _pair(self, tmp_path, monkeypatch):
        dist = tmp_path / "dist" / "yt-dlp"
        dist.parent.mkdir()
        dist.write_bytes(BODY)
        v.record(str(dist), DIGEST)
        live = tmp_path / "vol" / "yt-dlp"
        live.parent.mkdir()
        monkeypatch.setattr(app, "YTDLP_DIST", str(dist))
        monkeypatch.setattr(app, "YTDLP", str(live))
        return dist, live

    def test_a_changed_copy_on_the_volume_is_replaced_by_the_image_copy(self, tmp_path, monkeypatch):
        dist, live = self._pair(tmp_path, monkeypatch)
        live.write_bytes(b"tampered")
        v.record(str(live), DIGEST)
        app._check_installed_binary()
        assert live.read_bytes() == BODY
        assert app._BINARY_TRUSTED is True

    def test_a_copy_with_no_record_is_replaced_too(self, tmp_path, monkeypatch):
        dist, live = self._pair(tmp_path, monkeypatch)
        live.write_bytes(b"from an older shim")
        app._check_installed_binary()
        assert live.read_bytes() == BODY
        assert v.read_recorded(str(live)) == DIGEST

    def test_an_intact_copy_is_left_alone(self, tmp_path, monkeypatch):
        dist, live = self._pair(tmp_path, monkeypatch)
        live.write_bytes(b"newer verified")
        v.record(str(live), hashlib.sha256(b"newer verified").hexdigest())
        app._check_installed_binary()
        assert live.read_bytes() == b"newer verified"

    def test_nothing_runs_when_the_image_copy_fails_its_own_record(self, tmp_path, monkeypatch):
        dist = tmp_path / "yt-dlp"
        dist.write_bytes(b"swapped")
        v.record(str(dist), DIGEST)
        monkeypatch.setattr(app, "YTDLP_DIST", str(dist))
        monkeypatch.setattr(app, "YTDLP", str(dist))
        app._check_installed_binary()
        assert app._BINARY_TRUSTED is False
        assert app._run(["--version"]) is None
