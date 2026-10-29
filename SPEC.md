# kvrecover wire format and recovery rules

This is the complete, authoritative specification for the on-disk format and the
recovery semantics your program must implement. Nothing about the resolution
*procedure* is prescribed here beyond what is written explicitly - you must design
the algorithm yourself, but every rule it needs to satisfy is stated below. There
are no undisclosed rules.

All integers are little-endian. All byte offsets are relative to the start of the
record or segment unless stated otherwise.

## 1. Segment files

A segment is a no-follow regular directory entry named `seg-<ssn>.log`, where
`<ssn>` is exactly 16 lowercase hex digits encoding a `u64` segment sequence
number (SSN). SSNs are assigned once, at segment creation, and are never reused.

Only no-follow regular entries participate in filename classification. A regular
file that does not match `seg-[0-9a-f]{16}\.log` exactly (this includes anything
ending in `.tmp`) is not a segment. Regular files whose names end in `.tmp`
(a case-sensitive suffix) are abandoned write attempts - see §7 - and must be
ignored for every purpose except being listed in the report. Directories, symbolic
links, FIFOs, sockets, devices, and other non-regular entries are ignored entirely
even when their basenames match a segment name or end in `.tmp`: directories are
never descended into, and symbolic links are never followed.
Every entry directly inside the `--segments` directory has a UTF-8 basename
in graded evidence, and graded evidence contains no block or character device
entries; the caller-selected directory path itself may contain non-UTF-8 bytes.

### Segment header (24 bytes, offset 0)

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | 4 | `magic` | ASCII `KVS1` |
| 4 | 2 | `format_version` | u16, must be `1` |
| 6 | 2 | `reserved` | must be `0` |
| 8 | 8 | `ssn` | u64, must equal the SSN parsed from the filename |
| 16 | 8 | `created_at_logical` | u64 logical counter - informational only, never used to decide ordering or replay |

A segment is rejected outright (excluded from everything, reason recorded) if:
fewer than 24 bytes exist, `magic` doesn't match, `reserved` is non-zero,
`format_version` isn't `1`, or the header `ssn` doesn't match the filename's SSN.
Checks occur in that order: a short header, bad magic, or non-zero reserved field
has reason `unreadable_header`; a bad format version has reason
`unsupported_version`; only then can a filename/header mismatch have reason
`ssn_mismatch`.

## 2. Records

Records begin at offset 24 and repeat until the segment's torn boundary (§6).

### Record header (fixed 33 bytes) + variable payload + trailing CRC

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `magic` - ASCII `KVR1` |
| 4 | 1 | `record_type` - `0`=PUT, `1`=DELETE, `2`=COMMIT, `3`=ABORT, `4`=COMPACTION_BEGIN, `5`=COMPACTION_END |
| 5 | 8 | `txn_id` (u64) |
| 13 | 8 | `commit_seq` (u64) |
| 21 | 4 | `op_index` (u32) |
| 25 | 4 | `key_len` (u32) |
| 29 | 4 | `value_len` (u32) |
| 33 | key_len | `key` bytes |
| 33+key_len | value_len | `value` bytes |
| 33+key_len+value_len | 4 | `crc32c` (u32) |

`crc32c` is CRC-32C (Castagnoli), computed over every byte of the record from
offset 0 up to (not including) the CRC field itself. Concretely: the reflected,
table-driven algorithm with polynomial `0x82F63B78`, initial value `0xFFFFFFFF`,
and a final XOR of `0xFFFFFFFF` (the same construction as the common CRC-32 used
in zip/png, with the Castagnoli polynomial substituted in) - this is the standard
CRC-32C used by iSCSI, SCTP, and ext4 metadata checksums.

### Per-type field rules

PUT and DELETE `key` bytes are valid UTF-8 in every graded fixture; you do not
need to handle non-UTF-8 user keys. Valid user keys may contain non-ASCII
characters, quotes, backslashes, and control characters other than U+0000.
The COMPACTION_BEGIN `key` field is a binary list of little-endian `u64` source
SSNs, not UTF-8 text. `value` bytes are arbitrary binary.

- **PUT** - `key_len` in `[1, 65536]`, `value_len` in `[0, 1048576]`, `commit_seq != 0`.
  `txn_id != 0` marks a transactional record. `txn_id == 0` is the syntactic
  form used for a materialized record; it affects recovery only when it appears
  inside a complete, surviving compaction bracket (§5).
