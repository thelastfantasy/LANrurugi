#!/usr/bin/env bash
# `apt-get update` for GitHub-hosted runners, with a fallback mirror.
#
# The runner image points apt at azure.archive.ubuntu.com / archive.ubuntu.com /
# security.ubuntu.com. On 2026-11-11 every *.ubuntu.com name stopped resolving on the runners for
# hours (three consecutive CI runs failed in this step) while other hosts resolved fine, so when
# the first update cannot fetch everything, the Ubuntu sources are switched to a mirror outside
# that domain and the update is retried once.
set -euo pipefail

FALLBACK="${CI_APT_FALLBACK_MIRROR:-http://mirrors.edge.kernel.org/ubuntu/}"

# `--error-on=any`: without it apt-get exits 0 even when every index fetch failed. Plain http is
# what the stock sources use too; apt verifies the archive signatures either way.
if sudo apt-get update --error-on=any; then
  exit 0
fi

echo "::warning::Ubuntu archive unreachable from this runner; retrying apt-get update via ${FALLBACK}"
# The image's deb822 sources use `mirror+file:/etc/apt/apt-mirrors.txt`; any plain *.ubuntu.com URI
# (sources.list or other .sources files) is rewritten as well.
if [ -f /etc/apt/apt-mirrors.txt ]; then
  echo "${FALLBACK}" | sudo tee /etc/apt/apt-mirrors.txt >/dev/null
fi
for file in /etc/apt/sources.list /etc/apt/sources.list.d/*.sources /etc/apt/sources.list.d/*.list; do
  [ -f "$file" ] || continue
  sudo sed -i -E "s#https?://([a-z0-9.-]+\.)?(archive|security)\.ubuntu\.com/ubuntu/?#${FALLBACK}#g" "$file"
done
sudo apt-get update --error-on=any
