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
fi
# The UI's pictures and fonts are read at run time from the UI bundle by the
# absolute path they have there; the build sees them at the same path. Also for
# the image's own app (software renderer): all the pages' pictures would not fit
# into the image, and it runs as the fallback next to the bundle.
# (PANELOS_UI_DIR set by the caller: a check build of a second work tree; its binary is not for the panel)
export PANELOS_UI_DIR="${PANELOS_UI_DIR:-/opt/panelos-ui/share/ui}"
mkdir -p "$PANELOS_UI_DIR"
rsync -a --delete "$WORK/src/ui/" "$PANELOS_UI_DIR/"
cargo build --release --target aarch64-unknown-linux-gnu "${FEATURES[@]}"
# Keep the resolved dependency versions in the repository.
cp Cargo.lock "$REPO/app/Cargo.lock"
ls -la "$CARGO_TARGET_DIR/aarch64-unknown-linux-gnu/release/nspanel-app"