- **DELETE** - same `txn_id`/`commit_seq` rules as PUT, `key_len` in `[1, 65536]`,
  `value_len` must be `0`.
- **COMMIT** - `txn_id != 0`, `commit_seq != 0`, `op_index == 0`, `key_len == 0`,
  `value_len == 0`.
- **ABORT** - `txn_id != 0`, `commit_seq == 0`, `op_index == 0`, `key_len == 0`,
  `value_len == 0`.
- **COMPACTION_BEGIN** - `txn_id == 0`, `commit_seq == 0`, `op_index == 0`,
  `key_len == 8*N` for some `N` in `[1, 4096]`, `value_len == 0`. The key field
  holds `N` consecutive `u64` source SSNs. Their semantic ordering/bounds are
  validated as part of the compaction graph rules in §5.
- **COMPACTION_END** - `txn_id == 0`, `commit_seq == 0`, `op_index == 0`,
  `key_len == 0`, `value_len == 0`.

Any record that violates a rule above (wrong magic, unknown record type, wrong
field for its type, out-of-range length, bad CRC, or not enough bytes left in
the file to even read it) is invalid.

## 3. Torn boundary

Scan a segment's records strictly in file order starting at offset 24. The first
invalid record - for any reason: truncated header, truncated payload, bad CRC,
wrong magic, unknown record type, or a field-rule violation - marks that
segment's torn boundary. Every byte at or
after that offset is discarded and never examined again, even if a later byte
range would coincidentally parse as a valid-looking record.

## 4. Transactions

Every PUT/DELETE/COMMIT/ABORT record with a non-zero `txn_id` belongs to that
`txn_id`'s group. Across the full retained record order (§6):

- A group is **ABORTED** if it contains one or more ABORT records - this holds
  regardless of whether a COMMIT is also present.
- Otherwise, a group is **COMMITTED** if and only if it contains exactly one
  COMMIT record.
- Otherwise (zero COMMITs, or more than one COMMIT, and no ABORT) the group is
  **DISCARDED - uncommitted**.

Only COMMITTED groups' PUT/DELETE members ever affect final state. A transaction's
PUT/DELETE members may legally appear in an earlier segment than its COMMIT.

Each PUT/DELETE record's own `commit_seq` and `op_index` fields - not its
`txn_id`, its segment's SSN, or its byte offset - determine its place in the
logical write order (§8). `commit_seq` is assigned once per transaction, and is
carried unchanged onto every one of that transaction's PUT/DELETE records.
`op_index` orders multiple operations on the same key inside one transaction.

## 5. Compaction

A segment is a **compaction segment** if its first record is a valid
COMPACTION_BEGIN. In a compaction segment, every record between the
COMPACTION_BEGIN and the matching COMPACTION_END must be a materialized PUT or
DELETE (`txn_id == 0`); no other record type may appear in that range, and no
transactional (`txn_id != 0`) record may appear between BEGIN and END.
Any violation of this - including a COMMIT/ABORT/second BEGIN appearing before
END - makes the compaction **incomplete**, same as a missing END (below).

A compaction is **complete** only if a valid, untorn COMPACTION_END record is
reached with no structural violation since BEGIN. If no such END is reached
before the torn boundary or end of file, the compaction is **incomplete**: the
entire compaction segment is discarded (none of its records are trusted, even
ones that individually look valid), and every SSN it claimed as a source is
replayed normally, as if the compaction had never run.
An incomplete compaction has `excluded: false` and `superseded_by: null` in the
report; its discarded records do not make the compaction itself an excluded
source.

A valid COMPACTION_END record is itself reached and retained for classification,
but is terminal for its segment: nothing after it is read, regardless of whether
the segment is a compaction or whether trailing bytes exist or look valid.

For every **complete** compaction `c`, let `sources(c)` be its claimed source SSN
list (as a set). For two complete compactions `c1`, `c2` with `c1.ssn > c2.ssn`,
`c1` **supersedes** `c2` if either:

- `c2.ssn ∈ sources(c1)` - `c1` was built by compacting `c2`'s own output
  (a compaction of a compaction), or
- `sources(c2) ⊆ sources(c1)` - `c1` re-compacted a superset of the same
  original sources `c2` covered (e.g. a repeated compaction attempt after a
  crash prevented cleanup).

