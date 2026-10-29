#!/usr/bin/env python3
"""Prove public checks reject two deliberately incorrect source mutations."""
from pathlib import Path
import json
import shutil
import subprocess
import tempfile
import sys

ROOT = Path(__file__).resolve().parents[1]
controls = {
    'physical_order': ('recover.rs', 'items.sort_by_key(|a| (a.commit_seq, a.op_index));', 'items.sort_by_key(|a| (a.source_segment.clone(), a.op_index, a.commit_seq));'),
    'crc_omission': ('format.rs', 'if stored_crc != computed_crc {', 'if stored_crc != computed_crc && record_type != RecordType::Delete && record_type != RecordType::Abort {'),
}
with tempfile.TemporaryDirectory(prefix='storage-controls-') as folder:
    for label, (filename, before, after) in controls.items():
        staging = Path(folder)/label
        shutil.copytree(ROOT/'src', staging/'src')
        target = staging/'src'/filename
        text = target.read_text(encoding='utf-8')
        assert text.count(before) == 1
        target.write_text(text.replace(before, after), encoding='utf-8')
        binary = staging/('kvrecover.exe' if sys.platform == 'win32' else 'kvrecover')
        subprocess.run(['rustc', '--edition=2021', '-O', str(staging/'src/main.rs'), '-o', str(binary)], check=True)
        output = staging/'result.json'
        run = subprocess.run([sys.executable, str(ROOT/'tests/verify.py'), '--binary', str(binary), '--report', str(output)], capture_output=True)
        report = json.loads(output.read_text(encoding='utf-8'))
        assert run.returncode == 1 and report['cases_passed'] < report['cases_total'], label
        print(f"{label}: correctly rejected; {report['cases_total']-report['cases_passed']} failing public cases")
