//! Recovery algorithm. Implements SPEC.md sections 3-9. This module has no
//! knowledge of CLI flags or JSON - it takes a directory and returns a fully
//! resolved outcome or a specific failure.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::format::{self, Record, RecordType, SegmentHeaderError};

pub struct RejectedSegment {
    pub file: String,
    pub reason: &'static str,
}

pub struct CompactionInfo {
    pub segment: String,
    pub sources_claimed: Vec<u64>,
    pub status: &'static str, // "complete" | "incomplete"
    pub excluded: bool,
    pub superseded_by: Option<u64>,
}

pub struct DiscardedTxn {
    pub txn_id: u64,
    pub commit_seq: Option<u64>,
    pub reason: &'static str,
}

pub struct StateEntry {
    pub key: String,
    pub value: Vec<u8>,
}

pub struct RecoveryOutput {
    pub entries: Vec<StateEntry>,
    pub accepted_segments: Vec<String>,
    pub rejected_segments: Vec<RejectedSegment>,
    pub ignored_temp_files: Vec<String>,
    pub compactions: Vec<CompactionInfo>,
    pub discarded_transactions: Vec<DiscardedTxn>,
    pub committed_transaction_count: u64,
    pub deduplicated_record_count: u64,
}

pub enum Failure {
    Usage(String),
    ConflictingDuplicate { commit_seq: u64, op_index: u32, key: String, segments: Vec<String> },
    InvalidCompactionGraphCycle { segment: String, offending_source_ssn: u64 },
    InvalidCompactionGraphOverlap { segment_a: String, segment_b: String },
}

struct ScannedSegment {
    name: String,
    ssn: u64,
    records: Vec<(Record, usize)>, // (record, byte offset the record started at)
}

