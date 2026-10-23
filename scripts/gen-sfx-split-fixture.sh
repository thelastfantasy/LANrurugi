#!/usr/bin/env bash
# Regenerates test-fixtures/archives/sfx-split.
#
# The SFX .exe and .001 files are produced by the 7z CLI using p7zip's bundled 7zCon.sfx
# module. This is test-fixture generation only: the application runtime does not invoke 7z.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_DIR="$REPO_ROOT/test-fixtures/archives/sfx-split"
SAMPLE_ZIP="$REPO_ROOT/test-fixtures/archives/sample.zip"

rm -rf "$FIXTURE_DIR"
mkdir -p "$FIXTURE_DIR"
cd "$FIXTURE_DIR"

7z e "$SAMPLE_ZIP" page1.png page2.png >/dev/null
echo "SFX split fixture" > readme.txt

# 10KB volume size; for the current small content this produces a single .001 part.
7z a -tzip -sfx -v10k sfxsplit.exe page1.png page2.png readme.txt >/dev/null

echo "Generated:"
ls -l "$FIXTURE_DIR"
