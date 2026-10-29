mod crc32c;
mod format;
mod json;
mod recover;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use json::Value;
use recover::Failure;

struct Args {
    segments: PathBuf,
    out_state: PathBuf,
    out_report: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut segments = None;
    let mut out_state = None;
    let mut out_report = None;

    let mut it = std::env::args_os().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("missing value for {flag:?}"))?;
        match flag.to_str() {
            Some("--segments") if segments.is_none() => segments = Some(PathBuf::from(value)),
            Some("--out-state") if out_state.is_none() => out_state = Some(PathBuf::from(value)),
            Some("--out-report") if out_report.is_none() => out_report = Some(PathBuf::from(value)),
            Some("--segments" | "--out-state" | "--out-report") => {
                return Err(format!("duplicate flag {flag:?}"));
            }
            _ => return Err(format!("unknown flag {flag:?}")),
        }
    }

    Ok(Args {
        segments: segments.ok_or("missing --segments")?,
        out_state: out_state.ok_or("missing --out-state")?,
        out_report: out_report.ok_or("missing --out-report")?,
    })
}

/// Writes `contents` to `path` via a same-directory temp file plus atomic
/// rename, so a crash mid-write never leaves a half-written file at `path`.
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp_path = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("out"),
        std::process::id()
    ));
    fs::write(&tmp_path, contents)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("usage error: {e}");
            return ExitCode::from(1);
        }
    };

    match recover::recover(&args.segments) {
        Ok(out) => {
            let state = Value::Obj(vec![
                (
                    "entries",
                    Value::Arr(
                        out.entries
                            .iter()
                            .map(|e| {
                                Value::Obj(vec![
                                    ("key", Value::Str(e.key.clone())),
                                    ("value_b64", Value::Str(json::base64_encode(&e.value))),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("count", Value::Uint(out.entries.len() as u64)),
            ]);

            let report = Value::Obj(vec![
                (
                    "accepted_segments",
                    Value::Arr(out.accepted_segments.iter().map(|s| Value::Str(s.clone())).collect()),
                ),
                (
                    "rejected_segments",
                    Value::Arr(
                        out.rejected_segments
                            .iter()
                            .map(|r| {
                                Value::Obj(vec![
                                    ("file", Value::Str(r.file.clone())),
                                    ("reason", Value::Str(r.reason.to_string())),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "ignored_temp_files",
                    Value::Arr(out.ignored_temp_files.iter().map(|s| Value::Str(s.clone())).collect()),
                ),
                (
                    "compactions",
                    Value::Arr(
                        out.compactions
                            .iter()
                            .map(|c| {
                                Value::Obj(vec![
                                    ("segment", Value::Str(c.segment.clone())),
                                    (
                                        "sources_claimed",
                                        Value::Arr(c.sources_claimed.iter().map(|s| Value::Uint(*s)).collect()),
                                    ),
                                    ("status", Value::Str(c.status.to_string())),
                                    ("excluded", Value::Bool(c.excluded)),
                                    (
                                        "superseded_by",
                                        match c.superseded_by {
                                            Some(ssn) => Value::Uint(ssn),
                                            None => Value::Null,
                                        },
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "discarded_transactions",
                    Value::Arr(
                        out.discarded_transactions
                            .iter()
                            .map(|d| {
                                Value::Obj(vec![
                                    ("txn_id", Value::Uint(d.txn_id)),
                                    (
                                        "commit_seq",
                                        match d.commit_seq {
                                            Some(c) => Value::Uint(c),
                                            None => Value::Null,
                                        },
                                    ),
                                    ("reason", Value::Str(d.reason.to_string())),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("committed_transaction_count", Value::Uint(out.committed_transaction_count)),
                ("deduplicated_record_count", Value::Uint(out.deduplicated_record_count)),
            ]);

            if let Err(e) = write_atomic(&args.out_state, &json::render(&state)) {
                eprintln!("failed to write state: {e}");
                return ExitCode::from(1);
            }
            if let Err(e) = write_atomic(&args.out_report, &json::render(&report)) {
                eprintln!("failed to write report: {e}");
                return ExitCode::from(1);
            }
            ExitCode::from(0)
        }
        Err(Failure::Usage(msg)) => {
            eprintln!("usage error: {msg}");
            ExitCode::from(1)
        }
        Err(failure) => {
            match fs::remove_file(&args.out_state) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    eprintln!("failed to remove stale state output: {e}");
                    return ExitCode::from(1);
                }
            }
            let (error, detail) = match failure {
                Failure::ConflictingDuplicate { commit_seq, op_index, key, segments } => (
                    "conflicting_duplicate",
                    Value::Obj(vec![
                        ("commit_seq", Value::Uint(commit_seq)),
                        ("op_index", Value::Uint(op_index as u64)),
                        ("key", Value::Str(key)),
                        ("segments", Value::Arr(segments.into_iter().map(Value::Str).collect())),
                    ]),
                ),
                Failure::InvalidCompactionGraphCycle { segment, offending_source_ssn } => (
                    "invalid_compaction_graph",
                    Value::Obj(vec![
                        ("segment", Value::Str(segment)),
                        ("offending_source_ssn", Value::Uint(offending_source_ssn)),
                    ]),
                ),
                Failure::InvalidCompactionGraphOverlap { segment_a, segment_b } => (
                    "invalid_compaction_graph",
                    Value::Obj(vec![
                        ("segment_a", Value::Str(segment_a)),
                        ("segment_b", Value::Str(segment_b)),
                    ]),
                ),
                Failure::Usage(_) => unreachable!(),
            };
            let report = Value::Obj(vec![("error", Value::Str(error.to_string())), ("detail", detail)]);
            if let Err(e) = write_atomic(&args.out_report, &json::render(&report)) {
                eprintln!("failed to write report: {e}");
            }
            ExitCode::from(2)
        }
    }
}
