#!/usr/bin/env bash
# Build the panel app (app/) for the panel (aarch64) with Buildroot's Rust
# toolchain (BR2_PACKAGE_HOST_RUSTC, prebuilt) and its cross linker.
# Run as root in Linux/WSL after tools/build_image.sh has built host-rustc.
#
# Usage: tools/build_app.sh            -> $WORK/target/aarch64-unknown-linux-gnu/release/nspanel-app
#        GPU=1 tools/build_app.sh      -> the same with the UI on the GPU (feature "gpu"):
#                                         $WORK-gpu/target/.../nspanel-app, links libgbm from the
#                                         web bundle's Buildroot output (BRWEB, run with its libraries)
# Run on the panel: copy the binary over SSH and start it (it takes over the display).
set -euo pipefail

BROUT="${BROUT:-/root/nspanel/br-out}"
BRWEB="${BRWEB:-/root/nspanel/br-web}"
GPU="${GPU:-0}"
if [ "$GPU" = 1 ]; then
    WORK="${WORK:-/root/nspanel/app-build-gpu}"
else
    WORK="${WORK:-/root/nspanel/app-build}"
fi
export PATH="$BROUT/host/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export CARGO_HOME="${CARGO_HOME:-/root/nspanel/cargo-home}"
export CARGO_TARGET_DIR="$WORK/target"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$BROUT/host/bin/aarch64-linux-gcc"
# Native libraries (fontconfig) come from Buildroot's staging tree; its
# pkg-config wrapper already points at the target sysroot.
export PKG_CONFIG="$BROUT/host/bin/pkg-config"
export PKG_CONFIG_ALLOW_CROSS=1
# C/C++ parts (wake word engine) with Buildroot's cross compilers (they use the sysroot).
export CC_aarch64_unknown_linux_gnu="$BROUT/host/bin/aarch64-linux-gcc"
export CXX_aarch64_unknown_linux_gnu="$BROUT/host/bin/aarch64-linux-g++"
export AR_aarch64_unknown_linux_gnu="$BROUT/host/bin/aarch64-linux-ar"

REPO="${REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
mkdir -p "$WORK/src"
# Copy the sources out of the (Windows) repository folder; keep the target dir.
rm -rf "$WORK/src"
cp -r "$REPO/app" "$WORK/src"
rm -rf "$WORK/src/target"

cd "$WORK/src"
# The build takes the dependencies exactly as app/Cargo.lock pins them (cargo build --locked below: a build that
# would change the lock stops). After a change in Cargo.toml, UPDATE_LOCK=1 resolves them again first and copies
# the new lock into the repository.
if [ "${UPDATE_LOCK:-0}" = 1 ]; then
    cargo metadata --format-version 1 > /dev/null
    cp Cargo.lock "$REPO/app/Cargo.lock"