fn is_segment_filename(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("seg-")?;
    let hex = rest.strip_suffix(".log")?;
    if hex.len() != 16 || !hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

fn scan_records(buf: &[u8]) -> Vec<(Record, usize)> {
    let mut out = Vec::new();
    let mut offset = format::SEGMENT_HEADER_LEN;
    while let Some((record, next_offset)) = format::parse_record(buf, offset) {
        let terminal = record.record_type == RecordType::CompactionEnd;
        out.push((record, offset));
        offset = next_offset;
        if terminal {
            break;
        }
    }
    out
}

/// Index of the first CompactionEnd in `records`, if this is a well-formed
/// compaction prefix (BEGIN, then only materialized PUT/DELETE, then END).
/// Returns `None` when the compaction is incomplete: either no END was reached
/// or a structural violation appeared before it.
fn find_compaction_end(records: &[(Record, usize)]) -> Option<usize> {
    for (i, (r, _)) in records.iter().enumerate().skip(1) {
        match r.record_type {
            RecordType::Put | RecordType::Delete => {
                if r.txn_id != 0 {
                    return None; // structural violation -> incomplete
                }
            }
            RecordType::CompactionEnd => return Some(i),
            _ => return None, // structural violation -> incomplete
        }
    }
    None
}

pub fn recover(segments_dir: &Path) -> Result<RecoveryOutput, Failure> {
    let dir_entries = fs::read_dir(segments_dir)
        .map_err(|e| Failure::Usage(format!("cannot read segments directory: {e}")))?;

    let mut candidate_names: Vec<String> = Vec::new();
    let mut ignored_temp_files: Vec<String> = Vec::new();

    for entry in dir_entries {
        let entry = entry.map_err(|e| Failure::Usage(format!("directory read error: {e}")))?;
        // Classification is explicitly no-follow and regular-file-only.  In
        // particular, a matching directory or symlink is neither a segment nor
        // an abandoned temporary file.
        let file_type = entry
            .file_type()
            .map_err(|e| Failure::Usage(format!("directory entry type error: {e}")))?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_segment_filename(&name).is_some() {
            candidate_names.push(name);
        } else if name.ends_with(".tmp") {
            ignored_temp_files.push(name);
        }
    }
    candidate_names.sort();
    ignored_temp_files.sort();

    let mut accepted_segments: Vec<String> = Vec::new();
    let mut rejected_segments: Vec<RejectedSegment> = Vec::new();
    let mut scanned: Vec<ScannedSegment> = Vec::new();

    for name in &candidate_names {
        let name_ssn = is_segment_filename(name).unwrap();
        let path = segments_dir.join(name);
        let buf = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => {
                rejected_segments.push(RejectedSegment { file: name.clone(), reason: "unreadable_header" });
                continue;
            }
        };
        let header = match format::parse_segment_header(&buf) {
            Ok(h) => h,
            Err(SegmentHeaderError::Unreadable) => {
                rejected_segments.push(RejectedSegment { file: name.clone(), reason: "unreadable_header" });
                continue;
            }
            Err(SegmentHeaderError::UnsupportedVersion) => {
                rejected_segments.push(RejectedSegment { file: name.clone(), reason: "unsupported_version" });
                continue;
            }
        };
        if header.ssn != name_ssn {
            rejected_segments.push(RejectedSegment { file: name.clone(), reason: "ssn_mismatch" });
            continue;
        }
        accepted_segments.push(name.clone());
        let records = scan_records(&buf);
        scanned.push(ScannedSegment { name: name.clone(), ssn: header.ssn, records });
    }

    // Classify each accepted segment as a compaction or transactional, and
    // validate the first-record COMPACTION_BEGIN's source list against this
    // segment's own ssn regardless of completeness (SPEC.md section 5).
    struct Compaction<'a> {
        seg: &'a ScannedSegment,
        sources: Vec<u64>,
        complete: bool,
        materialized: Vec<Record>, // only meaningful if complete
    }

    let mut compactions: Vec<Compaction<'_>> = Vec::new();
    let mut transactional: Vec<&ScannedSegment> = Vec::new();

    for seg in &scanned {
        let is_compaction_start = seg
            .records
            .first()
            .map(|(r, _)| r.record_type == RecordType::CompactionBegin)
            .unwrap_or(false);

        if !is_compaction_start {
            transactional.push(seg);
            continue;
        }

        let begin = &seg.records[0].0;
        let sources = format::decode_sources(&begin.key);
        for (i, &s) in sources.iter().enumerate() {
            if s >= seg.ssn {
                return Err(Failure::InvalidCompactionGraphCycle {
                    segment: seg.name.clone(),
                    offending_source_ssn: s,
                });
            }
            if i > 0 && sources[i - 1] >= s {
                return Err(Failure::InvalidCompactionGraphCycle {
                    segment: seg.name.clone(),
                    offending_source_ssn: s,
                });
            }
        }

        match find_compaction_end(&seg.records) {
            Some(end_idx) => {
                let materialized: Vec<Record> = seg.records[1..end_idx]
                    .iter()
                    .map(|(r, _)| r.clone())
                    .collect();
                compactions.push(Compaction { seg, sources, complete: true, materialized });
            }
            None => {
                compactions.push(Compaction { seg, sources, complete: false, materialized: Vec::new() });
            }
        }
    }

    // Transitive supersession among complete compactions (SPEC.md section 5).
    let complete_idx: Vec<usize> = compactions
        .iter()
        .enumerate()
        .filter(|(_, c)| c.complete)
        .map(|(i, _)| i)
        .collect();

    let ncomp = compactions.len();
    let mut direct = vec![vec![false; ncomp]; ncomp];
    for &newer in &complete_idx {
        let newer_sources: HashSet<u64> = compactions[newer].sources.iter().copied().collect();
        for &older in &complete_idx {
            if compactions[newer].seg.ssn <= compactions[older].seg.ssn {
                continue;
            }
            let older_sources: HashSet<u64> = compactions[older].sources.iter().copied().collect();
            direct[newer][older] = newer_sources.contains(&compactions[older].seg.ssn)
                || older_sources.is_subset(&newer_sources);
        }
    }

    // Supersession is the transitive closure of the directional direct rule.
    // Do not invent new edges by unioning source coverage across siblings.
    let mut supersedes = direct.clone();
    for &via in &complete_idx {
        for &newer in &complete_idx {
            for &older in &complete_idx {
                supersedes[newer][older] |= supersedes[newer][via] && supersedes[via][older];
            }
        }
    }

    // Every complete pair participates in overlap validation, even if one or
    // both members are superseded by a third compaction.
    for (pos, &i) in complete_idx.iter().enumerate() {
        for &j in &complete_idx[pos + 1..] {
            let i_sources: HashSet<u64> = compactions[i].sources.iter().copied().collect();
            let j_sources: HashSet<u64> = compactions[j].sources.iter().copied().collect();
            if !i_sources.is_disjoint(&j_sources) && !supersedes[i][j] && !supersedes[j][i] {
                return Err(Failure::InvalidCompactionGraphOverlap {
                    segment_a: compactions[i].seg.name.clone(),
                    segment_b: compactions[j].seg.name.clone(),
                });
            }
        }
    }

    let excluded_compaction_idx: HashSet<usize> = complete_idx
        .iter()
        .copied()
        .filter(|&older| complete_idx.iter().any(|&newer| supersedes[newer][older]))
        .collect();

    let mut excluded_source_ssns: HashSet<u64> = HashSet::new();
    for &i in &complete_idx {
        // Once a complete compaction has materialized its sources, those
        // sources never resurface merely because a later compaction supersedes
        // this compaction.
        excluded_source_ssns.extend(compactions[i].sources.iter().copied());
    }

    // Build the report's compaction entries.
    let mut compaction_infos: Vec<CompactionInfo> = compactions
        .iter()
        .enumerate()
        .map(|(i, c)| CompactionInfo {
            segment: c.seg.name.clone(),
            sources_claimed: c.sources.clone(),
            status: if c.complete { "complete" } else { "incomplete" },
            excluded: excluded_compaction_idx.contains(&i),
            superseded_by: complete_idx
                .iter()
                .copied()
                .filter(|&newer| direct[newer][i])
                .min_by_key(|&newer| compactions[newer].seg.ssn)
                .map(|newer| compactions[newer].seg.ssn),
        })
        .collect();
    compaction_infos.sort_by(|a, b| a.segment.cmp(&b.segment));

    // Collect applicable (key, commit_seq, op_index, is_put, value) entries
    // from surviving compactions and from committed transactions.
    struct Applicable {
        key: String,
        commit_seq: u64,
        op_index: u32,
        is_put: bool,
        value: Vec<u8>,
        source_segment: String,
    }
    let mut applicable: Vec<Applicable> = Vec::new();

    for (i, c) in compactions.iter().enumerate() {
        if !c.complete || excluded_compaction_idx.contains(&i) {
            continue;
        }
        for r in &c.materialized {
            applicable.push(Applicable {
                key: String::from_utf8(r.key.clone()).unwrap(),
                commit_seq: r.commit_seq,
                op_index: r.op_index,
                is_put: r.record_type == RecordType::Put,
                value: r.value.clone(),
                source_segment: c.seg.name.clone(),
            });
        }
    }

    // Global transaction grouping across all retained transactional segments,
    // in (ssn, offset) order. Segments that are a source of any complete
    // compaction are excluded here (SPEC.md section 5) - their content is
    // superseded, whether or not the file itself still exists.
    let mut ordered_transactional: Vec<&ScannedSegment> = transactional
        .into_iter()
        .filter(|seg| !excluded_source_ssns.contains(&seg.ssn))
        .collect();
    ordered_transactional.sort_by_key(|s| s.ssn);

    use std::collections::BTreeMap;
    struct TxnGroup {
        commits: u32,
        aborted: bool,
        members: Vec<(Record, String)>, // (PUT/DELETE record, its segment name)
        commit_seq_seen: Option<u64>,
    }
    let mut groups: BTreeMap<u64, TxnGroup> = BTreeMap::new();

    for seg in &ordered_transactional {
        for (r, _) in &seg.records {
            if r.txn_id == 0 {
                // A txn_id==0 PUT/DELETE is syntactically valid (SPEC.md section
                // 2) but inert outside a complete, surviving compaction bracket
                // (section 6), so it never joins a transaction group.
                continue;
            }
            let group = groups.entry(r.txn_id).or_insert(TxnGroup {
                commits: 0,
                aborted: false,
                members: Vec::new(),
                commit_seq_seen: None,
            });
            match r.record_type {
                RecordType::Commit => {
                    group.commits += 1;
                    group.commit_seq_seen = Some(r.commit_seq);
                }
                RecordType::Abort => group.aborted = true,
                RecordType::Put | RecordType::Delete => {
                    group.members.push((r.clone(), seg.name.clone()))
                }
                _ => {}
            }
        }
    }

    let mut discarded_transactions: Vec<DiscardedTxn> = Vec::new();
    let mut committed_transaction_count: u64 = 0;

    for (txn_id, group) in &groups {
        if group.aborted {
            discarded_transactions.push(DiscardedTxn {
                txn_id: *txn_id,
                commit_seq: group.commit_seq_seen,
                reason: "aborted",
            });
            continue;
        }
        if group.commits == 1 {
            committed_transaction_count += 1;
            for (r, seg_name) in &group.members {
                applicable.push(Applicable {
                    key: String::from_utf8(r.key.clone()).unwrap(),
                    commit_seq: r.commit_seq,
                    op_index: r.op_index,
                    is_put: r.record_type == RecordType::Put,
                    value: r.value.clone(),
                    source_segment: seg_name.clone(),
                });
            }
        } else {
            let reason = if group.commits == 0 { "uncommitted_no_commit" } else { "uncommitted_multiple_commits" };
            discarded_transactions.push(DiscardedTxn { txn_id: *txn_id, commit_seq: group.commit_seq_seen, reason });
        }
    }
    discarded_transactions.sort_by_key(|d| d.txn_id);

    // Dedup / conflict detection, then per-key resolution.
    let mut by_key: BTreeMap<String, Vec<&Applicable>> = BTreeMap::new();
    for a in &applicable {
        by_key.entry(a.key.clone()).or_default().push(a);
    }

    let mut entries: Vec<StateEntry> = Vec::new();
    let mut deduplicated_record_count: u64 = 0;

    for (key, mut items) in by_key {
        items.sort_by_key(|a| (a.commit_seq, a.op_index));
        // Detect (commit_seq, op_index) collisions.
        let mut i = 0;
        let mut deduped: Vec<&Applicable> = Vec::new();
        while i < items.len() {
            let mut j = i + 1;
            while j < items.len() && items[j].commit_seq == items[i].commit_seq && items[j].op_index == items[i].op_index {
                j += 1;
            }
            let group = &items[i..j];
            let first = group[0];
            for other in &group[1..] {
                if other.is_put != first.is_put || other.value != first.value {
                    let mut segs: Vec<String> = group.iter().map(|a| a.source_segment.clone()).collect();
                    segs.sort();
                    segs.dedup();
                    return Err(Failure::ConflictingDuplicate {
                        commit_seq: first.commit_seq,
                        op_index: first.op_index,
                        key: key.clone(),
                        segments: segs,
                    });
                }
                deduplicated_record_count += 1;
            }
            deduped.push(first);
            i = j;
        }
        if let Some(winner) = deduped.last() {
            if winner.is_put {
                entries.push(StateEntry { key: key.clone(), value: winner.value.clone() });
            }
        }
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    Ok(RecoveryOutput {
        entries,
        accepted_segments,
        rejected_segments,
        ignored_temp_files,
        compactions: compaction_infos,
        discarded_transactions,
        committed_transaction_count,
        deduplicated_record_count,
    })
}
