#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

python3 - <<'PY'
import json
from pathlib import Path

for path in sorted(Path("luci/root/usr/share").rglob("*.json")):
    with path.open(encoding="utf-8") as handle:
        json.load(handle)
    print(f"valid JSON: {path}")
PY

bash -n files/gale-led.init
bash -n scripts/build-openwrt.sh
bash -n scripts/publish-release.sh
bash -n scripts/test-workflow-local.sh
bash -n scripts/validate-static.sh

if grep -RInE '(id_ed25519|owrt4\.internal|BEGIN (RSA|OPENSSH|EC) PRIVATE KEY)' \
	--exclude-dir=.git --exclude-dir=.cache --exclude-dir=.openwrt-sdk \
	--exclude-dir=release --exclude-dir=target \
	--exclude='README.md' --exclude='validate-static.sh' .; then
	echo 'private test-host or key material reference found in project artifacts' >&2
	exit 1
fi

if grep -nE 'git[[:space:]]+(push|mirror)|push[[:space:]]+(branches|commits|refs)' \
	.forgejo/workflows/release.yml; then
	echo 'prohibited Git mirroring or push logic found in release workflow' >&2
	exit 1
fi

echo 'static validation passed'
