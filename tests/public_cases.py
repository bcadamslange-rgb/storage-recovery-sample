"""Public synthetic cases, independently encoded from SPEC.md. No private corpus."""
from dataclasses import dataclass, field
import base64
import random
import struct


def crc32c(data):
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 if crc & 1 else 0)
    return crc ^ 0xFFFFFFFF


def record(kind, txn=0, seq=0, op=0, key=b'', value=b''):
    if isinstance(key, str):
        key = key.encode('utf-8')
    raw = struct.pack('<4sBQQIII', b'KVR1', kind, txn, seq, op, len(key), len(value)) + key + value
    return raw + struct.pack('<I', crc32c(raw))


def header(ssn):
    return struct.pack('<4sHHQQ', b'KVS1', 1, 0, ssn, 0)


def name(ssn):
    return f'seg-{ssn:016x}.log'


def segment(ssn, *records):
    return header(ssn) + b''.join(records)


def transaction(txn, seq, key, value):
    return record(0, txn, seq, key=key, value=value) + record(2, txn, seq)


def state(entries):
    return {'entries': [{'key': key, 'value_b64': base64.b64encode(value).decode()} for key, value in sorted(entries.items())], 'count': len(entries)}


def report(ssns=(), *, rejected=(), temps=(), compactions=(), discarded=(), committed=0, dedup=0):
    return {'accepted_segments': [name(s) for s in sorted(ssns)],
            'rejected_segments': list(rejected), 'ignored_temp_files': list(temps),
            'compactions': list(compactions), 'discarded_transactions': list(discarded),
            'committed_transaction_count': committed, 'deduplicated_record_count': dedup}


@dataclass
class Case:
    title: str
    files: dict
    expected_state: object
    expected_report: dict
    exit_code: int = 0
    directories: list = field(default_factory=list)
    symlinks: dict = field(default_factory=dict)


