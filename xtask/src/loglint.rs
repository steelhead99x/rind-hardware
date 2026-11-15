// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//! Checks a captured bench log against the records the firmware prints.
//!
//! A line is a record when it contains `RB:` (a serial log, boot noise and
//! all) or when it is a JSON object (the JSONL rind-results stores). Each
//! `start` opens a run. The numbers have to agree with that run's `cpu_hz`:
//! nanoseconds with the cycle counts, an ESP32-S3 `clock` record with the
//! wall clock, and an RP2040 `check_cycles`/`check_us` pair with each other
//! so a dropped 24-bit SysTick wrap cannot pass. A library that failed a
//! known-answer check must not then have a result for that operation. The
//! prefixes are the ones `esp32/src/vectors.rs` passes to `exclude`.

use serde_json::Value;
use std::collections::BTreeMap;

/// One problem in a capture. `line` is 1-based; 0 means the whole file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub line: usize,
    pub message: String,
}

/// One boot, from its `start` record through the record before the next.
#[derive(Clone, Debug)]
pub struct RunSummary {
    pub line: usize,
    pub chip: String,
    pub cpu_hz: u64,
    pub results: usize,
    pub checks: usize,
    pub checks_failed: usize,
}

/// Everything `lint` found. An empty capture is not ok: it has no runs and
/// one issue.
#[derive(Clone, Debug)]
pub struct Report {
    pub runs: Vec<RunSummary>,
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.issues.is_empty() && !self.runs.is_empty()
    }
}

struct Rec {
    line: usize,
    value: Value,
}

/// The id prefixes a failed check drops, in the order `vectors.rs` lists
/// them. `exclude` matches with `starts_with`, so `aes128-gcm` also covers
/// `aes128-gcm-open`.
fn exclusion_prefixes(check: &str) -> Option<&'static [&'static str]> {
    const TABLE: &[(&str, &[&str])] = &[
        (
            "aes128-gcm/",
            &["aes128-gcm", "quic-header-protection/aes128"],
        ),
        (
            "chacha20poly1305/",
            &["chacha20poly1305", "quic-header-protection/chacha20"],
        ),
        ("sha256/", &["sha256/"]),
        ("sha512/", &["sha512/"]),
        ("sha3-256/", &["sha3-256/"]),
        ("sha3-384/", &["sha3-384/"]),
        ("sha3-512/", &["sha3-512/"]),
        ("hmac-sha256/", &["hmac-sha256/", "hkdf-sha256/"]),
        ("x25519/", &["x25519-ecdh/"]),
        ("ed25519/", &["ed25519-"]),
        ("p256-ecdh/", &["p256-ecdh"]),
        ("mlkem768/", &["mlkem768-"]),
    ];
    TABLE
        .iter()
        .find(|(name, _)| check.starts_with(name))
        .map(|(_, prefixes)| *prefixes)
}

fn id_has_library(id: &str, library: &str) -> bool {
    id.split('/').any(|segment| segment == library)
}

fn excluded(id: &str, library: &str, check: &str) -> bool {
    let hit = |prefix: &str| id.starts_with(prefix) && id_has_library(id, library);
    match exclusion_prefixes(check) {
        Some(prefixes) => prefixes.iter().copied().any(hit),
        None => hit(check.split('/').next().unwrap_or(check)),
    }
}

/// Checks `text`. Boot lines that are not records are ignored.
pub fn lint(text: &str) -> Report {
    let mut issues = Vec::new();
    let mut records = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        match record_json(line) {
            None => {}
            Some(json) => match serde_json::from_str::<Value>(json) {
                Ok(value) if value.is_object() => records.push(Rec {
                    line: line_no,
                    value,
                }),
                Ok(_) => issues.push(Issue {
                    line: line_no,
                    message: "RB record is not a JSON object".into(),
                }),
                Err(e) => issues.push(Issue {
                    line: line_no,
                    message: format!("RB record is not JSON: {e}"),
                }),
            },
        }
    }

    let mut runs: Vec<Vec<Rec>> = Vec::new();
    let mut current: Option<Vec<Rec>> = None;
    for rec in records {
        if ty(&rec.value) == Some("start") {
            if let Some(done) = current.take() {
                runs.push(done);
            }
            current = Some(vec![rec]);
        } else if let Some(run) = current.as_mut() {
            run.push(rec);
        } else {
            issues.push(Issue {
                line: rec.line,
                message: format!(
                    "record before start ({})",
                    ty(&rec.value).unwrap_or("no type")
                ),
            });
        }
    }
    if let Some(run) = current {
        runs.push(run);
    }
    if runs.is_empty() && issues.is_empty() {
        issues.push(Issue {
            line: 0,
            message: "no RB records".into(),
        });
    }

    let mut summaries = Vec::new();
    for run in &runs {
        if let Some(summary) = lint_run(run, &mut issues) {
            summaries.push(summary);
        }
    }
    Report {
        runs: summaries,
        issues,
    }
}

