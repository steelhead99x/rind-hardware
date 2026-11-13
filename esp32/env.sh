# SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
# Source from esp32/: puts the Xtensa toolchain, ldproxy, espflash, and
# ESP-IDF's GCC (which ring's C build needs) on PATH.
here=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)
export PATH="$here/.embuild/espressif/tools/xtensa-esp-elf/esp-14.2.0_20241119/xtensa-esp-elf/bin:$HOME/.cargo/bin:$here/../.tools:$PATH"
. "$here/../.tools/export-esp.sh"
# rust-toolchain.toml picks "esp" too, but an exported RUSTUP_TOOLCHAIN (as
# nrf52840/env.sh sets) would override it.
export RUSTUP_TOOLCHAIN=esp