Supersession is transitive: if `c1` supersedes `c2` and `c2` supersedes `c3`,
then `c1` supersedes `c3`. A superseded compaction is excluded from replay
entirely (not just its sources), because its content is now redundant. A source
SSN is excluded from replay if it belongs to `sources(c)` for any complete
compaction `c`, including a complete compaction that is itself superseded. Once
excluded by completed compaction output, a source never resurfaces merely
because that output is later superseded.

Every pair of complete compactions is checked for overlap, even if either one is
superseded by a third compaction. If two complete compactions' source sets
intersect and neither supersedes the other by the rules above, that is an
**invalid compaction graph**: recovery must fail - see §9.

The source SSNs in the first-record COMPACTION_BEGIN that defines a compaction
must be strictly increasing, and each source must be strictly less than that
compaction's own SSN. A duplicate, out-of-order source, self-reference, or
forward reference is an **invalid compaction graph** and recovery must fail -
see §9. This validation applies regardless of whether that compaction later
turns out complete. A later BEGIN is a structural violation inside a compaction
bracket and is otherwise inert; it never defines another compaction.

A claimed source SSN that does not correspond to any segment file present on
disk is **not** an error by itself - that source may simply have already been
deleted after this compaction (or a superseding one) completed.

Everything not excluded - original segments and surviving compaction segments -
replays normally in the order defined by §8.

## 6. What "retained" means

A record is in the retained order only if: its segment was not rejected (§1),
its segment was not excluded as a completed compaction source (§5), it begins
strictly before that segment's torn boundary (§3), and - for compaction
segments - the compaction is complete and not excluded (§5).
Retained records are encountered by numeric segment SSN ascending, then byte
offset ascending. A `txn_id == 0` PUT/DELETE outside a complete surviving
compaction bracket is inert. Everything else never participates in resolution.

## 7. Temporary files

A compactor writes to `seg-<ssn>.log.tmp`, `fsync`s, and only renames to the
final `seg-<ssn>.log` name after a complete write including COMPACTION_END. A
no-follow regular `.tmp` file surviving to be observed is therefore always an
abandoned attempt - by construction, never partially trustworthy. Ignore it
completely; record it in the report's `ignored_temp_files` list and do nothing
else with it. As specified in §1, non-regular entries are not temp files and are
not listed.

## 8. Final state resolution

Collect every PUT/DELETE record that is (a) in the retained order (§6) and (b)
either part of a COMMITTED transactional group (§4) or a materialized record
inside a surviving compaction (§5). Group these by key. Within each key's group,
order entries by `(commit_seq ascending, op_index ascending)`.

If two entries for the same key have identical `(commit_seq, op_index)`: if they
are byte-identical in `record_type` and `value`, treat them as one entry
(deduplicated, no error). For `n` identical applicable entries at one key and
coordinate, increment `deduplicated_record_count` by `n-1`.
If they differ in `record_type` or `value`, this is a **conflicting duplicate**
and recovery must fail - see §9.

Otherwise, the entry with the greatest `(commit_seq, op_index)` for a key decides
that key's final state: present with that value if it is a PUT, absent if it is
a DELETE.

## 9. Failure mode

Two conditions make recovery fail outright rather than produce a best-effort
result: an invalid compaction graph (§5) and a conflicting duplicate (§8). On
either, the program must exit with status `2`, write a `report.json` describing
the specific failure, and ensure `state.json` is absent. This means removing a
pre-existing state file at the caller-provided output path before returning.

A separate failure mode - bad command-line usage, or a segments directory that
cannot be opened - exits with status `1` and writes neither file.

Any other outcome (recovery ran to completion) exits `0` and writes both files.

## 10. Output contract

On exit `0`, both files are UTF-8, use `\n` line endings only, are written with
2-space JSON indentation, and end with exactly one trailing newline. On exit
`2`, the same rules apply to `report.json`. Object keys appear in the exact
order shown below - this is not alphabetical.

Formatting is exact, not just "valid JSON", because byte-identical output is
part of what's graded: no space after `,` (a newline follows immediately), a
single space after `:`, an empty array/object prints inline as `[]`/`{}` rather
than spanning lines, and `value_b64` is standard RFC 4648 base64 with `=`
padding.

