#!/usr/bin/env bash
set -euo pipefail

provider=${1:-}
artifact_dir=${2:-}

if [[ ! $provider =~ ^(forgejo|github)$ ]]; then
	echo "usage: $0 forgejo|github ARTIFACT_DIRECTORY" >&2
	exit 2
fi
if [[ ! -d $artifact_dir ]]; then
	echo "artifact directory does not exist: $artifact_dir" >&2
	exit 2
fi
: "${RELEASE_TAG:?RELEASE_TAG is required}"
: "${RELEASE_NAME:?RELEASE_NAME is required}"
: "${RELEASE_BODY:?RELEASE_BODY is required}"
: "${RELEASE_PRERELEASE:?RELEASE_PRERELEASE is required}"

response_file=$(mktemp)
payload_file=$(mktemp)
cleanup() {
	rm -f -- "$response_file" "$payload_file"
}
trap cleanup EXIT

if [[ $provider == forgejo ]]; then
	: "${FORGEJO_API_URL:?FORGEJO_API_URL is required}"
	: "${FORGEJO_REPOSITORY:?FORGEJO_REPOSITORY is required}"
	: "${FORGEJO_TOKEN:?FORGEJO_TOKEN is required}"
	api_base="${FORGEJO_API_URL%/}/repos/${FORGEJO_REPOSITORY}"
	auth_header="Authorization: token ${FORGEJO_TOKEN}"
	accept_header="Accept: application/json"
else
	: "${GH_KEY:?GH_KEY is required}"
	api_base='https://api.github.com/repos/firebadnofire/google-wifi-led-control'
	auth_header="Authorization: Bearer ${GH_KEY}"
	accept_header='Accept: application/vnd.github+json'
fi

api_request() {
	local method=$1
	local url=$2
	shift 2
	local status
	status=$(curl --silent --show-error \
		--request "$method" \
		--header "$auth_header" \
		--header "$accept_header" \
		--output "$response_file" \
		--write-out '%{http_code}' \
		"$@" "$url")
	printf '%s' "$status"
}

require_status() {
	local actual=$1
	shift
	for expected in "$@"; do
		[[ $actual == "$expected" ]] && return 0
	done
	echo "$provider API returned unexpected HTTP status $actual" >&2
	jq -r '.message // .error // "No API error message was returned"' "$response_file" >&2 || true
	exit 1
}

encoded_tag=$(jq -rn --arg value "$RELEASE_TAG" '$value | @uri')
status=$(api_request GET "$api_base/releases/tags/$encoded_tag")

jq -n \
	--arg tag "$RELEASE_TAG" \
	--arg name "$RELEASE_NAME" \
	--arg body "$RELEASE_BODY" \
	--argjson prerelease "$RELEASE_PRERELEASE" \
	'{tag_name:$tag,name:$name,body:$body,draft:false,prerelease:$prerelease}' >"$payload_file"

if [[ $status == 404 ]]; then
	status=$(api_request POST "$api_base/releases" \
		--header 'Content-Type: application/json' --data-binary "@$payload_file")
	require_status "$status" 201
elif [[ $status == 200 ]]; then
	release_id=$(jq -er '.id' "$response_file")
	status=$(api_request PATCH "$api_base/releases/$release_id" \
		--header 'Content-Type: application/json' --data-binary "@$payload_file")
	require_status "$status" 200
else
	require_status "$status" 200 404
fi

release_id=$(jq -er '.id' "$response_file")
if [[ $provider == github ]]; then
	upload_base=$(jq -er '.upload_url | sub("\\{.*$"; "")' "$response_file")
fi

# Refresh the release so duplicate detection sees all assets after a retry.
status=$(api_request GET "$api_base/releases/$release_id")
require_status "$status" 200

mapfile -t assets < <(find "$artifact_dir" -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort)
if (( ${#assets[@]} == 0 )); then
	echo "no release assets found in $artifact_dir" >&2
	exit 1
fi

for name in "${assets[@]}"; do
	existing_id=$(jq -r --arg name "$name" '.assets[]? | select(.name == $name) | .id' "$response_file" | head -n 1)
	if [[ -n $existing_id ]]; then
		status=$(api_request DELETE "$api_base/releases/assets/$existing_id")
		require_status "$status" 204
	fi

	encoded_name=$(jq -rn --arg value "$name" '$value | @uri')
	path="$artifact_dir/$name"
	if [[ $provider == forgejo ]]; then
		status=$(api_request POST "$api_base/releases/$release_id/assets?name=$encoded_name" \
			--form "attachment=@$path")
		require_status "$status" 201
	else
		content_type=$(file --brief --mime-type "$path")
		status=$(api_request POST "$upload_base?name=$encoded_name" \
			--header "Content-Type: $content_type" --data-binary "@$path")
		require_status "$status" 201
	fi

	status=$(api_request GET "$api_base/releases/$release_id")
	require_status "$status" 200
done

echo "Published ${#assets[@]} assets to $provider release $RELEASE_TAG"