fn record_json(line: &str) -> Option<&str> {
    if let Some(at) = line.find("RB:") {
        let json = line[at + 3..].trim();
        return Some(json);
    }
    let trimmed = line.trim();
    trimmed.starts_with('{').then_some(trimmed)
}

fn ty(v: &Value) -> Option<&str> {
    v.get("type").and_then(Value::as_str)
}

fn lint_run(run: &[Rec], issues: &mut Vec<Issue>) -> Option<RunSummary> {
    let start = &run[0];
    let line = start.line;
    let chip = match string_field(&start.value, "chip") {
        Ok(s) if !s.is_empty() => s.to_string(),
        _ => {
            issues.push(Issue {
                line,
                message: "start record needs a chip".into(),
            });
            return None;
        }
    };
    let cpu_hz = match u64_field(&start.value, "cpu_hz") {
        Ok(hz) if hz > 0 => hz,
        _ => {
            issues.push(Issue {
                line,
                message: "start record needs a positive cpu_hz".into(),
            });
            return None;
        }
    };
    match string_field(&start.value, "profile") {
        Ok("release" | "debug") => {}
        _ => issues.push(Issue {
            line,
            message: "start record profile must be release or debug".into(),
        }),
    }

    if chip == "rp2040" {
        lint_systick(start, cpu_hz, issues);
    }

    let mut checks_done_at: Option<usize> = None;
    let mut done_at: Option<usize> = None;
    let mut clock_at: Option<usize> = None;
    let mut first_result: Option<usize> = None;
    let mut checks = Vec::new();
    let mut result_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut n_results = 0usize;

    for rec in run.iter().skip(1) {
        let Some(kind) = ty(&rec.value) else {
            issues.push(Issue {
                line: rec.line,
                message: "record has no string type".into(),
            });
            continue;
        };
        match kind {
            "start" => issues.push(Issue {
                line: rec.line,
                message: "start record inside a run".into(),
            }),
            "check" => {
                if checks_done_at.is_some() {
                    issues.push(Issue {
                        line: rec.line,
                        message: "check record after checks-done".into(),
                    });
                }
                match (
                    string_field(&rec.value, "check"),
                    string_field(&rec.value, "library"),
                    bool_field(&rec.value, "pass"),
                ) {
                    (Ok(check), Ok(library), Ok(pass)) => {
                        checks.push((rec.line, check.to_string(), library.to_string(), pass));
                    }
                    _ => issues.push(Issue {
                        line: rec.line,
                        message: "check record needs check, library and pass".into(),
                    }),
                }
            }
            "checks-done" => {
                if checks_done_at.is_some() {
                    issues.push(Issue {
                        line: rec.line,
                        message: "a run has two checks-done records".into(),
                    });
                }
                checks_done_at = Some(rec.line);
                let failed = u64_field(&rec.value, "failed");
                let n_fail = checks.iter().filter(|c| !c.3).count() as u64;
                match failed {
                    Ok(n) if n_fail == 0 && n == 0 => {}
                    Ok(n) if n_fail > 0 && n >= n_fail => {}
                    Ok(n) => issues.push(Issue {
                        line: rec.line,
                        message: format!(
                            "checks-done failed {n} does not match {n_fail} failing checks"
                        ),
                    }),
                    Err(_) => issues.push(Issue {
                        line: rec.line,
                        message: "checks-done needs a failed count".into(),
                    }),
                }
            }
            "clock" => {
                clock_at = Some(rec.line);
                lint_clock(rec, cpu_hz, issues);
            }
            "result" => {
                n_results += 1;
                first_result.get_or_insert(rec.line);
                lint_result(rec, cpu_hz, &mut result_ids, issues);
            }
            "skipped" => match (
                string_field(&rec.value, "id"),
                string_field(&rec.value, "reason"),
            ) {
                (Ok(id), Ok(_)) => {
                    if result_ids.contains_key(id) {
                        issues.push(Issue {
                            line: rec.line,
                            message: format!("{id} is both a result and skipped"),
                        });
                    }
                }
                _ => issues.push(Issue {
                    line: rec.line,
                    message: "skipped record needs an id and a reason".into(),
                }),
            },
            "bench" => {
                if string_field(&rec.value, "file").is_err() {
                    issues.push(Issue {
                        line: rec.line,
                        message: "bench record needs a file".into(),
                    });
                }
            }
            "done" => {
                if done_at.is_some() {
                    issues.push(Issue {
                        line: rec.line,
                        message: "a run has two done records".into(),
                    });
                }
                done_at = Some(rec.line);
            }
            _ => {}
        }
        if let Some(done_line) = done_at
            && rec.line > done_line
        {
            issues.push(Issue {
                line: rec.line,
                message: "record after done".into(),
            });
        }
    }

    let bench_run = n_results > 0
        || run.iter().any(|r| {
            matches!(
                ty(&r.value),
                Some("bench" | "checks-done" | "clock" | "result")
            )
        });
    if bench_run && checks_done_at.is_none() {
        issues.push(Issue {
            line,
            message: "bench run has no checks-done record".into(),
        });
    }
    if bench_run && done_at.is_none() {
        issues.push(Issue {
            line,
            message: "bench run has no done record".into(),
        });
    }
    if let Some(result_line) = first_result
        && checks_done_at.is_none_or(|done| result_line < done)
    {
        issues.push(Issue {
            line: result_line,
            message: "result before checks-done".into(),
        });
    }
    for rec in run.iter().skip(1) {
        if ty(&rec.value) != Some("result") {
            continue;
        }
        let Ok(id) = string_field(&rec.value, "id") else {
            continue;
        };
        for (check_line, check, library, pass) in &checks {
            if !*pass && excluded(id, library, check) {
                issues.push(Issue {
                    line: rec.line,
                    message: format!(
                        "{id} was raced after {library} failed {check} (line {check_line})"
                    ),
                });
            }
        }
    }

    if chip == "esp32s3" && n_results > 0 {
        match clock_at {
            None => issues.push(Issue {
                line,
                message: "esp32s3 run has no clock record".into(),
            }),
            Some(clock_line) => {
                if first_result.is_some_and(|result| clock_line > result) {
                    issues.push(Issue {
                        line: clock_line,
                        message: "clock record is after the first result".into(),
                    });
                }
            }
        }
    }

    let checks_failed = checks.iter().filter(|c| !c.3).count();
    Some(RunSummary {
        line,
        chip,
        cpu_hz,
        results: n_results,
        checks: checks.len(),
        checks_failed,
    })
}