JSON strings escape `"` and `\` with a backslash; use `\b`, `\f`, `\n`, `\r`,
and `\t` for those five controls and lowercase `\u00xx` for every other scalar
from U+0001 through U+001F. Other valid UTF-8 characters are emitted directly.

Recovery is read-only with respect to the segments directory: it must never
write, rename, delete, or otherwise modify anything under the path given by
`--segments`.
The caller supplies distinct state and report output directory entries in
writable directories outside that evidence tree, including after resolution
of existing parent-directory symlinks. Invocations violating these caller
preconditions are outside the required recovery domain.

### `state.json` (only on exit `0`)

```json
{
  "entries": [ { "key": "<utf-8 string>", "value_b64": "<base64>" } ],
  "count": 0
}
```
`entries` sorted ascending by `key` (byte-wise UTF-8 comparison).

### `report.json` (exit `0`)

```json
{
  "accepted_segments": ["seg-....log"],
  "rejected_segments": [ { "file": "...", "reason": "..." } ],
  "ignored_temp_files": ["...tmp"],
  "compactions": [ { "segment": "...", "sources_claimed": [0], "status": "complete", "excluded": false, "superseded_by": null } ],
  "discarded_transactions": [ { "txn_id": 0, "commit_seq": null, "reason": "..." } ],
  "committed_transaction_count": 0,
  "deduplicated_record_count": 0
}
```
Every array sorted ascending by its first field (filename, or `txn_id` for
`discarded_transactions`). `rejected_segments[].reason` is one of:
`unreadable_header`, `ssn_mismatch`, `unsupported_version`. `compactions[].status`
is `complete` or `incomplete`. `discarded_transactions[].reason` is one of:
`aborted`, `uncommitted_no_commit`, `uncommitted_multiple_commits`.

`accepted_segments` contains every header-valid filename candidate, including a
segment that later tears. For a complete compaction, `excluded` is true exactly
when it is superseded, and `superseded_by` is the numeric `u64` SSN, written as
a JSON integer like `sources_claimed` entries (not a filename string), of the
lowest-SSN complete compaction that directly supersedes it (not merely a
transitive successor); otherwise the value is `null`. In a discarded
transaction, `commit_seq` is the `commit_seq` from its last COMMIT in retained
encounter order, or `null` if it has no COMMIT.

`committed_transaction_count` counts COMMITTED groups (§4) found among
transactional (non-compaction) segments that are not excluded as a compaction
source - materialized compaction records are not transactions and are never
counted here, even though they originated from one. A segment excluded as a
compaction source is never scanned for transactions at all, including when the
compaction that excludes it is later superseded (§5).

### `report.json` (exit `2`)

The exact `detail` shape is determined by the first applicable condition below:

1. A source-order, duplicate-source, self-reference, or forward-reference error:
   `{ "segment": "seg-....log", "offending_source_ssn": 0 }`.
2. A non-superseding overlap: `{ "segment_a": "seg-....log", "segment_b": "seg-....log" }`, with the lower SSN first.
3. A conflicting duplicate: `{ "commit_seq": 0, "op_index": 0, "key": "<utf-8 string>", "segments": ["seg-....log"] }`, with unique segment names sorted ascending.
   `segments` includes every applicable entry at the conflicting key and
   coordinate, including entries whose value agrees with another entry.

The top-level shape is `{ "error": "invalid_compaction_graph", "detail": ... }`
for cases 1–2 and `{ "error": "conflicting_duplicate", "detail": ... }` for
case 3. The displayed field order is normative. If more than one error exists,
source validation takes precedence over overlap, which takes precedence over a
conflicting duplicate. Within source validation, choose numeric segment SSN
then source-list order; within overlap, choose the lowest `(segment_a SSN,
segment_b SSN)` pair; within conflicts, choose key byte order then the lowest
`(commit_seq, op_index)`.

## 11. Command-line contract

```
kvrecover --segments <DIR> --out-state <FILE> --out-report <FILE>
```
All three flags are required exactly once, plain ASCII double-hyphen. A missing
or repeated flag, missing flag value, or unrecognized argument is bad usage and
exits `1`. The paths given are caller-selected subject to the output-path
preconditions in §10; do not assume any fixed path.
On Unix, the caller-selected path arguments may contain non-UTF-8 filesystem
bytes. Treat path values as opaque paths; only the flag names are ASCII text.
