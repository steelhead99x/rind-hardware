<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# Security policy

## Reporting a vulnerability in this repository

If you find a vulnerability in rind-hardware itself — the firmware, the
patches under `patches/`, the tooling, or the criterion shim — please report
it privately through GitHub's Security advisory form on this repository:

https://github.com/steelhead99x/rind-hardware/security/advisories/new

Please include enough detail to reproduce the issue and to tell firmware
issues apart from problems in an upstream library. Do not open a public
issue for an unfixed vulnerability.

## Vulnerabilities in the rind library

Bugs and vulnerabilities in rind itself, rather than in this firmware port,
should be reported to upstream rind:

https://github.com/kixel-ai/rind

Follow that repository's security policy if it has one. The `rind` submodule
and the patches against it are rind's code.

## Scope

This repository packages rind's benchmark suite as microcontroller firmware.
A timing result, a failed known-answer check, or a board-specific crash is
not automatically a vulnerability; those can be filed as ordinary issues.
