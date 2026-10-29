//! Binary layout defined in SPEC.md sections 1-2. Pure decode: nothing here
//! decides recovery semantics, only whether bytes parse into a well-formed
//! header/record at all.

use crate::crc32c::crc32c;

pub const SEGMENT_MAGIC: &[u8; 4] = b"KVS1";
pub const RECORD_MAGIC: &[u8; 4] = b"KVR1";
pub const SEGMENT_HEADER_LEN: usize = 24;
pub const RECORD_HEADER_LEN: usize = 33;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    Put,
    Delete,
    Commit,
    Abort,
    CompactionBegin,
    CompactionEnd,
}

impl RecordType {
    fn from_u8(b: u8) -> Option<RecordType> {
        match b {
            0 => Some(RecordType::Put),
            1 => Some(RecordType::Delete),
            2 => Some(RecordType::Commit),
            3 => Some(RecordType::Abort),
            4 => Some(RecordType::CompactionBegin),
            5 => Some(RecordType::CompactionEnd),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Record {
    pub record_type: RecordType,
    pub txn_id: u64,
    pub commit_seq: u64,
    pub op_index: u32,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub enum SegmentHeaderError {
    Unreadable,
    UnsupportedVersion,
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentHeader {
    pub ssn: u64,
    #[allow(dead_code)]
    pub created_at_logical: u64,
}

/// Parses the 24-byte segment header.
pub fn parse_segment_header(buf: &[u8]) -> Result<SegmentHeader, SegmentHeaderError> {
    if buf.len() < SEGMENT_HEADER_LEN {
        return Err(SegmentHeaderError::Unreadable);
    }
    if &buf[0..4] != SEGMENT_MAGIC {
        return Err(SegmentHeaderError::Unreadable);
    }
    let format_version = u16::from_le_bytes([buf[4], buf[5]]);
    let reserved = u16::from_le_bytes([buf[6], buf[7]]);
    if reserved != 0 {
        return Err(SegmentHeaderError::Unreadable);
    }
    if format_version != 1 {
        return Err(SegmentHeaderError::UnsupportedVersion);
    }
    let ssn = u64::from_le_bytes(buf[8..16].try_into().unwrap());
    let created_at_logical = u64::from_le_bytes(buf[16..24].try_into().unwrap());
    Ok(SegmentHeader { ssn, created_at_logical })
}

fn type_fields_valid(
    t: RecordType,
    txn_id: u64,
    commit_seq: u64,
    op_index: u32,
    key_len: usize,
    value_len: usize,
) -> bool {
    match t {
        RecordType::Put => {
            commit_seq != 0 && (1..=65536).contains(&key_len) && value_len <= 1_048_576
        }
        RecordType::Delete => {
            commit_seq != 0 && (1..=65536).contains(&key_len) && value_len == 0
        }
        RecordType::Commit => {
            txn_id != 0 && commit_seq != 0 && op_index == 0 && key_len == 0 && value_len == 0
        }
        RecordType::Abort => {
            txn_id != 0 && commit_seq == 0 && op_index == 0 && key_len == 0 && value_len == 0
        }
        RecordType::CompactionBegin => {
            txn_id == 0
                && commit_seq == 0
                && op_index == 0
                && key_len % 8 == 0
                && key_len / 8 >= 1
                && key_len / 8 <= 4096
                && value_len == 0
        }
        RecordType::CompactionEnd => {
            txn_id == 0 && commit_seq == 0 && op_index == 0 && key_len == 0 && value_len == 0
        }
    }
}

/// Attempts to parse one record starting at `buf[offset..]`. Returns the record
/// and the offset immediately past it, or `None` if invalid - the caller treats
/// `None` as this segment's torn boundary at `offset`.
pub fn parse_record(buf: &[u8], offset: usize) -> Option<(Record, usize)> {
    if offset + RECORD_HEADER_LEN > buf.len() {
        return None;
    }
    let h = &buf[offset..offset + RECORD_HEADER_LEN];
    if &h[0..4] != RECORD_MAGIC {
        return None;
    }
    let record_type = RecordType::from_u8(h[4])?;
    let txn_id = u64::from_le_bytes(h[5..13].try_into().unwrap());
    let commit_seq = u64::from_le_bytes(h[13..21].try_into().unwrap());
    let op_index = u32::from_le_bytes(h[21..25].try_into().unwrap());
    let key_len = u32::from_le_bytes(h[25..29].try_into().unwrap()) as usize;
    let value_len = u32::from_le_bytes(h[29..33].try_into().unwrap()) as usize;

    let key_start = offset + RECORD_HEADER_LEN;
    let value_start = key_start.checked_add(key_len)?;
    let crc_start = value_start.checked_add(value_len)?;
    let crc_end = crc_start.checked_add(4)?;
    if crc_end > buf.len() {
        return None;
    }

    if !type_fields_valid(record_type, txn_id, commit_seq, op_index, key_len, value_len) {
        return None;
    }

    let key = buf[key_start..value_start].to_vec();
    let value = buf[value_start..crc_start].to_vec();
    let stored_crc = u32::from_le_bytes(buf[crc_start..crc_end].try_into().unwrap());
    let computed_crc = crc32c(&buf[offset..crc_start]);
    if stored_crc != computed_crc {
        return None;
    }

    Some((
        Record { record_type, txn_id, commit_seq, op_index, key, value },
        crc_end,
    ))
}

/// Decodes a COMPACTION_BEGIN payload into its list of source SSNs, in the
/// order they appear on disk (validation of ordering/bounds happens in
/// `recover.rs`, which has the context - this segment's own SSN - to check it).
pub fn decode_sources(key: &[u8]) -> Vec<u64> {
    key.chunks_exact(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect()
}
