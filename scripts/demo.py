#!/usr/bin/env python3
"""Execute three public demonstrations and save real command results."""
from pathlib import Path
import argparse
import json
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from public_cases import cases
from verify import run_case

parser = argparse.ArgumentParser()
parser.add_argument('--binary', required=True)
parser.add_argument('--output', default='results/demo')
args = parser.parse_args()
output = Path(args.output)
binary = Path(args.binary).resolve()
selected = ['committed_binary_values', 'bad_crc_delete_stops_scan', 'conflicting_duplicate']
log = []
for title in selected:
    case = next(c for c in cases() if c.title == title)
    folder = output / title
    # Reuse only identical generated inputs; verification refuses unrelated data.
    result = run_case(binary, case, folder)
    log.append(result)
    print(f"\nCASE: {title}\nExit status: {result['exit_status']}\nChecks passed: {result['passed']}")
    state = folder / 'state.json'
    print(state.read_text(encoding='utf-8') if state.exists() else 'State file: absent')
    print((folder / 'report.json').read_text(encoding='utf-8'))
output.mkdir(parents=True, exist_ok=True)
(output / 'demo-results.json').write_text(json.dumps(log, indent=2) + '\n', encoding='utf-8')
sys.exit(0 if all(result['passed'] for result in log) else 1)