fn lint_systick(start: &Rec, cpu_hz: u64, issues: &mut Vec<Issue>) {
    let cycles = u64_field(&start.value, "check_cycles");
    let us = u64_field(&start.value, "check_us");
    let (Ok(cycles), Ok(us)) = (cycles, us) else {
        issues.push(Issue {
            line: start.line,
            message: "rp2040 start record needs check_cycles and check_us".into(),
        });
        return;
    };
    if us == 0 {
        issues.push(Issue {
            line: start.line,
            message: "rp2040 check_us is 0".into(),
        });
        return;
    }
    let expect_us = cycles as f64 / cpu_hz as f64 * 1e6;
    let err_us = (expect_us - us as f64).abs();
    if err_us <= 1_000.0 {
        return;
    }
    let bits = u64_field(&start.value, "systick_bits").unwrap_or(24);
    let period_us = (1u64 << bits.min(31)) as f64 / cpu_hz as f64 * 1e6;
    let wraps = (err_us / period_us).round();
    let wrap_note = if wraps >= 1.0 && (err_us - wraps * period_us).abs() < 0.25 * period_us {
        format!("; that is {wraps:.0} dropped or doubled 24-bit SysTick wrap")
    } else {
        String::new()
    };
    issues.push(Issue {
        line: start.line,
        message: format!(
            "rp2040 SysTick cross-check is off by {err_us:.0} us (cycles {cycles} vs timer {us} us){wrap_note}"
        ),
    });
}

