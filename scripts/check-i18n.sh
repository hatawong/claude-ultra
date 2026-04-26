#!/bin/bash
# Verify all locale JSON files have the same key set as en.json.
#
# Default mode: report mismatches as warnings, always exit 0 — the project has
# pre-existing inconsistencies that are tracked separately (long-tail i18n
# maintenance). Use this mode in pre-commit / dev to surface drift after a
# new key is added without blocking commits.
#
# --strict mode: exit 1 on any mismatch. Use in CI gate once the existing
# drift has been cleaned up.
#
# Usage: bash scripts/check-i18n.sh [--strict]
#   or:  bun run check-i18n

set -e

STRICT=0
[ "${1:-}" = "--strict" ] && STRICT=1

DIR="$(cd "$(dirname "$0")/.." && pwd)/src/locales"
REF="$DIR/en.json"

if [ ! -f "$REF" ]; then
  echo "ERROR: reference $REF not found" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq required but not installed (brew install jq)" >&2
  exit 1
fi

# Extract all dotted key paths from a JSON file (scalars only, sorted).
keys() {
  jq -r 'paths(scalars) | join(".")' "$1" | sort
}

ref_keys=$(keys "$REF")
mismatch=0

for f in "$DIR"/*.json; do
  [ "$f" = "$REF" ] && continue
  name=$(basename "$f" .json)
  cur=$(keys "$f")
  missing=$(comm -23 <(echo "$ref_keys") <(echo "$cur"))
  extra=$(comm -13 <(echo "$ref_keys") <(echo "$cur"))
  if [ -n "$missing" ] || [ -n "$extra" ]; then
    echo "[$name] mismatch:"
    [ -n "$missing" ] && echo "  missing keys (vs en.json):" && echo "$missing" | sed 's/^/    /'
    [ -n "$extra" ]   && echo "  extra keys (not in en.json):" && echo "$extra" | sed 's/^/    /'
    mismatch=1
  else
    echo "[$name] ok"
  fi
done

if [ $mismatch -eq 0 ]; then
  echo "All locales consistent with en.json."
  exit 0
fi

if [ $STRICT -eq 1 ]; then
  exit 1
fi

echo
echo "(--strict not set; exiting 0 despite mismatches.)"
exit 0
