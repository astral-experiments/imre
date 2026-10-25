"""Summarize a hosted shutil artifact after validating its complete paired reports."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import reference_cases, validate_pair


def report(directory, run):
    def read(name):
        return json.loads((directory / name).read_text())

    commit, tree = (directory / 'source.txt').read_text().splitlines()
    actual_tree = subprocess.check_output(['git', 'rev-parse', commit + '^{tree}'], text=True).strip()
    assert actual_tree == tree
    reference = read('reference-linux.json')
    evidence = read('evidence-linux.json')
    attempts = read('method-attempts-linux.json')
    validate_pair(evidence, reference)
    assert reference['denominator'] == 223 and attempts['compiler_tree'] == tree
    digests = dict(line.split()[::-1] for line in (directory / 'binaries.sha256').read_text().splitlines())
    assert digests['target/debug/imre'] == attempts['compiler_sha256']
    assert digests['target/debug/libimre_runtime.a'] == attempts['runtime_sha256']
    levels = read('misc-linux.json')['optimization_levels']
    summaries = {}
    for complete in evidence:
        level = str(complete['opt_level'])
        selected = levels[level]
        rows = {row['id']: row for row in complete['records'] if row['id'] in selected}
        assert set(rows) == set(selected) and len(rows) == 7
        summary = {}
        for name, row in rows.items():
            brief = {'classification': row['classification'], 'roles': row['roles']}
            if row.get('reason'):
                brief['reason'] = row['reason'].strip().splitlines()[-1]
            summary[name] = brief
        summaries[level] = {'all_223': complete['counts'], 'ownership_disk_terminal': summary}
    files = sorted(p for p in directory.iterdir() if p.is_file())
    return {
        'schema': 'imre.shutil.hosted-capacity/v1',
        'run': f'https://github.com/astral-sh/imre/actions/runs/{run}',
        'source_commit': commit, 'source_tree': tree,
        'cpython_commit': reference['cpython_commit'], 'platform': reference['platform'],
        'denominator': reference['denominator'],
        'reference': dict(Counter(r if isinstance(r, str) else next(iter(r))
                                  for r in reference_cases(reference).values())),
        'optimization_levels': summaries,
        'independent_controls': read('misc-controls.json'),
        **({'retained_args_control': read('os-args-control.json')}
           if (directory / 'os-args-control.json').is_file() else {}),
        'handler_controls': {p.name: p.read_text().splitlines() for p in files if p.suffix == '.log'},
        'artifact_files_sha256': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files},
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--run', type=int, required=True)
    parser.add_argument('--result', type=Path, required=True)
    args = parser.parse_args()
    args.result.write_text(json.dumps(report(args.directory, args.run), indent=2) + '\n')
