<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# patches

Two series live here, and they do not share a number sequence.

**Base series.** `0001` through `0005` in this directory are `git format-patch`
files against the pinned rind commit. `cargo xtask apply-patches` applies them
in order. A new required patch appends `0006` here only when it is part of
that series. An optional patch does not.

**Optional sets.** A set is a directory `patches/<name>/` whose own patches
are numbered from `0001`. Two sets may each have a `0001`. Apply one with
`cargo xtask apply-patches --set <name>`; `--reverse` takes that set back
out and leaves the base series. `poly1305-32` is the set on main. Its file
was briefly `0006` in the base sequence. `armv6m-sha512-ghash` is the
ARMv6-M set (rolled SHA-512, GHASH by multiplies, SHA-256's short path, and
two no-std build fixes); its first two patches were top-level `0006` and
`0007` on that branch before sets existed.

**Ring.** `ring-0.17.14-*.patch` is applied by `cargo xtask vendor-ring` to a
copy of ring, not to the rind submodule. Those names are outside both
sequences. Provenance records their sha256 with the rind patches.
