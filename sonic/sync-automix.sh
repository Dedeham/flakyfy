#!/usr/bin/env bash
# Copies the transition analysis from the Flakyfy apps' audio engine into
# src/automix, with the shared test vectors and the profile fixture the
# apps read, so flakyfy-sonic analyzes songs exactly as the players do.
#
#   ./sync-automix.sh [path to the FlakyfyPlayer checkout]
#
# The default is FLAKYFYPLAYER, else ../../FlakyfyPlayer beside this repo. Run
# `cargo test` afterwards: the parity test checks the copy against the
# vectors, and the fixture test against the apps' profile fixture.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
apps="${1:-${FLAKYFYPLAYER:-$here/../../FlakyfyPlayer}}"
engine="$apps/audio-engine/src/automix"
resources="$apps/shared/core/src/commonTest/resources"

if [[ ! -f "$engine/profile.rs" ]]; then
  echo "No audio engine analysis at $engine" >&2
  exit 1
fi

for file in dsp.rs envelope.rs features.rs profile.rs synthetic.rs; do
  # Line endings as the repository keeps them.
  tr -d '\r' < "$engine/$file" > "$here/src/automix/$file"
done
for file in automix-vectors.json transition-profile.json; do
  tr -d '\r' < "$resources/$file" > "$here/testdata/$file"
done
echo "Copied the analysis from $apps ($(git -C "$apps" rev-parse --short HEAD 2>/dev/null || echo 'not a git checkout'))."
