#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n1)
tag=${1:-v$version}
runner=${FORGEJO_RUNNER:-forgejo-runner}

if [[ ! $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$ ]]; then
	echo "release tag must be vMAJOR.MINOR.PATCH with an optional SemVer prerelease" >&2
	exit 1
fi

command -v "$runner" >/dev/null || {
	echo "forgejo-runner not found; set FORGEJO_RUNNER to its verified binary path" >&2
	exit 1
}
command -v docker >/dev/null || {
	echo "docker is required for local workflow execution" >&2
	exit 1
}
docker info >/dev/null || {
	echo "the Docker daemon is unavailable to the current user" >&2
	exit 1
}
: "${CI_KEY:?CI_KEY must contain a base64-encoded test signing secret key}"
: "${CI_KEY_PASSPHRASE:?CI_KEY_PASSPHRASE is required}"

cd "$root"
export CI_KEY CI_KEY_PASSPHRASE
"$runner" exec \
	--directory "$root" \
	--workflows .forgejo/workflows/release.yml \
	--event push \
	--env "LOCAL_RELEASE_TAG=$tag" \
	--env LOCAL_RELEASE_DRY_RUN=1 \
	--env LOCAL_WORKSPACE=/local-src \
	--secret CI_KEY \
	--secret CI_KEY_PASSPHRASE \
	--image node:20-bookworm@sha256:8f693eaa7e0a8e71560c9a82b55fd54c2ae920a2ba5d2cde28bac7d1c01c9ba5 \
	--container-opts "--volume=$root:/local-src:z"
