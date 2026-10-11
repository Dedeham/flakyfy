"""Trust for the yt-dlp binary: it is installed only when its SHA2-256 matches the checksum list
the yt-dlp project publishes with each release.

Three places use this. The image build checks the binary it bakes in (Dockerfile). The shim's
self-update downloads a release and installs it only after the same check. And at startup the
copy that is about to run is compared with the hash recorded when it was installed, so a file
changed on the volume afterwards is replaced by the image's verified copy instead of run.

Nothing here executes the downloaded file before it has been checked.
"""
from __future__ import annotations

import hashlib
import os
import tempfile
import urllib.parse
from typing import Optional

import requests

RELEASES = os.environ.get("YTDLP_RELEASES_URL", "https://github.com/yt-dlp/yt-dlp/releases")
ASSET = "yt-dlp"
SUMS = "SHA2-256SUMS"
# A release asset is a few MB; the cap stops a bad redirect from filling the disk or memory.
MAX_BINARY_BYTES = 64 * 1024 * 1024
MAX_SUMS_BYTES = 256 * 1024


class VerifyError(Exception):
    """The download could not be checked, or did not match its published checksum."""


def parse_sums(text: str, asset: str = ASSET) -> Optional[str]:
    """The hex SHA2-256 listed for `asset` in a SHA2-256SUMS file, or None."""
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1].lstrip("*") == asset:
            digest = parts[0].lower()
            if len(digest) == 64 and all(c in "0123456789abcdef" for c in digest):
                return digest
    return None


def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sidecar(path: str) -> str:
    return path + ".sha256"


def read_recorded(path: str) -> Optional[str]:
    try:
        with open(sidecar(path)) as fh:
            value = fh.read().split()[0].lower()
        return value if len(value) == 64 else None
    except (OSError, IndexError):
        return None


def record(path: str, digest: str) -> None:
    with open(sidecar(path), "w") as fh:
        fh.write(digest + "\n")


MAX_REDIRECTS = 5
_REDIRECTS = (301, 302, 303, 307, 308)


def _https(url: str) -> bool:
    return url.lower().startswith("https://")


def _get(session, url: str, limit: int, timeout=(8, 60)) -> bytes:
    """GET over https only. Redirects are followed by hand, and one that leaves https (or goes
    on too long) is refused, so a downgrade cannot hand the checksum or the binary to plain HTTP."""
    try:
        for _ in range(MAX_REDIRECTS + 1):
            if not _https(url):
                raise VerifyError("refusing a non-https address")
            with session.get(url, stream=True, timeout=timeout, allow_redirects=False) as resp:
                if resp.status_code in _REDIRECTS:
                    url = urllib.parse.urljoin(url, resp.headers.get("Location", ""))
                    continue
                if resp.status_code != 200:
                    raise VerifyError(f"HTTP {resp.status_code}")
                data = bytearray()
                for chunk in resp.iter_content(1 << 16):
                    data.extend(chunk)
                    if len(data) > limit:
                        raise VerifyError("download larger than allowed")
                return bytes(data)
        raise VerifyError("too many redirects")
    except requests.RequestException as e:
        raise VerifyError(f"download failed: {type(e).__name__}") from e


def download_verified(directory: str, session=None, base: str = RELEASES, tag: str = "latest") -> tuple[str, str]:
    """Download the release binary into `directory` and return (path, sha256) once its hash
    matches the published list. The file is removed and VerifyError raised otherwise."""
    session = session or requests
    where = "latest/download" if tag == "latest" else f"download/{tag}"
    sums = _get(session, f"{base}/{where}/{SUMS}", MAX_SUMS_BYTES).decode("utf-8", "replace")
    expected = parse_sums(sums)
    if expected is None:
        raise VerifyError("no checksum listed for the binary")
    body = _get(session, f"{base}/{where}/{ASSET}", MAX_BINARY_BYTES)
    actual = hashlib.sha256(body).hexdigest()
    if actual != expected:
        raise VerifyError("checksum does not match the published one")
    fd, tmp = tempfile.mkstemp(prefix=".yt-dlp-", dir=directory)
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(body)
        os.chmod(tmp, 0o755)
    except BaseException:
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise
    return tmp, actual
