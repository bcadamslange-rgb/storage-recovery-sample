#!/usr/bin/env python3
"""Run public cases against the actual executable; require exact expected bytes."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from public_cases import cases


def canonical(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode('utf-8')


def fingerprint(folder):
    result = {}
    for path in sorted(folder.rglob('*')):
        st = path.lstat()
        payload = os.readlink(path).encode() if path.is_symlink() else path.read_bytes() if path.is_file() else b'directory'
        result[path.relative_to(folder).as_posix()] = [st.st_mode, st.st_mtime_ns, hashlib.sha256(payload).hexdigest()]
    return result


def run_case(binary, case, output):
    inputs = output / 'inputs'
    if inputs.exists():
        expected_names = set(case.files) | set(case.directories) | set(case.symlinks)
        if {p.name for p in inputs.iterdir()} != expected_names:
            raise ValueError('Refusing to reuse an input directory containing unrelated files')
        for filename, payload in case.files.items():
            existing = inputs / filename
            if existing.is_symlink() or not existing.is_file() or existing.read_bytes() != payload:
                raise ValueError('Refusing to replace modified demonstration inputs')
    inputs.mkdir(parents=True, exist_ok=True)
    for filename, payload in case.files.items():
        (inputs / filename).write_bytes(payload)
    for directory in case.directories:
        (inputs / directory).mkdir(exist_ok=True)
    for filename, target in case.symlinks.items():
        (inputs / filename).symlink_to(target)
    before = fingerprint(inputs)
    state = output / 'state.json'; report = output / 'report.json'
    # Exercise replacement on success and stale-state removal on semantic failure.
    state.write_text('STALE STATE', encoding='utf-8')
    report.write_text('STALE REPORT', encoding='utf-8')
    argv = [str(binary), '--segments', str(inputs), '--out-state', str(state), '--out-report', str(report)]
    process = subprocess.run(argv, capture_output=True, timeout=15)
    report_bytes = report.read_bytes() if report.exists() else b''
    state_bytes = state.read_bytes() if state.exists() else None
    checks = {
        'exit_status': process.returncode == case.exit_code,
        'report_exact_bytes': report_bytes == canonical(case.expected_report),
        'state_exact_bytes_or_absent': state_bytes == (canonical(case.expected_state) if case.expected_state is not None else None),
        'inputs_unchanged': before == fingerprint(inputs),
    }
    result = {'case': case.title, 'passed': all(checks.values()), 'checks': checks, 'exit_status': process.returncode,
              'state_sha256': hashlib.sha256(state_bytes).hexdigest() if state_bytes is not None else None,
              'report_sha256': hashlib.sha256(report_bytes).hexdigest(), 'stderr': process.stderr.decode('utf-8', 'replace')}
    if not result['passed']:
        result['expected_report'] = case.expected_report
        result['observed_report'] = report_bytes.decode('utf-8', 'replace')
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--report', default='results/public-verification.json')
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    results = []
    with tempfile.TemporaryDirectory(prefix='storage-public-') as temp:
        for case in cases():
            result = run_case(binary, case, Path(temp) / case.title)
            results.append(result)
            print(('PASS' if result['passed'] else 'FAIL') + ' ' + case.title)
        repeat_case = next(case for case in cases() if case.title == 'committed_binary_values')
        repeated = [run_case(binary, repeat_case, Path(temp) / f'repeat-{i}') for i in range(3)]
        repeat_ok = all(item['passed'] for item in repeated) and len({(r['state_sha256'], r['report_sha256']) for r in repeated}) == 1
        usage = subprocess.run([str(binary)], capture_output=True, timeout=15)
        usage_ok = usage.returncode == 1
    output = {'scope': 'Public synthetic cases and seeded logical-order cases; no private fixtures included.',
              'cases_passed': sum(r['passed'] for r in results), 'cases_total': len(results),
              'repeatability_passed': repeat_ok, 'repeatability_runs': repeated, 'bad_usage_exit_1': usage_ok,
              'passed': all(r['passed'] for r in results) and repeat_ok and usage_ok, 'cases': results}
    target = Path(args.report); target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(output, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    print(f"{output['cases_passed']}/{output['cases_total']} public cases passed; repeatability={repeat_ok}; usage={usage_ok}")
    return 0 if output['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
