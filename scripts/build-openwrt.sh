#!/usr/bin/env bash
set -euo pipefail

project_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export PATH="$HOME/.cargo/bin:$PATH"
sdk_version=25.12.2
sdk_file=openwrt-sdk-25.12.2-ipq40xx-chromium_gcc-14.3.0_musl_eabi.Linux-x86_64.tar.zst
sdk_sha256=574197ecb6f0f858c76d674088a936cb5fbedfa6c352614643eb66e0b3084711
sdk_url="https://downloads.openwrt.org/releases/$sdk_version/targets/ipq40xx/chromium/$sdk_file"
cache="${XDG_CACHE_HOME:-/tmp}/gale-led/$sdk_file"
output="$project_root/.openwrt-sdk/artifacts"

for command in curl sha256sum tar zstd git make gcc g++ cargo rustup; do
	command -v "$command" >/dev/null || {
		echo "required build command missing: $command" >&2
		exit 1
	}
done

mkdir -p "$(dirname "$cache")" "$output"
if [[ ! -f $cache ]]; then
	curl --fail --location --retry 3 --output "$cache" "$sdk_url"
fi
printf '%s  %s\n' "$sdk_sha256" "$cache" | sha256sum --check --strict

build_root=$(mktemp -d /tmp/gale-led-sdk-build.XXXXXX)
cleanup() {
	rm -rf -- "$build_root"
}
trap cleanup EXIT

tar --zstd --extract --file "$cache" --directory "$build_root"
sdk=$(find "$build_root" -mindepth 1 -maxdepth 1 -type d -name 'openwrt-sdk-*' -print -quit)
if [[ -z $sdk ]]; then
	echo 'the verified archive did not contain an OpenWrt SDK directory' >&2
	exit 1
fi

cd "$sdk"
rustup target add armv7-unknown-linux-musleabihf
mkdir -p package/gale-led
(
	cd "$project_root"
	tar --exclude='./.git' --exclude='./target' --exclude='./.openwrt-sdk' \
		--exclude='./release' --create --file - .
) | tar --extract --file - --directory package/gale-led

printf '%s\n' \
	'CONFIG_PACKAGE_gale-led=m' \
	'CONFIG_PACKAGE_luci-app-gale-led=m' >> .config
make defconfig
make package/gale-led/clean
make package/gale-led/compile V=s

find "$output" -maxdepth 1 -type f \
	\( -name 'gale-led-*.apk' -o -name 'luci-app-gale-led-*.apk' \) -delete
mapfile -t packages < <(find bin/packages -type f \
	\( -name 'gale-led-*.apk' -o -name 'luci-app-gale-led-*.apk' \) | LC_ALL=C sort)
if (( ${#packages[@]} != 2 )); then
	printf 'expected two package artifacts, found %d\n' "${#packages[@]}" >&2
	exit 1
fi
cp -- "${packages[@]}" "$output/"
printf 'OpenWrt packages copied to %s\n' "$output"
printf '  %s\n' "${packages[@]##*/}"
