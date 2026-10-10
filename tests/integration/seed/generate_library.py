#!/usr/bin/env python3
"""Write the harness library: short generated sine tones as tagged WAV files.

Nothing here is recorded audio, so nothing is copyrighted. Output is deterministic.
Usage: generate_library.py <output-dir>
"""
import math
import struct
import sys
from pathlib import Path

RATE = 8000
SECONDS = 2

# (artist, album, [(title, frequency Hz)]) -- 3 artists, 4 albums, 12 tracks.
LIBRARY = [
    ("Tone Generator", "Sine Waves", [("Low A", 110), ("Middle A", 220), ("Concert A", 440), ("High A", 880)]),
    ("Tone Generator", "Fifths", [("Root", 262), ("Fifth", 392), ("Octave", 523)]),
    ("Silent Partners", "Quiet Room", [("Hush", 0), ("Murmur", 0), ("Still", 0)]),
    ("Test Pattern", "Calibration", [("One Kilohertz", 1000), ("Two Kilohertz", 2000)]),
]


def info_chunk(tags):
    body = b"INFO"
    for key, value in tags:
        data = value.encode("utf-8") + b"\0"
        if len(data) % 2:
            data += b"\0"
        body += key + struct.pack("<I", len(data)) + data
    return b"LIST" + struct.pack("<I", len(body)) + body


def write_wav(path, freq, tags):
    n = RATE * SECONDS
    samples = bytearray()
    for i in range(n):
        v = int(8000 * math.sin(2 * math.pi * freq * i / RATE)) if freq else 0
        samples += struct.pack("<h", v)
    fmt = struct.pack("<HHIIHH", 1, 1, RATE, RATE * 2, 2, 16)
    chunks = (b"fmt " + struct.pack("<I", len(fmt)) + fmt
              + b"data" + struct.pack("<I", len(samples)) + bytes(samples)
              + info_chunk(tags))
    path.write_bytes(b"RIFF" + struct.pack("<I", 4 + len(chunks)) + b"WAVE" + chunks)


def main(out):
    out = Path(out)
    count = 0
    for artist, album, tracks in LIBRARY:
        folder = out / artist / album
        folder.mkdir(parents=True, exist_ok=True)
        for number, (title, freq) in enumerate(tracks, 1):
            tags = [(b"INAM", title), (b"IART", artist), (b"IPRD", album),
                    (b"ITRK", str(number)), (b"ICRD", "2024"), (b"IGNR", "Test")]
            write_wav(folder / f"{number:02d} {title}.wav", freq, tags)
            count += 1
    print(f"wrote {count} tracks to {out}")


if __name__ == "__main__":
    main(sys.argv[1])