def cases():
    result = []
    def add(title, files, entries=None, output=None, exit_code=0, **kwargs):
        result.append(Case(title, files, state(entries or {}) if exit_code == 0 else None, output or report(), exit_code, **kwargs))
    add('empty_directory', {})
    add('committed_binary_values', {name(1): segment(1, transaction(1, 1, 'one_zero', b'\0'), transaction(2, 2, 'two_zero', b'\0\0'), transaction(3, 3, 'three_zero', b'\0\0\0'))},
        {'one_zero': b'\0', 'two_zero': b'\0\0', 'three_zero': b'\0\0\0'}, report([1], committed=3))
    add('zero_ssn_and_empty_value', {name(0): segment(0, transaction(1, 1, 'empty', b''))}, {'empty': b''}, report([0], committed=1))
    add('logical_order_beats_filename', {name(1): segment(1, transaction(1, 20, 'status', b'new')), name(2): segment(2, transaction(2, 10, 'status', b'old'))}, {'status': b'new'}, report([1, 2], committed=2))
    add('cross_file_transaction', {name(1): segment(1, record(0, 8, 9, key='item', value=b'persisted')), name(2): segment(2, record(2, 8, 9))}, {'item': b'persisted'}, report([1, 2], committed=1))
    add('operation_index_and_delete', {name(1): segment(1, record(0, 1, 4, 0, 'item', b'a'), record(1, 1, 4, 1, 'item'), record(0, 1, 4, 2, 'item', b'b'), record(2, 1, 4))}, {'item': b'b'}, report([1], committed=1))
    add('abort_wins_over_commit', {name(1): segment(1, transaction(1, 4, 'item', b'a'), record(3, 1))}, {}, report([1], discarded=[{'txn_id': 1, 'commit_seq': 4, 'reason': 'aborted'}]))
    add('missing_commit', {name(1): segment(1, record(0, 1, 4, key='item', value=b'a'))}, {}, report([1], discarded=[{'txn_id': 1, 'commit_seq': None, 'reason': 'uncommitted_no_commit'}]))
    add('multiple_commits', {name(1): segment(1, transaction(1, 4, 'item', b'a'), record(2, 1, 4))}, {}, report([1], discarded=[{'txn_id': 1, 'commit_seq': 4, 'reason': 'uncommitted_multiple_commits'}]))
    duplicate = {name(1): segment(1, transaction(1, 9, 'item', b'a')), name(2): segment(2, transaction(2, 9, 'item', b'a'))}
    add('identical_duplicate', duplicate, {'item': b'a'}, report([1, 2], committed=2, dedup=1))
    conflicting = dict(duplicate)
    conflicting[name(2)] = segment(2, transaction(2, 9, 'item', b'b'))
    add('conflicting_duplicate', conflicting, output={'error': 'conflicting_duplicate', 'detail': {'commit_seq': 9, 'op_index': 0, 'key': 'item', 'segments': [name(1), name(2)]}}, exit_code=2)
    bad_delete = bytearray(record(1, 2, 2, key='item')); bad_delete[-1] ^= 1
    add('bad_crc_delete_stops_scan', {name(1): segment(1, transaction(1, 1, 'item', b'kept'), bytes(bad_delete), record(2, 2, 2))}, {'item': b'kept'}, report([1], committed=1))
    bad_abort = bytearray(record(3, 1)); bad_abort[-1] ^= 1
    add('bad_crc_abort_stops_scan', {name(1): segment(1, transaction(1, 1, 'item', b'kept'), bytes(bad_abort))}, {'item': b'kept'}, report([1], committed=1))
    add('truncated_tail_does_not_resync', {name(1): segment(1, transaction(1, 1, 'item', b'kept'), b'bad', transaction(2, 2, 'item', b'wrong'))}, {'item': b'kept'}, report([1], committed=1))
    add('bad_header_magic', {name(1): b'NOPE' + header(1)[4:]}, output=report(rejected=[{'file': name(1), 'reason': 'unreadable_header'}]))
    bad_version = bytearray(header(1)); bad_version[4] = 2
    add('unsupported_header_version', {name(1): bytes(bad_version)}, output=report(rejected=[{'file': name(1), 'reason': 'unsupported_version'}]))
    add('filename_header_mismatch', {name(1): header(2)}, output=report(rejected=[{'file': name(1), 'reason': 'ssn_mismatch'}]))
    add('temporary_file_and_unrelated_name', {'pending.tmp': b'unfinished', 'readme.txt': b'not a segment', name(1): header(1)}, output=report([1], temps=['pending.tmp']))
    add('nonregular_entries_are_ignored', {name(1): header(1)}, output=report([1]), directories=[name(2), 'directory.tmp'])
    begin = record(4, key=struct.pack('<Q', 1))
    materialized = record(0, seq=10, key='item', value=b'compacted')
    compact = {'segment': name(10), 'sources_claimed': [1], 'status': 'complete', 'excluded': False, 'superseded_by': None}
    source = segment(1, transaction(1, 1, 'item', b'original'))
    add('complete_compaction_replaces_source', {name(1): source, name(10): segment(10, begin, materialized, record(5))}, {'item': b'compacted'}, report([1, 10], compactions=[compact]))
    incomplete = dict(compact, status='incomplete')
    add('incomplete_compaction_keeps_source', {name(1): source, name(10): segment(10, begin, materialized)}, {'item': b'original'}, report([1, 10], compactions=[incomplete], committed=1))
    add('compaction_end_is_terminal', {name(1): segment(1, record(5), transaction(1, 1, 'ignored', b'ignored'))}, output=report([1]))
    add('invalid_compaction_source', {name(10): segment(10, record(4, key=struct.pack('<Q', 10)))}, output={'error': 'invalid_compaction_graph', 'detail': {'segment': name(10), 'offending_source_ssn': 10}}, exit_code=2)
    add('unicode_and_json_escaping', {name(1): segment(1, transaction(1, 1, 'café\n"\\', b'\xff\x00'))}, {'café\n"\\': b'\xff\x00'}, report([1], committed=1))
    rng = random.Random(43210)
    for index in range(40):
        order = list(range(1, 8)); rng.shuffle(order)
        files = {name(ssn): segment(ssn, transaction(ssn, seq, 'counter', str(seq).encode())) for ssn, seq in enumerate(order, 1)}
        add(f'logical_order_seeded_{index:02d}', files, {'counter': b'7'}, report(range(1, 8), committed=7))
    return result
