# SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
# Source from nrf52840/: Arm's GNU toolchain (for ring's C and assembly) on PATH.
# While developing a rind patch in another checkout, also set RIND_DIR to it
# (the bench files come from there) and point Cargo.toml's rind at it.
here=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)
arm_bin=$(ls -d "$here"/../.tools/arm-gnu-toolchain-*/bin | head -1)
case ":$PATH:" in
*":$arm_bin:"*) ;;
*) export PATH="$arm_bin:$HOME/.cargo/bin:$PATH" ;;
esac
export CC_thumbv7em_none_eabihf=arm-none-eabi-gcc AR_thumbv7em_none_eabihf=arm-none-eabi-ar
export RUSTUP_TOOLCHAIN=1.98.1