fn lint_clock(rec: &Rec, cpu_hz: u64, issues: &mut Vec<Issue>) {
    let (Ok(wall), Ok(cycles)) = (
        u64_field(&rec.value, "wall_ns"),
        u64_field(&rec.value, "cycles"),
    ) else {
        issues.push(Issue {
            line: rec.line,
            message: "clock record needs wall_ns and cycles".into(),
        });
        return;
    };
    if wall == 0 {
        issues.push(Issue {
            line: rec.line,
            message: "clock record wall_ns is 0".into(),
        });
        return;
    }
    let hz = cycles as f64 / wall as f64 * 1e9;
    let rel = (hz - cpu_hz as f64) / cpu_hz as f64;
    if rel.abs() > 0.02 {
        issues.push(Issue {
            line: rec.line,
            message: format!(
                "clock record is {hz:.0} Hz, cpu_hz is {cpu_hz} ({:.2}% off)",
                rel * 100.0
            ),
        });
    }
}

fn lint_result(
    rec: &Rec,
    cpu_hz: u64,
    seen: &mut BTreeMap<String, usize>,
    issues: &mut Vec<Issue>,
) {
    let line = rec.line;
    let Ok(id) = string_field(&rec.value, "id") else {
        issues.push(Issue {
            line,
            message: "result record needs an id".into(),
        });
        return;
    };
    if let Some(prev) = seen.insert(id.to_string(), line) {
        issues.push(Issue {
            line,
            message: format!("{id} is reported twice (also line {prev})"),
        });
    }

    let Some(cycles) = rec.value.get("cycles").filter(|v| v.is_object()) else {
        issues.push(Issue {
            line,
            message: format!("{id} has no cycles object"),
        });
        return;
    };
    let Some(min) = f64_field(cycles, "min") else {
        issues.push(Issue {
            line,
            message: format!("{id} cycles need min, p5, median, p95, max, mean and stddev"),
        });
        return;
    };
    let (Some(p5), Some(median), Some(p95), Some(max), Some(mean), Some(stddev)) = (
        f64_field(cycles, "p5"),
        f64_field(cycles, "median"),
        f64_field(cycles, "p95"),
        f64_field(cycles, "max"),
        f64_field(cycles, "mean"),
        f64_field(cycles, "stddev"),
    ) else {
        issues.push(Issue {
            line,
            message: format!("{id} cycles need min, p5, median, p95, max, mean and stddev"),
        });
        return;
    };
    for (name, n) in [
        ("min", min),
        ("p5", p5),
        ("median", median),
        ("p95", p95),
        ("max", max),
        ("mean", mean),
        ("stddev", stddev),
    ] {
        if !n.is_finite() {
            issues.push(Issue {
                line,
                message: format!("{id} cycles.{name} is not finite"),
            });
            return;
        }
    }
    // `{:.1}` can move a printed percentile by 0.05, so a true ordering can
    // look inverted by a tenth.
    let le = |a: f64, b: f64| a <= b + 0.11;
    if !(le(min, p5)
        && le(p5, median)
        && le(median, p95)
        && le(p95, max)
        && le(min, mean)
        && le(mean, max))
    {
        issues.push(Issue {
            line,
            message: format!(
                "{id} cycles out of order: min {min} p5 {p5} median {median} p95 {p95} max {max} mean {mean}"
            ),
        });
    }
    if stddev < 0.0 {
        issues.push(Issue {
            line,
            message: format!("{id} stddev is negative"),
        });
    }
    if median == 0.0 {
        issues.push(Issue {
            line,
            message: format!("{id} median is 0 cycles"),
        });
    }

    let samples = u64_field(&rec.value, "samples");
    let iters = u64_field(&rec.value, "iters_per_sample");
    match samples {
        Ok(n) if (11..=51).contains(&n) => {}
        Ok(n) => issues.push(Issue {
            line,
            message: format!("{id} has {n} samples; the firmware takes 11 to 51"),
        }),
        Err(_) => issues.push(Issue {
            line,
            message: format!("{id} needs a samples count"),
        }),
    }
    if !matches!(iters, Ok(n) if n >= 1) {
        issues.push(Issue {
            line,
            message: format!("{id} needs iters_per_sample >= 1"),
        });
    }

    match f64_field(&rec.value, "ns") {
        Some(ns) => check_ns(line, id, "ns", ns, median, cpu_hz, issues),
        None => issues.push(Issue {
            line,
            message: format!("{id} needs ns"),
        }),
    }
    if let Some(ns) = f64_field(&rec.value, "ns_low") {
        check_ns(line, id, "ns_low", ns, p5, cpu_hz, issues);
    } else {
        issues.push(Issue {
            line,
            message: format!("{id} needs ns_low"),
        });
    }
    if let Some(ns) = f64_field(&rec.value, "ns_high") {
        check_ns(line, id, "ns_high", ns, p95, cpu_hz, issues);
    } else {
        issues.push(Issue {
            line,
            message: format!("{id} needs ns_high"),
        });
    }

    match throughput_bytes(&rec.value) {
        Ok(Some(bytes)) if bytes > 0 => match f64_field(&rec.value, "cycles_per_byte") {
            Some(cpb) => {
                let expect = median / bytes as f64;
                let tol = 0.05 / bytes as f64 + 0.0006;
                if (cpb - expect).abs() > tol {
                    issues.push(Issue {
                        line,
                        message: format!(
                            "{id} cycles_per_byte {cpb} does not match median/bytes {expect:.3}"
                        ),
                    });
                }
            }
            None => issues.push(Issue {
                line,
                message: format!("{id} needs cycles_per_byte"),
            }),
        },
        Ok(_) => {
            if !rec.value.get("cycles_per_byte").is_some_and(Value::is_null) {
                issues.push(Issue {
                    line,
                    message: format!("{id} cycles_per_byte should be null"),
                });
            }
        }
        Err(msg) => issues.push(Issue {
            line,
            message: format!("{id} {msg}"),
        }),
    }

    if let (Some(wall), Some(counted)) = (
        rec.value.get("wall_ns").and_then(as_u64),
        rec.value.get("sampled_cycles").and_then(as_u64),
    ) {
        if wall == 0 || counted == 0 {
            issues.push(Issue {
                line,
                message: format!("{id} wall_ns and sampled_cycles must be positive"),
            });
        } else {
            let hz = counted as f64 / wall as f64 * 1e9;
            if hz > cpu_hz as f64 * 1.02 || hz < cpu_hz as f64 * 0.25 {
                issues.push(Issue {
                    line,
                    message: format!(
                        "{id} sampled {counted} cycles in {wall} ns ({hz:.0} Hz, cpu_hz {cpu_hz})"
                    ),
                });
            }
        }
    }
}

