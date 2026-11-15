#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
# Build the ESP32-S3 sha2, hmac, and hkdf images at two rind commits.
# Does not flash. See esp32/pr69/README.md.

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
rind=$root/rind
esp=$root/esp32
out=$esp/images-pr69
status=$out/patch-status.txt
fork_url=https://github.com/steelhead99x/rind.git

# Fork main, then the PR head. Short names are the output directories.
commits=(
  "8dfb7942d231219eed477fb58f5e15fe8a821bd2"
  "ef58672f28c16f49b727d2e360b474ada1913b8a"
)

benches=("$@")
if [[ ${#benches[@]} -eq 0 ]]; then
  benches=(sha2 hmac hkdf)
fi

mkdir -p "$out" "$root/.tools"
: >"$status"

say() { printf '%s\n' "$*" | tee -a "$status"; }

if [[ ! -e $rind/.git ]]; then
  git -C "$root" submodule update --init -- rind
fi
if ! git -C "$rind" remote get-url fork >/dev/null 2>&1; then
  git -C "$rind" remote add fork "$fork_url"
fi
git -C "$rind" fetch --no-tags fork main perf/esp32s3-rolled-sha512

rind_orig=$(git -C "$rind" rev-parse HEAD)
restore_rind() {
  git -C "$rind" checkout --detach --quiet "$rind_orig" || true
  git -C "$rind" reset --hard --quiet "$rind_orig" || true
  git -C "$rind" clean -fdq || true
}
trap restore_rind EXIT

# Same unsigned identity cargo xtask apply-patches uses, so the fork commit
# does not depend on who runs the script or on commit.gpgsign.
git_am() {
  local patch=$1
  shift
  env -u GIT_AUTHOR_NAME -u GIT_AUTHOR_EMAIL -u GIT_AUTHOR_DATE \
    -u GIT_COMMITTER_NAME -u GIT_COMMITTER_EMAIL -u GIT_COMMITTER_DATE \
    -u GIT_CONFIG_COUNT -u GIT_CONFIG_PARAMETERS \
    GIT_EDITOR=true \
    git -C "$rind" \
      -c user.name=steelhead99x \
      -c user.email=steelhead99x@users.noreply.github.com \
      -c commit.gpgsign=false \
      -c tag.gpgsign=false \
      -c i18n.commitEncoding=utf-8 \
      -c am.signoff=false \
      -c format.signOff=false \
      -c am.messageid=false \
      am --no-gpg-sign --no-signoff --no-verify --whitespace=nowarn \
      --committer-date-is-author-date "$@" "$patch"
}

apply_series() {
  local sha=$1
  say "commit $sha"
  git -C "$rind" checkout --detach --quiet "$sha"
  git -C "$rind" reset --hard --quiet "$sha"
  git -C "$rind" clean -fdq
  local patch base plain three
  for patch in "$root"/patches/000[1-5]-*.patch; do
    base=$(basename "$patch")
    plain=$(mktemp)
    three=$(mktemp)
    if git -C "$rind" apply --check "$patch" >"$plain" 2>&1; then
      git_am "$patch"
      say "  $base clean $(git -C "$rind" rev-parse HEAD)"
    elif git -C "$rind" apply --3way --check "$patch" >"$three" 2>&1; then
      # Plain `git am` is what xtask runs. On these two commits 0003's
      # cpu.rs hunk does not match: upstream inserted MissingFeature::name
      # between the Display impl and the Error impl. The blob in the patch
      # is still in this history, and `git am -3` auto-merges it.
      git_am "$patch" -3
      say "  $base plain-apply-failed 3way-merged $(git -C "$rind" rev-parse HEAD)"
      say "  $base plain apply said:"
      grep -E 'error:|patch failed' "$plain" | sed 's/^/    /' | tee -a "$status" || true
    else
      say "  $base CONFLICT"
      cat "$plain" "$three" | tee -a "$status"
      rm -f "$plain" "$three"
      git -C "$rind" am --abort >/dev/null 2>&1 || true
      exit 1
    fi
    rm -f "$plain" "$three"
  done
  say "  fork_commit $(git -C "$rind" rev-parse HEAD)"
}

vendor_ring() {
  local dest=$root/.tools/ring-esp
  if [[ -f $dest/Cargo.toml && -f $dest/crypto/poly1305/poly1305.c ]]; then
    if grep -q POLY1305_STATE_ALIGN "$dest/crypto/poly1305/poly1305.c"; then
      say "ring 0.17.14 already patched in .tools/ring-esp"
      return
    fi
  fi
  local tmp
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/ring.crate" https://static.crates.io/crates/ring/ring-0.17.14.crate
  tar -xzf "$tmp/ring.crate" -C "$tmp"
  rm -rf "$dest"
  mv "$tmp/ring-0.17.14" "$dest"
  rm -rf "$tmp"
  # Same patch `cargo xtask vendor-ring` applies. The file has a note before
  # the diff; `patch` skips that preamble.
  patch -d "$dest" -p1 <"$root/patches/ring-0.17.14-xtensa-poly1305-align.patch"
  say "ring 0.17.14 patched into .tools/ring-esp"
}

# esp-idf-sys installs the ESP-IDF toolchain from its build script. ring's C
# files need xtensa-esp32s3-elf-gcc. Building this crate first, into the same
# target directory and .embuild tree the firmware uses, finishes that install
# before ring's build script runs.
build_esp_idf_sys() {
  local helper=$root/.tools/idf-first
  mkdir -p "$helper/src" "$helper/.cargo"
  cat >"$helper/Cargo.toml" <<'EOF'
[package]
name = "idf-first"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
esp-idf-sys = { version = "=0.38.1", default-features = false, features = ["std", "binstart", "native"] }
EOF
  printf '// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0\n' >"$helper/src/lib.rs"
  cat >"$helper/.cargo/config.toml" <<EOF
# SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
[build]
target = "xtensa-esp32s3-espidf"

[target.xtensa-esp32s3-espidf]
linker = "ldproxy"
rustflags = ["--cfg", "espidf_time64"]

[unstable]
build-std = ["std", "panic_abort"]

[env]
MCU = "esp32s3"
ESP_IDF_VERSION = "v5.5.1"
ESP_IDF_SDKCONFIG_DEFAULTS = "sdkconfig.defaults"
ESP_IDF_TOOLS_INSTALL_DIR = "workspace"
CC_xtensa_esp32s3_espidf = "xtensa-esp32s3-elf-gcc"
AR_xtensa_esp32s3_espidf = "xtensa-esp32s3-elf-ar"
CARGO_WORKSPACE_DIR = { value = "$esp", force = true }
EOF
  say "building esp-idf-sys before ring"
  (
    cd "$helper"
    export CARGO_WORKSPACE_DIR=$esp
    export CARGO_TARGET_DIR=$esp/target
    unset IDF_PATH || true
    cargo build --release
  )
}

build_images() {
  local sha=$1
  local short=${sha:0:8}
  local dest=$out/$short
  mkdir -p "$dest"
  local bench elf
  for bench in "${benches[@]}"; do
    say "build $short $bench"
    (
      cd "$esp"
      export RIND_BENCH_PAD=0
      cargo build --release --features "$bench"
    )
    elf=$esp/target/xtensa-esp32s3-espidf/release/rind-bench-esp32
    cp "$elf" "$dest/$bench.elf"
    say "  file $(stat -c '%s' "$dest/$bench.elf") bytes  sha256 $(sha256sum "$dest/$bench.elf" | awk '{print $1}')"
    xtensa-esp32s3-elf-size "$dest/$bench.elf" | tee -a "$status"
  done
  say "  rind_HEAD $(git -C "$rind" rev-parse HEAD)"
}

say "pr69 ESP32-S3 images; this script does not flash"
vendor_ring

# shellcheck disable=SC1091
set +u
# shellcheck disable=SC1091
. "$esp/env.sh"
set -u
if ! command -v xtensa-esp32s3-elf-gcc >/dev/null; then
  echo "xtensa-esp32s3-elf-gcc is not on PATH. Install espup 0.19.0 and source .tools/export-esp.sh (see esp32/pr69/README.md)." >&2
  exit 1
fi
if ! command -v ldproxy >/dev/null; then
  echo "ldproxy is not on PATH. cargo install ldproxy --version 0.3.5 --locked" >&2
  exit 1
fi
rustc +esp --version | tee -a "$status"

build_esp_idf_sys

for sha in "${commits[@]}"; do
  apply_series "$sha"
  build_images "$sha"
done

say "done. images under $out. nothing was flashed."
