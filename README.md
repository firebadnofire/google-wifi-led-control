# Google Wifi LED Control

Google Wifi LED Control is a native OpenWrt application for setting the built-in RGB status LED on one Google Wifi **Gale** router. Each installation controls only the router on which it runs; there is no discovery, fleet management, or remote-device control.

The project is intentionally static and small: LuCI stores validated settings in UCI, a narrow rpcd/ucode method asks the one-shot init service to apply them, and the Rust `gale-led` utility writes the three Linux LED brightness channels. No daemon remains running.

## Support status

| Component | Supported and tested baseline |
| --- | --- |
| Hardware | Google Wifi (Gale), board `google,wifi` |
| OpenWrt | 25.12.x; developed against 25.12.2 |
| Target | `ipq40xx/chromium` |
| Package architecture | `arm_cortex-a7_neon-vfpv4` for `gale-led`; `noarch` for LuCI |
| Package manager | `apk` |

Other Google Wifi or Nest Wifi models are not claimed to be compatible. OpenWrt 24.10 and older use a different package format and are not release targets for this project.

## Architecture

```text
LuCI form
  -> /etc/config/gale-led (UCI)
  -> gale-led rpcd/ucode object (fixed methods only)
  -> /etc/init.d/gale-led reload
  -> /usr/bin/gale-led apply
  -> /sys/class/leds/LED0_{Red,Green,Blue}
```

At boot, `/etc/init.d/gale-led` runs `gale-led apply` once and exits. The utility verifies all three channels before taking control, sets each `trigger` to `none`, and writes only `brightness`. It never writes `led_current`; Gale's observed default remains `100` with `max_current` `120`.

## Installation

Download both APKs, their `.asc` signatures, `SHA256SUMS`, and `SHA256SUMS.asc` from the same release. Verify them with the project's release-signing public key before installation:

```sh
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum -c SHA256SUMS
gpg --verify gale-led-1.0.0-r1.apk.asc gale-led-1.0.0-r1.apk
gpg --verify luci-app-gale-led-1.0.0-r1.apk.asc luci-app-gale-led-1.0.0-r1.apk
```

The exact filenames include OpenWrt's package release revision and should be taken from `SHA256SUMS`, not assumed from the examples. Copy the two APKs to the router, then install the utility first:

```sh
apk add --allow-untrusted ./gale-led-*.apk
apk add --allow-untrusted ./luci-app-gale-led-*.apk
/etc/init.d/rpcd restart
```

`--allow-untrusted` is needed because the detached GPG signature is an external release signature rather than an OpenWrt package-feed signature. Do not skip the GPG and SHA-256 checks.

## LuCI configuration

Open **System > Gale LED**. The page provides:

- an Enabled switch;
- synchronized native color picker and `#RRGGBB` field;
- a 0–100 percent brightness slider;
- a live color swatch; and
- a hardware-compatibility notice.

Use **Save & Apply** to commit UCI and apply the LED immediately. Browser validation is backed by validation in the privileged Rust utility, so bypassing the page cannot write an invalid color or brightness.

### Screenshots

Screenshots will be added after final visual verification against the supported LuCI theme. No mock screenshot is presented as device evidence.

## CLI

```sh
gale-led hardware
gale-led status
gale-led set '#A020F0'
gale-led set '#A020F0' --brightness 75
gale-led set-rgb 160 32 240
gale-led set-rgb 160 32 240 --brightness 75
gale-led off
gale-led apply
```

`hardware` and `status` emit JSON. Invalid arguments exit with status 2; configuration, sysfs, or I/O failures exit with status 1 and an actionable message. Direct `set`, `set-rgb`, and `off` calls are temporary: `apply`, a service reload, or the next boot restores UCI state.

## Manual UCI configuration

```uci
config led 'main'
	option enabled '1'
	option color '#A020F0'
	option brightness '100'
```

Apply a change with:

```sh
uci set gale-led.main.enabled='1'
uci set gale-led.main.color='#A020F0'
uci set gale-led.main.brightness='75'
uci commit gale-led
/etc/init.d/gale-led reload
```

`enabled` must be `0` or `1`, `color` must be exactly `#RRGGBB`, and `brightness` must be an integer from 0 through 100. A disabled configuration writes zero brightness to all channels.

## Building

The official OpenWrt SDK is a Linux x86-64 toolchain. Build on Linux (or WSL 2), not with a generic desktop ARM target. For the supported baseline:

```sh
curl -fLO https://downloads.openwrt.org/releases/25.12.2/targets/ipq40xx/chromium/openwrt-sdk-25.12.2-ipq40xx-chromium_gcc-14.3.0_musl_eabi.Linux-x86_64.tar.zst
echo '574197ecb6f0f858c76d674088a936cb5fbedfa6c352614643eb66e0b3084711  openwrt-sdk-25.12.2-ipq40xx-chromium_gcc-14.3.0_musl_eabi.Linux-x86_64.tar.zst' | sha256sum -c -
tar --zstd -xf openwrt-sdk-25.12.2-ipq40xx-chromium_gcc-14.3.0_musl_eabi.Linux-x86_64.tar.zst
cd openwrt-sdk-25.12.2-*
rustup target add armv7-unknown-linux-musleabihf
ln -s /absolute/path/to/google-wifi-led-control package/gale-led
printf '%s\n' 'CONFIG_PACKAGE_gale-led=m' 'CONFIG_PACKAGE_luci-app-gale-led=m' >> .config
make defconfig
make package/gale-led/compile V=s
```

