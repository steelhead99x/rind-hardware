<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# patches

Two series live here, and they do not share a number sequence.

**Base series.** `0001` through `0005` in this directory are `git format-patch`
files against the pinned rind commit. `cargo xtask apply-patches` commits them
in order. A new required patch appends `0006` here only when it is part of
that series. An optional patch does not.

**Optional sets.** A set is a directory `patches/<name>/` whose own patches
are numbered from `0001`. Two sets may each have a `0001`. Apply one with
`cargo xtask apply-patches --set <name>`; `--reverse` takes that set back
out and leaves the base series. `poly1305-32` is the set on main. Its file
was briefly `0006` in the base sequence. That number collides with
`rp2040/armv6m-sha512-ghash`, which still carries top-level `0006` (rolled
SHA-512) and `0007` (GHASH by multiplies) on that branch. Those two belong
in `patches/armv6m-sha512-ghash/0001-...` and `0002-...` when the branch
lands, not as further base patches.

**Ring.** `ring-0.17.14-*.patch` is applied by `cargo xtask vendor-ring` to a
copy of ring, not to the rind submodule. Those names are outside both
sequences. Provenance records their sha256 with the rind patches.
