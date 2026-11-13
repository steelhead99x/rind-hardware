# SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
# Source from efr32mg24/: Arm's GNU toolchain (for ring's C and assembly) on PATH.
here=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)
arm_bin=$(ls -d "$here"/../.tools/arm-gnu-toolchain-*/bin | head -1)
case ":$PATH:" in
*":$arm_bin:"*) ;;
*) export PATH="$arm_bin:$HOME/.cargo/bin:$PATH" ;;
esac
export CC_thumbv8m_main_none_eabihf=arm-none-eabi-gcc AR_thumbv8m_main_none_eabihf=arm-none-eabi-ar
export RUSTUP_TOOLCHAIN=1.98.1