Installable APKs are produced under `bin/packages/`. Dependencies include a Linux build toolchain, rustup with the `armv7-unknown-linux-musleabihf` standard library, `zstd`, GNU Make, Git, and standard download tools. Cargo is invoked by the OpenWrt package recipe with the SDK's target linker and dynamic musl configuration; this is not a generic desktop ARM build.

## Development and tests

Rust 1.85 or newer is required because the code uses Rust 2024 edition.

```sh
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
bash ./scripts/validate-static.sh
```

Unit tests cover valid and malformed hex colors, RGB bounds, brightness bounds and rounding, configuration parsing, missing hardware, fixture-backed sysfs writes, trigger takeover, and preservation of `led_current`. The fixture seam is internal to the library; the production CLI cannot redirect privileged writes to arbitrary paths.

For an idempotent local SDK build using the same pinned release inputs, run `bash scripts/build-openwrt.sh` under Linux or WSL. Artifacts are copied to `.openwrt-sdk/artifacts/`.

## Release process

`.forgejo/workflows/release.yml` runs for `vMAJOR.MINOR.PATCH` tags and manual builds of an existing tag. It:

1. verifies that the tag matches `Cargo.toml` and the OpenWrt package version;
2. runs formatting, tests, Clippy, and static validation;
3. downloads and SHA-256 verifies the official OpenWrt 25.12.2 `ipq40xx/chromium` SDK;
4. builds both self-contained packages through OpenWrt's package infrastructure without mutable feed inputs;
5. generates deterministic `SHA256SUMS` and armored detached GPG signatures for every APK and the checksum file;
6. creates or updates the Forgejo release with its automatic repository-scoped token; and
7. sends the exact same release metadata and bytes to `firebadnofire/google-wifi-led-control` through the GitHub REST API.

The workflow does not add a GitHub remote, push Git objects, or mirror branches, commits, tags, or refs. Forgejo handles source synchronization separately.

Required Forgejo secrets are `CI_KEY`, `CI_KEY_PASSPHRASE`, and `GH_KEY`. `GH_KEY` should be a fine-grained GitHub token restricted to the target repository with **Contents: write** permission. The runner needs Docker and outbound HTTPS access. The job uses a digest-pinned Node 20/Debian Bookworm image, installs its build dependencies, and checksum-verifies a pinned rustup installer before selecting Rust 1.90.0 with rustfmt, Clippy, and the ARM target.

To exercise the complete workflow locally on a Linux Docker host, install and verify a Forgejo Runner binary, export an armored disposable signing key as `CI_KEY` and its passphrase as `CI_KEY_PASSPHRASE`, then run:

```sh
FORGEJO_RUNNER=/path/to/forgejo-runner bash scripts/test-workflow-local.sh v1.0.0
```

Local mode binds the current checkout into the job container, runs the same validation, SDK, package, checksum, and signature steps, and always skips checkout, cache publication, and both release APIs. Generated files remain in the ignored `target/`, `.cache/`, `.openwrt-sdk/`, and `release/` paths. The `:z` bind-mount label supports SELinux-enforcing Docker hosts.

## Security model

- rpcd ACLs grant read access only to this UCI package plus `hardware`/`status`, and write access only to this UCI package plus `apply`.
- The ucode object accepts no path, command, color, or brightness arguments. Its commands are fixed absolute paths.
- Rust revalidates every persisted value and all expected sysfs attributes before the first write.
- Normal operation changes `trigger` and `brightness` only. Current limiting is left to the kernel/firmware defaults.
- There are no listeners, new ports, remote-control APIs, embedded credentials, or arbitrary shell endpoints.

## Troubleshooting

- **“unsupported Gale LED hardware”**: run `gale-led hardware` and confirm all three `LED0_Red`, `LED0_Green`, and `LED0_Blue` directories expose `brightness`, `max_brightness`, and `trigger`.
- **Invalid configuration**: run `uci show gale-led`, correct the three required options, commit, and reload the service.
- **LuCI page missing after install**: restart rpcd and inspect `logread` before reinstalling packages.
- **Apply fails**: run `gale-led status`, `/etc/init.d/gale-led reload`, and `logread`, preserving the first concrete error.
- **Package rejected**: confirm the router is OpenWrt 25.12.x `ipq40xx/chromium`, uses `apk`, and that both checksum and GPG verification succeeded.

## Hardware validation

The final `1.0.0-r1` APKs were installed and exercised on a real Google Wifi (Gale) running OpenWrt 25.12.2, target `ipq40xx/chromium`. Package simulation and installation, RGB primary colors, mixed colors, global brightness scaling, off, invalid CLI input, UCI apply/disable/restore, one-shot service reload, rpcd hardware/status/apply, anonymous-session denial, preservation of `led_current`, and configuration restoration after a confirmed reboot all passed. The final retained state was enabled, `#A020F0`, 100%, producing sysfs brightness values `160 32 240`, with all triggers set to `none` and no resident daemon.

The authenticated LuCI route reached the correct **OpenWrt | Gale LED** view, but browser automation was blocked by another Chrome extension before the rendered controls could be inspected or submitted. Physical light appearance also requires a person beside the router; this run verified the real sysfs outputs but did not claim a human visual color judgment. Local CI remains independent of the private test router.

## License

MIT. See [LICENSE](LICENSE).
