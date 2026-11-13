# SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
# Source from rp2040/: Arm's GNU toolchain on PATH (objcopy for the UF2).
# While developing a rind patch in another checkout, also set RIND_DIR to it
# (the bench files come from there) and point Cargo.toml's rind at it.
here=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)
arm_bin=$(ls -d "$here"/../.tools/arm-gnu-toolchain-*/bin | head -1)
case ":$PATH:" in
*":$arm_bin:"*) ;;
*) export PATH="$arm_bin:$HOME/.cargo/bin:$PATH" ;;
esac
export CC_thumbv6m_none_eabi=arm-none-eabi-gcc AR_thumbv6m_none_eabi=arm-none-eabi-ar
export RUSTUP_TOOLCHAIN=1.98.1
