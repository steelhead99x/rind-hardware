<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# armv6m-sha512-ghash: ARMv6-M (RP2040) fixes and speedups for rind's portable backend

An optional patch set. It applies on top of base patches 0001-0005 at the pinned
rind commit: `cargo xtask apply-patches --set armv6m-sha512-ghash`. Every patch
changes Arm targets without CAS atomics (ARMv6-M, `thumbv6m-none-eabi`) only,
except 0003 and 0005, which change bare-metal (`target_os = "none"`) builds.
0001 and 0002 were top-level `0006` and `0007` on this branch before optional
sets existed (see [../README.md](../README.md)); their content is unchanged.

| patch | what | Pico, measured |
|---|---|---|
| 0001 | rolled SHA-512 (HACL*'s unrolled one is about 27 KB, larger than the 16 KB XIP cache) | sha512/16KB 0.095x |
| 0002 | GHASH's 32x32->64 carry-less products by `muls` alone, no `__aeabi_lmul` | AES-128-GCM/16KB 0.479x |
| 0003 | debug and clippy builds without std: `Entry`'s debug counter is a `std::thread_local!` | builds |
| 0004 | SHA-256 below 2 KB by 8-round loops, linked with its finish in `.text.rind_sha256_hot` | sha256/32B 0.737x, hmac-sha256/32B 0.876x, sha256/16KB 1.008x |
| 0005 | VOPRF without std: import `Vec` from `alloc` | builds |

The ratios are rind's median cycles over 2 to 3 interleaved rounds on two Picos
(RP2040, 125 MHz, SysTick), against rind's `no-std/thumbv6m-alloc` branch, on
the patches' source branch `rp2040/armv6m-no-std` of the rind fork. 0005 is
needed at the current pin with or without this set: the base series alone does
not build for `thumbv6m-none-eabi` there, because the OPRF landed after
`0003`.