fn check_ns(
    line: usize,
    id: &str,
    field: &str,
    ns: f64,
    cycles: f64,
    cpu_hz: u64,
    issues: &mut Vec<Issue>,
) {
    let expect = cycles * 1e9 / cpu_hz as f64;
    // `ns` is formatted from the unrounded median and `cycles` to one decimal,
    // so the two can disagree by half a tenth of a cycle.
    let tol = 0.05 * 1e9 / cpu_hz as f64 + 0.001;
    if (ns - expect).abs() > tol {
        issues.push(Issue {
            line,
            message: format!(
                "{id} {field} {ns} does not match {cycles} cycles at {cpu_hz} Hz (expected {expect:.3})"
            ),
        });
    }
}

fn throughput_bytes(v: &Value) -> Result<Option<u64>, String> {
    let Some(tp) = v.get("throughput") else {
        return Err("needs throughput".into());
    };
    match tp {
        Value::Null => Ok(None),
        Value::Object(map) => {
            if let Some(bytes) = map.get("Bytes").or_else(|| map.get("BytesDecimal")) {
                as_u64(bytes)
                    .map(Some)
                    .ok_or_else(|| "throughput bytes are not an integer".into())
            } else if map.contains_key("Elements") {
                Ok(None)
            } else {
                Err("throughput is not Bytes, BytesDecimal or Elements".into())
            }
        }
        _ => Err("throughput is not an object or null".into()),
    }
}

fn string_field<'a>(v: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    v.get(key).and_then(Value::as_str).ok_or("missing")
}

fn bool_field(v: &Value, key: &str) -> Result<bool, &'static str> {
    v.get(key).and_then(Value::as_bool).ok_or("missing")
}

fn u64_field(v: &Value, key: &str) -> Result<u64, &'static str> {
    v.get(key).and_then(as_u64).ok_or("missing")
}

