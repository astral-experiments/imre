"""Record the first outcome of each complete-method probe, separately at O0 and O3."""
import argparse
from collections import Counter
import json
from pathlib import Path
import re
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import reference_cases, roles

SCHEMA = 'imre.pathlib.whole-method-probes/v1'
LEVELS = (0, 3)


def expected(reference):
    return {(name, nth) for (name, nth), outcome in reference_cases(reference).items() if outcome == 'pass'}


def collect(work, reference, commit):
    if not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ValueError('supply the full source commit used to build the native compiler and runtime')
    passes = expected(reference)
    records = {}
    counts = {}
    for level in LEVELS:
        result = json.loads((work / f'outcome-O{level}.json').read_bytes())
        if result['failures'] or result['skips'] or {(i, 1) for i in result['completed']} != passes:
            raise ValueError(f'incomplete original fixture lifecycles at O{level}')
        counts[f'O{level}'] = dict(sorted(Counter(r['status'] for r in result['completed'].values()).items()))
        for name, probe in result['completed'].items():
            if probe['id'] != name or probe['opt_level'] != level:
                raise ValueError((name, level, probe))
            identity = {k: probe[k] for k in ('instance', 'owner', 'source', 'upstream_line', 'method_sha256') if k in probe}
            row = records.setdefault(name, {'id': name, 'occurrence': 1, **identity, 'levels': {}})
            if any(row.get(k) != v for k, v in identity.items()):
                raise ValueError(f'original definition changed between builds: {name}')
            row['levels'][f'O{level}'] = {k: v for k, v in probe.items() if k not in identity and k not in ('id', 'opt_level')}
    return {'schema': SCHEMA, 'cpython_commit': reference['cpython_commit'], 'imre_commit': commit,
            'suite': reference['module'], 'platform': reference['platform'], 'denominator': len(reference_cases(reference)),
            'reference_passes_probed': len(passes), 'counts': counts, 'records': [records[i] for i in sorted(records)]}


def report_blockers(report, reference, level, adapters):
    """Use diagnosed blockers for remaining IDs. Only audited adapters earn pass credit."""
    if (report['schema'], report['cpython_commit'], report['suite'], report['platform'], report['denominator']) != (
            SCHEMA, reference['cpython_commit'], reference['module'], reference['platform'], len(reference_cases(reference))):
        raise ValueError('whole-method report does not match the pinned suite')
    records = {(r['id'], r['occurrence']): r for r in report['records']}
    if len(records) != len(report['records']) or records.keys() != expected(reference):
        raise ValueError('whole-method report lost or duplicated reference passes')
    blockers = {}
    for key, record in records.items():
        if key in adapters or key[0] in adapters:
            continue
        result = record['levels'][f'O{level}']
        status = result['status']
        failed = status in ('native failure', 'native timeout')
        reason = result.get('diagnostic') or result.get('stderr') or status
        if status == 'native candidate':
            reason = 'native method returned, but no adapter has verified its fixture and product operations'
        evidence = {'whole_method_probe': status, 'imre_commit': report['imre_commit'], 'opt_level': level}
        for detail in ('stage', 'failing_line', 'wrapper_line', 'decorators', 'fixture_limits',
                       'assertion_fixture', 'alternate_attempts', 'code', 'stdout'):
            if detail in result:
                evidence[detail] = result[detail]
        if 'source' in record:
            evidence['source'] = f"Lib/test/test_pathlib/{record['source']}:{result.get('failing_line', record['upstream_line'])}"
            evidence['method_sha256'] = record['method_sha256']
        blockers[key] = {'outcome': 'failure' if failed else 'blocked', 'reason': reason, 'evidence': evidence,
                         'roles': roles(body='imre' if failed else 'unobserved', assertions='unknown' if failed else 'unobserved',
                                        operation='unknown' if failed else 'unobserved', fixture='mixed' if failed else 'cpython')}
    return blockers


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--imre-commit', required=True)
    parser.add_argument('--result', type=Path, required=True)
    args = parser.parse_args()
    reference = json.loads((HERE / 'reference-linux.json').read_bytes())
    report = collect(args.work, reference, args.imre_commit)
    records = report.pop('records')
    with args.result.open('w', encoding='utf8') as stream:
        stream.write(json.dumps(report, indent=2)[:-2] + ',\n  "records": [\n')
        stream.write(',\n'.join('    ' + json.dumps(row, ensure_ascii=True) for row in records))
        stream.write('\n  ]\n}\n')
