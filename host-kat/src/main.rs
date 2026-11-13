// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Known-answer checks for a Linux host, run before timing it: the boards'
//! checks (rind, ring, RustCrypto + dalek, libcrux) from the shared
//! `esp32/src/vectors.rs`, then the host-only libraries in `host.rs`. Exits
//! non-zero if any library fails any check.

#[path = "../../esp32/src/vectors.rs"]
mod vectors;

mod host;

fn main() -> std::process::ExitCode {
    let mut failed = vectors::check_all();
    failed.extend(host::check_all());
    let n = failed.len();
    println!("{{\"type\":\"host-kat-done\",\"failed\":{n}}}");
    if n == 0 && criterion::excluded() == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
