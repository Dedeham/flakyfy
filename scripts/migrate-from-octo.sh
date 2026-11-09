#!/usr/bin/env bash
# Moves an existing Octo install's .env to Flakyfy's names. Run it once in the Octo folder,
# after `docker compose down` and after checking out Flakyfy (see "Moving from Octo" in the README).
#
#   scripts/migrate-from-octo.sh           shows what would change
#   scripts/migrate-from-octo.sh --apply   changes it; .env.octo-backup keeps the original
#
# Nothing in the config or music folders is moved: Flakyfy reads Octo's folders as they are.
# Safe to run again. Values are never printed, because .env holds passwords.
set -euo pipefail

dir="$(cd "$(dirname "$0")/.." && pwd)"
apply=0
while [ $# -gt 0 ]; do
  case "$1" in
    --apply) apply=1; shift ;;
    --dir) dir="${2:?--dir needs a folder}"; shift 2 ;;
    *) printf 'migrate-from-octo: unknown option %s\n' "$1" >&2; exit 1 ;;
  esac
done

die() { printf 'migrate-from-octo: %s\n' "$*" >&2; exit 1; }
env_file="$dir/.env"
[ -f "$env_file" ] || die "no .env in $dir; there is nothing to move"

if command -v docker > /dev/null 2>&1 && docker ps --format '{{.Names}}' 2>/dev/null | grep -qx 'octo'; then
  die "the octo container is still running. Run 'docker compose down' with Octo's compose file first"
fi

value() { sed -n "s/^$1=//p" "$env_file" | tail -n 1 | tr -d '"\r'; }

# Blank means the default folder, and Octo's default is not Flakyfy's: without this line Flakyfy
# would start from an empty config.
config="$(value FLAKYFY_CONFIG_DIR)"
[ -n "$config" ] || config="$(value OCTO_CONFIG_DIR)"
[ -n "$config" ] || { [ ! -d "$dir/octo-config" ] || config=./octo-config; }

new="$(mktemp "$env_file.XXXXXX")"
trap 'rm -f "$new"' EXIT
awk -v config="$config" '
  NR == FNR {
    if ($0 ~ /^FLAKYFY_[A-Z_]*=./) { split($0, kv, "="); set[kv[1]] = 1 }
    next
  }
  /^OCTO_[A-Z_]*=/ {
    key = substr($0, 1, index($0, "=") - 1)
    sub(/^OCTO_/, "FLAKYFY_", key)
    if (key in set) next
    $0 = key substr($0, index($0, "="))
  }
  /^RADIO_SONIC_URL=/ { gsub(/octo-sonic/, "flakyfy-sonic") }
  /^UPDATES_REPO=winters27\/octo/ { next }
  /^FLAKYFY_CONFIG_DIR=/ {
    if (done) next
    $0 = "FLAKYFY_CONFIG_DIR=" config
    done = 1
  }
  { print }
  END { if (!done && config != "") print "FLAKYFY_CONFIG_DIR=" config }
' "$env_file" "$env_file" > "$new"

if cmp -s "$env_file" "$new"; then
  echo "Nothing to change: .env already uses Flakyfy's names."
  exit 0
fi

echo "Lines in .env that change (values hidden):"
diff "$env_file" "$new" | sed -n 's/^< \([^=]*\).*/  - \1/p; s/^> \([^=]*\).*/  + \1/p' || true

tag="$(sed -n 's/^\(OCTO\|FLAKYFY\)_IMAGE_TAG=//p' "$env_file" | tail -n 1)"
case "$tag" in
  ""|latest) ;;
  *) echo "Note: the image tag in .env is pinned. Flakyfy's releases have their own numbers, so check it exists or remove the line." ;;
esac

if [ "$apply" != 1 ]; then
  echo "Nothing was changed. Run again with --apply."
  exit 0
fi

[ -e "$env_file.octo-backup" ] || cp -p "$env_file" "$env_file.octo-backup"
chmod 600 "$new"
mv -f "$new" "$env_file"
trap - EXIT
echo "Done. The original is in .env.octo-backup. Next: docker compose pull && docker compose up -d --remove-orphans"
