#!/usr/bin/env bash
# Dumps every source + config file in the workspace into one file at the repo root.
set -euo pipefail
OUT="ferromix_dump.txt"
cd "$(dirname "$0")"
: > "$OUT"

{
  echo "=== FERROMIX SOURCE DUMP ==="
  echo "generated: $(date -Is)"
  echo "toolchain: $(rustc --version 2>/dev/null || echo '?')"
  echo
  echo "=== TREE ==="
  # show layout without target/.git
  find . -type f \
    \( -name '*.rs' -o -name '*.toml' \) \
    -not -path './target/*' -not -path './.git/*' | sort
  echo
} >> "$OUT"

# Dump each file with a clear header.
find . -type f \
  \( -name '*.rs' -o -name 'Cargo.toml' -o -name '*.service' \) \
  -not -path './target/*' -not -path './.git/*' \
  | sort | while read -r f; do
    {
      echo
      echo "############################################################"
      echo "### FILE: $f"
      echo "############################################################"
      cat "$f"
      echo
    } >> "$OUT"
done

echo "Wrote $OUT ($(wc -l < "$OUT") lines, $(du -h "$OUT" | cut -f1))"
