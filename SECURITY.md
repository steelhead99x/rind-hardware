<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# Security policy

## Reporting a vulnerability

Report a vulnerability in rind-hardware itself (the firmware, the patches
under `patches/`, the xtask, or the criterion shim) privately through this
repository's security advisory form:

https://github.com/steelhead99x/rind-hardware/security/advisories/new

Include enough detail to reproduce it. Don't open a public issue for an
unfixed vulnerability.

## Vulnerabilities in rind

Report a vulnerability in the `rind` submodule that the patches here don't
introduce to [rind](https://github.com/kixel-ai/rind), as its
[SECURITY.md](https://github.com/kixel-ai/rind/blob/main/SECURITY.md) says.

## Scope

A slow timing result, a failed known-answer check, or a board-specific
crash is not by itself a vulnerability; file it as an ordinary issue.