fi
FEATURES=()
if [ "$GPU" = 1 ]; then
    # Only libgbm comes from the web bundle (the panel loads Mesa from there at
    # run time); everything else links against the image's own libraries.
    mkdir -p "$WORK/gbm-link"
    ln -sf "$BRWEB"/staging/usr/lib/libgbm.so* "$WORK/gbm-link/"
    # DT_RPATH (not RUNPATH) so that libgbm's own dependencies and the libraries
    # EGL opens later are looked up in the web bundle too; child processes
    # (wpa_cli, iw, ...) keep the image's libraries (no LD_LIBRARY_PATH).
    export RUSTFLAGS="${RUSTFLAGS:-} -L native=$WORK/gbm-link -C link-arg=-Wl,-rpath-link,$BRWEB/staging/usr/lib -C link-arg=-Wl,--disable-new-dtags,-rpath,/opt/panelos-web/usr/lib"
    FEATURES=(--features gpu)
    # Skia comes prebuilt (skia-bindings, under Slint's Skia renderer). skia-bindings would download it from GitHub
    # without a checksum, and without a download for its key it would build Skia from sources it downloads unchecked.
    # So the key it will use is worked out here first, as its build script makes it (binary_cache/binaries.rs key():
    # the first 20 characters of its rust-skia commit, the target, its Skia features sorted, "static", "debug"), and
    # it must be pinned in tools/skia-binaries.sha256. The archive is fetched, checked against the pinned SHA-256 and
    # handed to skia-bindings as the one file it reads (file://): no other Skia gets into the build.
    skia=$(cargo tree --locked --target aarch64-unknown-linux-gnu "${FEATURES[@]}" -i skia-bindings -e features --depth 0 --prefix none --format '{p}|{f}')
    SKIA_VERSION=$(sed -n 's/^skia-bindings v\([^|]*\)|.*/\1/p' <<< "$skia")
    skia_features=$(sed -n 's/^skia-bindings v[^|]*|//p' <<< "$skia" | tr ',' '\n' | sed -n \
        -e 's/^jpeg-encode$/jpege/p' -e 's/^jpeg-decode$/jpegd/p' -e 's/^webp-encode$/webpe/p' -e 's/^webp-decode$/webpd/p' \
        -e 's/^embed-freetype$/ftembed/p' -e 's/^freetype-woff2$/ftwoff2/p' \
        -e '/^\(pdf\|gl\|egl\|wayland\|x11\|vulkan\|metal\|d3d\|ganesh\|graphite\|textlayout\|svg\|skottie\)$/p' \
        | LC_ALL=C sort | paste -sd- -)
    skia_vcs=("$CARGO_HOME"/registry/src/*/skia-bindings-"$SKIA_VERSION"/.cargo_vcs_info.json)
    skia_vcs=${skia_vcs[0]}
    skia_commit=$(sed -n 's/.*"sha1": *"\([0-9a-f]\{40\}\)".*/\1/p' "$skia_vcs" 2> /dev/null || true)
    if [ -z "$SKIA_VERSION" ] || [ -z "$skia_commit" ]; then
        echo "build_app: cannot tell which Skia binaries skia-bindings wants ($skia_vcs)" >&2
        exit 1
    fi
    SKIA_KEY="${skia_commit:0:20}-aarch64-unknown-linux-gnu${skia_features:+-$skia_features}"
    case "${RUSTFLAGS:-}" in *+crt-static*) SKIA_KEY+=-static ;; esac
    if [ -n "${SKIA_DEBUG:-}" ] && [ "$SKIA_DEBUG" != 0 ]; then SKIA_KEY+=-debug; fi
    SKIA_SHA=$(tr -d '\r' < "$REPO/tools/skia-binaries.sha256" | awk -v v="$SKIA_VERSION" -v k="$SKIA_KEY" '!found && $1 == v && $2 == k { print $3; found = 1 }')
    if [ -z "$SKIA_SHA" ]; then
        echo "build_app: no pinned Skia binaries for skia-bindings $SKIA_VERSION, key $SKIA_KEY: add them to tools/skia-binaries.sha256" >&2
        exit 1
    fi
    SKIA_FILE="$CARGO_HOME/skia-binaries/$SKIA_VERSION/skia-binaries-$SKIA_KEY.tar.gz"
    if ! echo "$SKIA_SHA  $SKIA_FILE" | sha256sum -c --status 2> /dev/null; then
        mkdir -p "$(dirname "$SKIA_FILE")"
        part=$(mktemp "$SKIA_FILE.XXXXXX")
        curl -fsSL --retry 3 -o "$part" "https://github.com/rust-skia/skia-binaries/releases/download/$SKIA_VERSION/skia-binaries-$SKIA_KEY.tar.gz"
        if ! echo "$SKIA_SHA  $part" | sha256sum -c --status; then
            rm -f "$part"
            echo "build_app: the Skia binaries for skia-bindings $SKIA_VERSION do not match tools/skia-binaries.sha256" >&2
            exit 1
        fi
        mv "$part" "$SKIA_FILE"
    fi
    export SKIA_BINARIES_URL="file://$SKIA_FILE"
fi
# The UI's pictures and fonts are read at run time from the UI bundle by the
# absolute path they have there; the build sees them at the same path. Also for
# the image's own app (software renderer): all the pages' pictures would not fit
# into the image, and it runs as the fallback next to the bundle.
# (PANELOS_UI_DIR set by the caller: a check build of a second work tree; its binary is not for the panel)
export PANELOS_UI_DIR="${PANELOS_UI_DIR:-/opt/panelos-ui/share/ui}"
mkdir -p "$PANELOS_UI_DIR"
rsync -a --delete "$WORK/src/ui/" "$PANELOS_UI_DIR/"
cargo build --locked --release --target aarch64-unknown-linux-gnu "${FEATURES[@]}"
if [ "$GPU" = 1 ]; then
    # skia-bindings' own key is the one worked out above (else the checked archive does not fit its features)
    used=$(ls -t "$CARGO_TARGET_DIR"/aarch64-unknown-linux-gnu/release/build/skia-bindings-*/output)
    used=${used%%$'\n'*}
    if ! grep -qxF "TRYING TO DOWNLOAD AND INSTALL SKIA BINARIES: $SKIA_VERSION/$SKIA_KEY" "$used" \
        || ! grep -qxF "DOWNLOAD AND INSTALL SUCCEEDED" "$used"; then
        echo "build_app: skia-bindings wanted other Skia binaries than $SKIA_KEY (see $used)" >&2
        exit 1
    fi
fi
ls -la "$CARGO_TARGET_DIR/aarch64-unknown-linux-gnu/release/nspanel-app"
