"""Global cap on concurrent yt-dlp processes, with a slice reserved for
interactive work.

Flakyfy prewarms upcoming tracks in the background. Those resolves are
fire-and-forget, but they used to compete on equal terms with a user pressing
play, so a burst could leave an interactive resolve queued behind eight
background ones.

Background callers must clear an outer semaphore sized (total - reserve) before
they reach the main gate, so background can never hold more than that many slots
and `reserve` slots stay reachable by an interactive request. Interactive
callers take the main gate directly and are never blocked by the outer one.

Acquire and release are paired on one object on purpose. The failure mode of a
hand-rolled two-semaphore scheme is a mismatched release: forget to hand back
the outer permit and background capacity decays toward zero, hand back a permit
that was never taken and the gate silently stops bounding anything. Both are
invisible in production until the box falls over.

Kept free of any app.py import so it stays cheap to unit test: importing app.py
initialises a sqlite meta cache and pulls in flask and requests.
"""
from __future__ import annotations

import threading
import time


class TieredGate:
    """A counting gate where background work cannot consume every slot."""

    def __init__(self, total: int, reserve: int):
        # Clamp rather than trust the environment. reserve >= total would leave
        # background with a zero-capacity semaphore, which is not "background is
        # deprioritised" but "background deadlocks until it times out", and the
        # symptom (discovery quietly stops working) is far from the cause.
        total = max(1, total)
        reserve = min(max(0, reserve), total - 1)
        self.total = total
        self.reserve = reserve
        self._main = threading.Semaphore(total)
        self._bg = threading.Semaphore(total - reserve)

    def acquire(self, bg: bool, timeout: float) -> bool:
        """Take a slot. Returns False if `timeout` elapsed without one."""
        if not bg:
            return self._main.acquire(timeout=timeout)

        # One budget spanning both acquisitions, not `timeout` each. Two
        # independent waits would let a background request block for twice the
        # caller's own HTTP timeout, holding a worker thread for a response
        # nobody is left to read.
        deadline = time.monotonic() + timeout
        if not self._bg.acquire(timeout=timeout):
            return False
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not self._main.acquire(timeout=remaining):
            self._bg.release()
            return False
        return True

    def release(self, bg: bool) -> None:
        """Hand a slot back. `bg` must match the value passed to acquire."""
        self._main.release()
        if bg:
            self._bg.release()
