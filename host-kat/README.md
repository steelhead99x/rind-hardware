<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# host-kat

The boards' known-answer checks (`esp32/src/vectors.rs`, included by path,
unchanged) compiled for a Linux host through a two-function criterion shim,
plus the host-only libraries (aws-lc-rs, graviola, OpenSSL) checked against
the same vectors and rind's outputs. Run this before timing a host.

It covers AES-128-GCM (NIST GCM TC2, 1350 B), ChaCha20-Poly1305 (RFC 8439
§2.8.2, 1350 B), SHA-256 and SHA-512 (FIPS 180-2, 16 KB), HMAC-SHA256
(RFC 4231 TC2) and X25519 (RFC 7748 §6.1), together with the board vector
set. 91 checks passed on x86_64 and on the Jetson Orin Nano (2026-11-12).

```sh
# from the repository root, with the rind submodule initialised
git submodule update --init rind
cd host-kat
cargo run --release
```

A successful run prints `{"type":"host-kat-done","failed":0}` and exits 0.
A non-zero exit means a library failed a check; do not time that host until
it is clean. The crate is `rind-bench-host-kat` and is not published.

The Jetson Orin Nano host results that followed a clean run are in
[rind-results](https://github.com/steelhead99x/rind-results/tree/main/results/jetson-orin-nano).
