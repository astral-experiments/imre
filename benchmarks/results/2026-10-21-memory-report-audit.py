"""Replay the published memory-report statistics and output oracle, without engines.

Run beside the evidence archive and index, or pass --prefix /path/to/2026-10-21-memory-report.
The native/tool hashes were checked before publication; executables are deliberately
excluded from the archive. This replay uses the exact archived Python oracle sources.
"""

import argparse
import collections
import hashlib
import json
from pathlib import Path
import runpy
import statistics
import subprocess
import sys
import tarfile
import tempfile

if sys.flags.optimize:
    raise SystemExit('audit requires assertions; do not use -O')

REVISION = '4fb67dcd36ae47c91be7e78fdc80cfd8b2f58b3a'
AUDIT_SHA = 'a6e1c7f4f9029ac007edbe9d8472e875b49ca16b0d516b4b89d34476ecef538e'
RECORDS = {
    ('baseline', 'normal'): ('/tmp/relar-memory-timing-normal-v2.json', 'd1fffd17767d83638254c2c88cd142a5d37d3cab18a2c2c74211e78dd30c64ad'),
    ('baseline', 'thinlto'): ('/tmp/relar-memory-timing-thinlto-v2.json', 'b0a18d19c2e49466c155023fb69ab4bd6622140574ec3b879cd1bf95c4d7638d'),
    ('final', 'normal'): ('/tmp/relar-memory-timing-normal-final-v1.json', 'dbdbb9b6bc2e5ff9ce7a714c3d78b290d7449d0ca7595d7e2514924915fe7686'),
    ('final', 'thinlto'): ('/tmp/relar-memory-timing-thinlto-final-v1.json', 'bfa7d1f8af6e87173e424853610f87ad9cc57991ea2baaa8bf743d576f21823a'),
}
ENGINES = ('relar', 'port', 'original', 'rust')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, default=Path(__file__).with_name('2026-10-21-memory-report'))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    index = json.loads(Path(str(args.prefix) + '-evidence-index.json').read_text())
    assert index['schema'] == 1
    counts = {'gates': 0, 'warmups': 0, 'measured': 0}
    recomputed = {}
    with tarfile.open(str(args.prefix) + '-evidence.tar.gz', 'r:gz') as archive, tempfile.TemporaryDirectory(prefix='memory-report-audit-') as temporary:
        root = Path(temporary) / 'source'

        def blob(item, expected=None):
            member = archive.getmember(item['member'])
            assert member.isfile(), item['member']
            data = archive.extractfile(member).read()
            assert len(data) == item['bytes'] and sha(data) == item['sha256']
            assert expected is None or sha(data) == expected
            return data

        def retained(path, expected=None):
            return blob(index['files'][str(path)], expected)

        records = {key: json.loads(retained(path, expected)) for key, (path, expected) in RECORDS.items()}
        audit = json.loads(retained('/tmp/relar-memory-final-audit.json', AUDIT_SHA))
        final = records['final', 'normal']['preparation']
        historical_root = Path(final['source_and_tool_paths']['helper:memory_benchmark.py']).parent.parent
        for name, path in final['source_and_tool_paths'].items():
            if not name.startswith(('helper:', 'example:', 'stdlib:', 'fixture:', 'upstream:')):
                continue
            relative = Path(path).relative_to(historical_root)
            assert '..' not in relative.parts
            data = blob(index['sources'][REVISION][relative.as_posix()], final['source_and_tool_sha256'][name])
            destination = root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
        benchmark = runpy.run_path(str(root / 'benchmarks/memory_benchmark.py'))
        validator = benchmark['validator']
        empty = validator.case('empty-control', validator.report(), validator.report())
        workloads = [(benchmark['describe'](empty, 0, 'empty-schema-valid-reports'), empty), *benchmark['workloads']([32, 256])]

        for (stage, mode), record in records.items():
            prep = record['preparation']
            sample_count = 4 if stage == 'baseline' else 12
            assert record['input_guards_passed'] and prep['input_guards_passed']
            assert record['samples'] == sample_count and record['warmups'] == 2 and record['copies'] == [32, 256]
            assert record['cpu']['affinity'] == [31]
            assert record['python_environment'] == {'PYTHONHASHSEED': '0', 'PYTHONPATH': '', 'PYTHONHOME': None}
            if stage == 'final':
                assert record['revision'] == prep['revision'] == REVISION and not record['dirty'] and not prep['dirty']
            for name in final['source_and_tool_paths']:
                if name.startswith(('helper:', 'fixture:', 'upstream:', 'stdlib:')) or name in ('python', 'clang'):
                    assert prep['source_and_tool_sha256'][name] == final['source_and_tool_sha256'][name]
            assert prep['source_and_tool_sha256']['rust'] == records['final', mode]['preparation']['source_and_tool_sha256']['rust']
            commands = benchmark['commands_for'](prep, prep['source_and_tool_paths'])
            commands = {engine: [arg.replace(str(root), str(historical_root)) for arg in command] for engine, command in commands.items()}
            orders = [list(order) for order in benchmark['engine_orders'](commands)]
            assert record['engine_orders'] == orders
            rows = [record['startup_control'], *record['workloads']]
            assert len(rows) == len(workloads) == 7
            artifact = Path(RECORDS[stage, mode][0]).with_suffix('')
            results = []
            for row, (metadata, case) in zip(rows, workloads, strict=True):
                assert {name: row[name] for name in metadata} == metadata
                expected = validator.oracle(case)
                for engine in ENGINES:
                    gate = row['gates'][engine]
                    assert gate['command'] == [*commands[engine], *case.arguments]
                    assert gate['elapsed_ns'] > 0
                    original_directory = artifact / row['label'] / engine
                    retained_names = {str(Path(path).relative_to(original_directory)) for path in index['files'] if Path(path).is_relative_to(original_directory)}
                    assert retained_names == set(gate['files'])
                    directory = Path(temporary) / stage / mode / row['label'] / engine
                    directory.mkdir(parents=True)
                    for name, identity in gate['files'].items():
                        relative = Path(name)
                        assert not relative.is_absolute() and '..' not in relative.parts
                        data = retained(artifact / row['label'] / engine / relative, identity['sha256'])
                        assert len(data) == identity['bytes']
                        destination = directory / relative
                        destination.parent.mkdir(parents=True, exist_ok=True)
                        destination.write_bytes(data)
                    assert gate['directories'] == []
                    process = subprocess.CompletedProcess(gate['command'], gate['returncode'], gate['stdout'].encode(), gate['stderr'].encode())
                    checked = validator.check_result(case, expected, process, directory, engine)
                    assert checked == {key: value for key, value in gate.items() if key not in ('command', 'elapsed_ns')}
                for phase, length in (('warmups', 2), ('samples', sample_count)):
                    assert len(row[phase]) == length
                    for position, sample in enumerate(row[phase]):
                        assert sample['index'] == position and sample['order'] == orders[position % 4]
                        for engine in ENGINES:
                            assert sample[engine]['elapsed_ns'] > 0
                            assert {k: v for k, v in sample[engine].items() if k != 'elapsed_ns'} == {k: v for k, v in row['gates'][engine].items() if k != 'elapsed_ns'}
                positions = collections.Counter((engine, position) for sample in row['samples'] for position, engine in enumerate(sample['order']))
                neighbors = collections.Counter(pair for sample in row['samples'] for pair in zip(sample['order'], sample['order'][1:]))
                assert len(positions) == 16 and set(positions.values()) == {sample_count // 4}
                assert len(neighbors) == 12 and set(neighbors.values()) == {sample_count // 4}
                medians = {}
                for engine in ENGINES:
                    times = [sample[engine]['elapsed_ns'] / 1e9 for sample in row['samples']]
                    q = statistics.quantiles(times, n=4, method='inclusive')
                    values = {'median_seconds': statistics.median(times), 'min_seconds': min(times), 'max_seconds': max(times), 'p25_seconds': q[0], 'p75_seconds': q[2]}
                    assert values == row['summary'][engine]
                    medians[engine] = values['median_seconds']
                for left, right in (('original', 'relar'), ('port', 'relar'), ('relar', 'rust'), ('original', 'rust'), ('port', 'rust')):
                    assert row[f'{left}_over_{right}'] == medians[left] / medians[right]
                if row is not record['startup_control']:
                    fractions = {engine: record['startup_control']['summary'][engine]['median_seconds'] / medians[engine] for engine in ENGINES}
                    assert row['startup_control_fraction'] == fractions
                    results.append({'label': row['label'], 'median_milliseconds': {key: value * 1000 for key, value in medians.items()}, 'relar_over_rust': medians['relar'] / medians['rust']})
                counts['gates'] += 4
                counts['warmups'] += 8
                counts['measured'] += sample_count * 4
            recomputed[stage + '-' + mode] = results
            if stage == 'final':
                for actual, claimed in zip(results, audit['modes'][mode]['workloads'], strict=True):
                    assert all(actual[key] == claimed[key] for key in actual)
        targets = [row for key, rows in recomputed.items() if key.startswith('final-') for row in rows if row['label'].endswith('-256')]
        assert len(targets) == 4
        result = {'status': 'passed', 'scope': 'Archived sources, complete output oracle, filesystem bytes, order balance and statistics; no engine execution.',
                  'counts': counts, 'all_four_large_cases_at_most_twice_rust': all(row['relar_over_rust'] <= 2 for row in targets), 'records': recomputed}
        output = json.dumps(result, indent=2) + '\n'
        if args.output:
            args.output.write_text(output)
        print(output)


if __name__ == '__main__':
    main()