fn f64_field(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn as_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        let n = v.as_f64()?;
        (n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= u64::MAX as f64).then_some(n as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(report: &Report) -> Vec<&str> {
        report.issues.iter().map(|i| i.message.as_str()).collect()
    }

    fn assert_ok(text: &str) -> Report {
        let report = lint(text);
        assert!(report.ok(), "issues: {:?}", messages(&report));
        report
    }

    /// The published x25519 capture's shape: a timestamp in front of `RB:`,
    /// and a harness row whose printed median (17.0) is the `{:.1}` rounding
    /// of the cycles that produced `ns` 70.841.
    const ESP_LOG: &str = r#"
     0.000 SPIWP:0xee
     0.223 RB:{"type":"start","chip":"esp32s3","cpu_hz":240000000,"core":1,"profile":"release"}
     0.254 RB:{"type":"check","check":"x25519/rfc7748-6.1","library":"dalek","pass":true}
     1.407 RB:{"type":"checks-done","failed":0}
     2.408 RB:{"type":"clock","wall_ns":1000001000,"cycles":240000826,"spins":1066193}
     2.735 RB:{"type":"result","id":"harness/empty/iter","ns":70.841,"ns_low":70.841,"ns_high":70.987,"throughput":null,"cycles":{"median":17.0,"min":17.0,"p5":17.0,"p95":17.0,"max":17.0,"mean":17.0,"stddev":0.0},"cycles_per_byte":null,"samples":51,"iters_per_sample":28222,"wall_ns":102201000,"sampled_cycles":24482366}
     3.076 RB:{"type":"bench","file":"x25519"}
     9.244 RB:{"type":"result","id":"x25519-ecdh/dalek","ns":28271404.167,"ns_low":28189204.167,"ns_high":28324329.167,"throughput":{"Elements":1},"cycles":{"median":6785137.0,"min":6764008.0,"p5":6765409.0,"p95":6797839.0,"max":6799609.0,"mean":6782169.2,"stddev":11430.5},"cycles_per_byte":null,"samples":51,"iters_per_sample":1,"wall_ns":1441801000,"sampled_cycles":345890628}
    10.944 RB:{"type":"result","id":"x25519-keygen/dalek","ns":7089225.000,"ns_low":7083675.000,"ns_high":7102962.500,"throughput":{"Elements":1},"cycles":{"median":1701414.0,"min":1699497.0,"p5":1700082.0,"p95":1704711.0,"max":1728231.0,"mean":1702314.0,"stddev":3942.7},"cycles_per_byte":null,"samples":51,"iters_per_sample":1,"wall_ns":362299000,"sampled_cycles":86818014}
    40.905 RB:{"type":"done"}
"#;

    #[test]
    fn a_serial_log_with_boot_noise_passes() {
        let report = assert_ok(ESP_LOG);
        assert_eq!(report.runs.len(), 1);
        assert_eq!(report.runs[0].chip, "esp32s3");
        assert_eq!(report.runs[0].results, 3);
        assert_eq!(report.runs[0].checks_failed, 0);
    }

    #[test]
    fn jsonl_without_the_rb_prefix_passes() {
        let jsonl = ESP_LOG
            .lines()
            .filter_map(|l| l.split("RB:").nth(1))
            .map(|j| {
                // rind-results pretty-prints with a space after ':' and ','.
                j.replace("\":\"", "\": \"")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_ok(&jsonl);
    }

    #[test]
    fn records_before_start_and_an_empty_file_fail() {
        let early = lint(
            "RB:{\"type\":\"check\",\"check\":\"sha256/fips180-abc\",\"library\":\"rind\",\"pass\":true}\n",
        );
        assert!(messages(&early).iter().any(|m| m.contains("before start")));
        let empty = lint("boot log only\n");
        assert!(messages(&empty).iter().any(|m| m.contains("no RB records")));
        let bad = lint("RB:{not json}\n");
        assert!(messages(&bad).iter().any(|m| m.contains("not JSON")));
    }

    #[test]
    fn a_failed_check_must_not_be_raced_including_the_extra_prefixes() {
        let mut raced = start_esp();
        raced.push_str(
            "RB:{\"type\":\"check\",\"check\":\"aes128-gcm/nist-gcm-tc2\",\"library\":\"ring\",\"pass\":false}\n",
        );
        raced.push_str(
            "RB:{\"type\":\"check\",\"check\":\"hmac-sha256/rfc4231-tc2\",\"library\":\"rind\",\"pass\":false}\n",
        );
        raced.push_str(
            "RB:{\"type\":\"check\",\"check\":\"x25519/rfc7748-6.1\",\"library\":\"dalek\",\"pass\":false}\n",
        );
        raced.push_str("RB:{\"type\":\"checks-done\",\"failed\":5}\n");
        raced.push_str("RB:{\"type\":\"clock\",\"wall_ns\":1000000000,\"cycles\":240000000}\n");
        raced.push_str(&result("aes128-gcm-open/ring/16KB", 16384));
        raced.push_str(&result("quic-header-protection/aes128/ring", 0));
        raced.push_str(&result("aes128-gcm/rind/16KB", 16384));
        raced.push_str(&result("hkdf-sha256/expand/rind", 0));
        raced.push_str(&result("hmac-sha256/rind/64B", 64));
        raced.push_str(&result("x25519-ecdh/dalek", 0));
        raced.push_str(&result("x25519-keygen/dalek", 0));
        raced.push_str("RB:{\"type\":\"done\"}\n");
        let report = lint(&raced);
        let text = messages(&report).join("\n");
        assert!(text.contains("aes128-gcm-open/ring/16KB"), "{text}");
        assert!(
            text.contains("quic-header-protection/aes128/ring"),
            "{text}"
        );
        assert!(text.contains("hkdf-sha256/expand/rind"), "{text}");
        assert!(text.contains("hmac-sha256/rind/64B"), "{text}");
        assert!(text.contains("x25519-ecdh/dalek"), "{text}");
        assert!(
            !text.contains("aes128-gcm/rind"),
            "rind passed aes and must still be raced: {text}"
        );
        assert!(
            !text.contains("x25519-keygen/dalek"),
            "keygen is not the excluded prefix: {text}"
        );
    }

    #[test]
    fn statistics_and_the_clock_have_to_match_cpu_hz() {
        let mut bad = String::from(&start_esp());
        bad.push_str(r#"RB:{"type":"checks-done","failed":0}"#);
        bad.push('\n');
        bad.push_str(r#"RB:{"type":"clock","wall_ns":1000000000,"cycles":250000000}"#);
        bad.push('\n');
        bad.push_str(&result_body(
            "sha256/rind/32B",
            32,
            100.0, // ns does not match 10 cycles at 240 MHz
            10.0,
            10.0,
            9.0, // p5 above median, and min above p5
            10.0,
            12.0,
            10.0,
            0.0,
            51,
            9.0, // cycles per byte should be 10/32
        ));
        bad.push_str(r#"RB:{"type":"done"}"#);
        bad.push('\n');
        let text = messages(&lint(&bad)).join("\n");
        assert!(text.contains("clock record"), "{text}");
        assert!(text.contains("out of order"), "{text}");
        assert!(text.contains("does not match"), "{text}");
        assert!(text.contains("cycles_per_byte"), "{text}");
    }

    #[test]
    fn sample_count_below_eleven_fails() {
        let mut text = start_esp();
        text.push_str(r#"RB:{"type":"checks-done","failed":0}"#);
        text.push('\n');
        text.push_str(r#"RB:{"type":"clock","wall_ns":1000000000,"cycles":240000000}"#);
        text.push('\n');
        text.push_str(&result_body(
            "sha256/rind/32B",
            32,
            41.667,
            10.0,
            10.0,
            10.0,
            10.0,
            10.0,
            10.0,
            0.0,
            10,
            0.313,
        ));
        text.push_str(r#"RB:{"type":"done"}"#);
        let msgs = messages(&lint(&text)).join("\n");
        assert!(msgs.contains("10 samples"), "{msgs}");
    }

    #[test]
    fn rp2040_systick_wrap_is_rejected_and_agreement_passes() {
        let good = r#"RB:{"type":"start","chip":"rp2040","cpu_hz":125000000,"profile":"release","check_cycles":37500000,"check_us":300000,"systick_bits":24}
RB:{"type":"checks-done","failed":0}
RB:{"type":"bench","file":"sha2"}
RB:{"type":"result","id":"sha256/rind/32B","ns":80.000,"ns_low":80.000,"ns_high":80.000,"throughput":{"Bytes":32},"cycles":{"median":10.0,"min":10.0,"p5":10.0,"p95":10.0,"max":10.0,"mean":10.0,"stddev":0.0},"cycles_per_byte":0.313,"samples":11,"iters_per_sample":1}
RB:{"type":"done"}
"#;
        assert_ok(good);

        let lost = good.replace("37500000", "20722784");
        let msgs = messages(&lint(&lost)).join("\n");
        assert!(msgs.contains("SysTick"), "{msgs}");
        assert!(msgs.contains("wrap"), "{msgs}");
    }

    #[test]
    fn two_runs_and_a_watchdog_probe_are_separate() {
        let twice = format!("{ESP_LOG}\nboot\n{ESP_LOG}");
        let report = assert_ok(&twice);
        assert_eq!(report.runs.len(), 2);

        let probe = r#"RB:{"type":"start","chip":"nrf52840","cpu_hz":64000000,"profile":"release"}
RB:{"type":"wdt","event":"probe","runstatus":1}
RB:{"type":"wdt","event":"feed"}
"#;
        let report = assert_ok(probe);
        assert_eq!(report.runs[0].results, 0);
    }

    #[test]
    fn a_bench_run_needs_checks_done_and_done_last() {
        let text = r#"RB:{"type":"start","chip":"nrf52840","cpu_hz":64000000,"profile":"release"}
RB:{"type":"result","id":"sha256/rind/32B","ns":156.250,"ns_low":156.250,"ns_high":156.250,"throughput":{"Bytes":32},"cycles":{"median":10.0,"min":10.0,"p5":10.0,"p95":10.0,"max":10.0,"mean":10.0,"stddev":0.0},"cycles_per_byte":0.313,"samples":11,"iters_per_sample":1}
RB:{"type":"done"}
RB:{"type":"oled","ok":true}
"#;
        let msgs = messages(&lint(text)).join("\n");
        assert!(msgs.contains("checks-done"), "{msgs}");
        assert!(msgs.contains("before checks-done"), "{msgs}");
        assert!(msgs.contains("after done"), "{msgs}");
    }

    #[test]
    fn checks_done_failed_count_allows_one_check_to_add_two_prefixes() {
        let one = r#"RB:{"type":"start","chip":"nrf52840","cpu_hz":64000000,"profile":"release"}
RB:{"type":"check","check":"aes128-gcm/nist-gcm-tc2","library":"ring","pass":false}
RB:{"type":"checks-done","failed":2}
RB:{"type":"done"}
"#;
        assert_ok(one);
        let zero = one.replace("\"failed\":2", "\"failed\":0");
        assert!(messages(&lint(&zero)).join("\n").contains("does not match"));
    }

    fn start_esp() -> String {
        "RB:{\"type\":\"start\",\"chip\":\"esp32s3\",\"cpu_hz\":240000000,\"profile\":\"release\"}\n"
            .into()
    }

    /// A result whose ns matches `median` cycles at 240 MHz when `ns` is passed
    /// through, or the caller's ns when they want a mismatch. `bytes` 0 means
    /// an Elements throughput.
    fn result(id: &str, bytes: u64) -> String {
        let median = 240.0;
        let ns = median * 1e9 / 240_000_000.0;
        result_body(
            id,
            bytes,
            ns,
            median,
            median,
            median,
            median,
            median,
            median,
            0.0,
            11,
            if bytes == 0 {
                f64::NAN
            } else {
                median / bytes as f64
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn result_body(
        id: &str,
        bytes: u64,
        ns: f64,
        median: f64,
        min: f64,
        p5: f64,
        p95: f64,
        max: f64,
        mean: f64,
        stddev: f64,
        samples: u64,
        cpb: f64,
    ) -> String {
        let (tp, cpb) = if bytes == 0 {
            ("{\"Elements\":1}".to_string(), "null".to_string())
        } else {
            (format!("{{\"Bytes\":{bytes}}}"), format!("{cpb:.3}"))
        };
        format!(
            concat!(
                "RB:{{\"type\":\"result\",\"id\":\"{id}\",\"ns\":{ns:.3},\"ns_low\":{ns:.3},\"ns_high\":{ns:.3},",
                "\"throughput\":{tp},\"cycles\":{{\"median\":{median},\"min\":{min},\"p5\":{p5},",
                "\"p95\":{p95},\"max\":{max},\"mean\":{mean},\"stddev\":{stddev}}},",
                "\"cycles_per_byte\":{cpb},\"samples\":{samples},\"iters_per_sample\":1,",
                "\"wall_ns\":1000000,\"sampled_cycles\":240000}}\n"
            ),
            id = id,
            ns = ns,
            tp = tp,
            median = median,
            min = min,
            p5 = p5,
            p95 = p95,
            max = max,
            mean = mean,
            stddev = stddev,
            cpb = cpb,
            samples = samples,
        )
    }
}
