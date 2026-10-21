"""Replay dataclass memory-report evidence using archived sources, without engines.

Run beside the evidence archive, manifest and index, or pass --prefix. The replay
requires Python's standard library and never invokes native programs or Git.
"""
import argparse
import collections
from datetime import datetime
import gzip
import hashlib
import json
from pathlib import Path, PurePosixPath
import runpy
import statistics
import sys
import tarfile
import tempfile

if sys.flags.optimize:
    raise SystemExit('replay requires assertions; do not use -O')
sys.dont_write_bytecode = True
REVISION = 'f4aee96d255b4b4772e8d3fd2ba7169844cc58eb'
ENGINES = ('relar', 'port', 'original', 'rust')
AUDIT_HELPER = 'benchmarks/results/2026-10-21-memory-report-artifact-audit.py'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def safe(value):
    path = PurePosixPath(value)
    assert not path.is_absolute() and '..' not in path.parts and str(path) != '.', value
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, default=Path(__file__).with_name('2026-10-21-dataclass-records'))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    manifest = json.loads(Path(str(args.prefix) + '-evidence-manifest.json').read_bytes())
    index_bytes = Path(str(args.prefix) + '-evidence-index.json').read_bytes()
    assert manifest['status'] == 'passed' and manifest['measurement_revision'] == REVISION
    assert sha(index_bytes) == manifest['logical_index_sha256']
    assert sha(Path(__file__).read_bytes()) == manifest['replay_sha256']
    index = json.loads(index_bytes)
    assert index['schema'] == 1 and index['source_roles'] == {'measurement': REVISION}
    archive_path = Path(str(args.prefix) + '-evidence.tar.gz')
    archive_bytes = archive_path.read_bytes()
    assert len(archive_bytes) == manifest['archive']['bytes'] and sha(archive_bytes) == manifest['archive']['sha256']
    counts = collections.Counter()
    modes = {}
    with tarfile.open(archive_path, 'r:gz') as archive, tempfile.TemporaryDirectory(prefix='dataclass-replay-') as temporary:
        members = archive.getmembers()
        assert len(members) == len({member.name for member in members}) == manifest['member_count']
        assert {member.name for member in members} == set(manifest['members'])
        payloads = {}
        for member in members:
            safe(member.name)
            assert member.isfile() and not member.issym() and not member.islnk()
            data = archive.extractfile(member).read()
            expected = manifest['members'][member.name]
            assert len(data) == expected['bytes'] and sha(data) == expected['sha256']
            assert member.mode == expected['mode'] and member.mtime == member.uid == member.gid == 0
            payloads[member.name] = data
        assert payloads['provenance/logical-index.json'] == index_bytes

        def blob(item, expected=None):
            data = payloads[item['member']]
            assert len(data) == item['bytes'] and sha(data) == item['sha256']
            assert expected is None or sha(data) == expected
            assert item['member'] == 'blobs/sha256/' + sha(data)
            return data

        def retained(path, expected=None):
            return blob(index['files'][str(path)], expected)

        def load(path, expected=None):
            return json.loads(retained(path, expected))

        source = index['sources'][REVISION]
        for relative, item in source.items():
            safe(relative)
            data = blob(item)
            assert hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest() == item['git_blob']
        for path, item in index['files'].items():
            blob(item)
        for binding in index['captured_source_bindings']:
            assert binding['revision'] == REVISION
            assert all(binding[key] == source[binding['relative']][key] for key in ('sha256', 'bytes', 'git_blob', 'mode', 'member'))
        for original, entries in index['trees'].items():
            for relative, directory in entries.items():
                safe(relative)
                if not directory:
                    assert str(Path(original) / relative) in index['files'] or str(Path(original) / relative) in index['excluded']

        audit = load(manifest['audit']['path'], manifest['audit']['sha256'])
        assert audit['status'] == 'passed' and audit['measurement_revision'] == REVISION
        assert audit['audit_script_sha256'] == manifest['audit_script']['sha256']
        retained(manifest['audit_script']['path'], audit['audit_script_sha256'])
        records = {mode: load(item['path'], item['sha256']) for mode, item in manifest['timings'].items()}
        assert set(records) == {'normal', 'thinlto'}
        context = load('/tmp/relar-dataclass-timing-context.json', audit['timing_context_sha256'])
        dates = [context['started_at'], records['normal']['started_at_utc'], records['normal']['completed_at_utc'],
                 records['thinlto']['started_at_utc'], records['thinlto']['completed_at_utc'], context['completed_at']]
        assert list(map(datetime.fromisoformat, dates)) == sorted(map(datetime.fromisoformat, dates))
        root = Path(temporary) / 'source'
        final = records['normal']['preparation']
        original_root = Path(final['source_and_tool_paths']['helper:memory_benchmark.py']).parent.parent
        for path, expected in audit['guarded_inputs'].items():
            if Path(path).is_relative_to(original_root):
                blob(source[Path(path).relative_to(original_root).as_posix()], expected)
            elif path in index['excluded']:
                assert index['excluded'][path]['sha256'] == expected
            else:
                retained(path, expected)
        build = load(manifest['build']['path'], manifest['build']['sha256'])
        assert build['revision'] == REVISION and build['input_guards_passed'] and build['source_matches_revision']
        for relative, expected in build['source_sha256'].items():
            blob(source[relative], expected)
        required = {AUDIT_HELPER}
        required.update(Path(path).relative_to(original_root).as_posix()
                        for name, path in final['source_and_tool_paths'].items()
                        if name.startswith(('helper:', 'example:', 'stdlib:', 'fixture:', 'upstream:')))
        for relative in sorted(required):
            destination = root / safe(relative)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(blob(source[relative]))
        benchmark = runpy.run_path(str(root / 'benchmarks/memory_benchmark.py'))
        validator = benchmark['validator']
        helper = runpy.run_path(str(root / AUDIT_HELPER))
        for data in payloads.values():
            assert helper['native_kind'](data[:16]) is None
        empty = validator.case('empty-control', validator.report(), validator.report())
        workloads = [(benchmark['describe'](empty, 0, 'empty-schema-valid-reports'), empty), *benchmark['workloads']([32, 256])]
        application_cases = validator.cases()
        assert len(application_cases) == 72
        asan_metadata = manifest['asan_application']
        asan = load(asan_metadata['build']['path'], asan_metadata['build']['sha256'])
        assert asan['revision'] == REVISION and '--sanitize' in asan['compile_command']
        assert asan['environment'] == {'ASAN_OPTIONS': 'detect_leaks=1:abort_on_error=1'}
        assert index['excluded']['/tmp/relar-dataclass-memory-asan']['sha256'] == asan['binary_sha256']
        asan_validation = load(asan_metadata['validation']['path'], asan_metadata['validation']['sha256'])
        assert asan_validation['cases'] == 72 and asan_validation['engine_executions'] == 264
        assert set(asan_validation['engines']) == set(ENGINES)
        for retained_case, (case, error, original) in zip(asan_validation['results'], application_cases, strict=True):
            assert retained_case['case'] == validator.case_record(case) and retained_case['expected_error'] == error
            assert set(retained_case['engines']) == set(ENGINES) - (set() if original else {'original'})
            expected = validator.oracle(case, error)
            for engine, result in retained_case['engines'].items():
                helper['check_result'](validator, case, expected, {**result, '_engine': engine})
                counts['asan_application_executions'] += 1

        for mode, record in records.items():
            prep_item = manifest['preparations'][mode]
            prep = load(prep_item['path'], record['preparation_record_sha256'])
            assert record['preparation_record_sha256'] == prep_item['sha256'] and record['preparation'] == prep
            assert record['input_guards_passed'] and prep['input_guards_passed']
            assert record['revision'] == prep['revision'] == REVISION and not record['dirty'] and not prep['dirty']
            assert prep['thinlto'] == (mode == 'thinlto') and prep['effective_target'] == 'x86_64-unknown-linux-gnu'
            assert record['samples'] == 12 and record['warmups'] == 2 and record['copies'] == [32, 256]
            assert record['cpu']['affinity'] == [31]
            assert record['python_environment'] == {'PYTHONHASHSEED': '0', 'PYTHONPATH': '', 'PYTHONHOME': None}
            python = record['python']
            assert python['implementation'] == 'cpython' and python['version_number'] == '3.14.5'
            assert python['gil_enabled'] and not any(python[key] for key in ('free_threaded_build', 'jit_enabled', 'debug_build'))
            assert all(python['pgo'].values()) and python['lto']['lto_flag_present'] and python['lto']['configure_option'] == '--with-lto'
            for name, path in prep['source_and_tool_paths'].items():
                expected = prep['source_and_tool_sha256'][name]
                if Path(path).is_relative_to(original_root):
                    blob(source[Path(path).relative_to(original_root).as_posix()], expected)
                elif path in index['excluded']:
                    assert index['excluded'][path]['sha256'] == expected
                else:
                    retained(path, expected)
            previous = json.loads(gzip.decompress(blob(source[f'benchmarks/results/2026-10-21-memory-report-{mode}.json.gz'])))
            previous_hashes = previous['preparation']['source_and_tool_sha256']
            unchanged = [name for name in prep['source_and_tool_sha256']
                         if name.startswith(('helper:', 'fixture:', 'upstream:', 'rust_source:'))
                         or name in ('python', 'clang', 'rust', 'rust_build_record', 'lto_linker')]
            assert all(prep['source_and_tool_sha256'][name] == previous_hashes[name] for name in unchanged)
            hashes, paths = prep['source_and_tool_sha256'], prep['source_and_tool_paths']
            assert build['sha256']['compiler'] == hashes['compiler'] and build['sha256'][mode + '_runtime'] == hashes['runtime']
            rust = load(paths['rust_build_record'], hashes['rust_build_record'])
            assert rust == prep['rust_build_record'] and rust['binary_sha256'] == hashes['rust'] and rust['input_guards_passed']
            assert rust['thinlto'] == (mode == 'thinlto') and rust['llvm_budget'] == (1000 if mode == 'thinlto' else None)
            for relative, expected in rust['sources'].items():
                blob(source['benchmarks/reference/memory_report/' + relative], expected)
            for name in ('binary', 'llvm', 'validation'):
                path, expected = prep[name], prep[name + '_sha256']
                if path in index['excluded']:
                    assert index['excluded'][path]['sha256'] == expected
                else:
                    retained(path, expected)
            validation = load(prep['validation'], prep['validation_sha256'])
            assert validation['cases'] == 72 and validation['engine_executions'] == 264 and tuple(validation['engines']) == ENGINES
            for retained_case, (case, error, original) in zip(validation['results'], application_cases, strict=True):
                assert retained_case['case'] == validator.case_record(case) and retained_case['expected_error'] == error
                assert set(retained_case['engines']) == set(ENGINES) - (set() if original else {'original'})
                expected = validator.oracle(case, error)
                for engine, result in retained_case['engines'].items():
                    helper['check_result'](validator, case, expected, {**result, '_engine': engine})
                    counts['preparation_executions'] += 1
            commands = benchmark['commands_for'](prep, prep['source_and_tool_paths'])
            commands = {engine: [arg.replace(str(root), str(original_root)) for arg in command] for engine, command in commands.items()}
            orders = [list(order) for order in benchmark['engine_orders'](commands)]
            assert record['engine_orders'] == orders
            rows = [record['startup_control'], *record['workloads']]
            assert len(rows) == len(workloads) == 7
            artifact = Path(manifest['timings'][mode]['path']).with_suffix('')
            results = []
            for row, (description, case) in zip(rows, workloads, strict=True):
                assert {key: row[key] for key in description} == description
                assert set(row['gates']) == set(ENGINES)
                expected = validator.oracle(case)
                for engine, gate in row['gates'].items():
                    assert gate['command'] == [*commands[engine], *case.arguments] and gate['elapsed_ns'] > 0
                    helper['check_result'](validator, case, expected, {**gate, '_engine': engine})
                    directory = artifact / row['label'] / engine
                    names = {Path(path).relative_to(directory).as_posix() for path in index['files'] if Path(path).is_relative_to(directory)}
                    assert names == set(gate['files'])
                    for name, identity in gate['files'].items():
                        data = retained(directory / safe(name), identity['sha256'])
                        assert len(data) == identity['bytes']
                    actual_directories = sorted(str(Path(name).relative_to(Path(row['label']) / engine))
                                                for name, is_dir in index['trees'][str(artifact)].items()
                                                if is_dir and Path(name).is_relative_to(Path(row['label']) / engine)
                                                and Path(name) != Path(row['label']) / engine)
                    assert actual_directories == gate['directories']
                    counts['gates'] += 1
                for phase, length in (('warmups', 2), ('samples', 12)):
                    assert len(row[phase]) == length
                    for position, sample in enumerate(row[phase]):
                        assert set(sample) == {'index', 'order', *ENGINES}
                        assert sample['index'] == position and sample['order'] == orders[position % 4]
                        for engine in ENGINES:
                            assert type(sample[engine]['elapsed_ns']) is int and sample[engine]['elapsed_ns'] > 0
                            assert {key: value for key, value in sample[engine].items() if key != 'elapsed_ns'} == {
                                key: value for key, value in row['gates'][engine].items() if key != 'elapsed_ns'}
                            counts[phase] += 1
                positions = collections.Counter((engine, position) for sample in row['samples'] for position, engine in enumerate(sample['order']))
                neighbors = collections.Counter(pair for sample in row['samples'] for pair in zip(sample['order'], sample['order'][1:]))
                assert len(positions) == 16 and set(positions.values()) == {3}
                assert len(neighbors) == 12 and set(neighbors.values()) == {3}
                seconds = {}
                for engine in ENGINES:
                    values = [sample[engine]['elapsed_ns'] / 1e9 for sample in row['samples']]
                    q = statistics.quantiles(values, n=4, method='inclusive')
                    actual = {'median_seconds': statistics.median(values), 'min_seconds': min(values),
                              'max_seconds': max(values), 'p25_seconds': q[0], 'p75_seconds': q[2]}
                    assert actual == row['summary'][engine]
                    seconds[engine] = actual['median_seconds']
                for left, right in (('original', 'relar'), ('port', 'relar'), ('relar', 'rust'), ('original', 'rust'), ('port', 'rust')):
                    assert row[left + '_over_' + right] == seconds[left] / seconds[right]
                fractions = {engine: results[0]['seconds'][engine] / seconds[engine] for engine in ENGINES} if results else None
                if fractions is not None:
                    assert fractions == row['startup_control_fraction']
                results.append({'label': row['label'], 'records': row['records'], 'seconds': seconds,
                                'relar_over_rust': seconds['relar'] / seconds['rust'],
                                'original_over_relar': seconds['original'] / seconds['relar'],
                                'port_over_relar': seconds['port'] / seconds['relar'],
                                'startup_control_fraction': fractions})
            assert results == audit['modes'][mode]
            modes[mode] = results
            sidecar = Path(str(args.prefix) + '-' + mode + '.json.gz')
            if sidecar.exists():
                assert gzip.decompress(sidecar.read_bytes()) == retained(manifest['timings'][mode]['path'])
        targets = [row for rows in modes.values() for row in rows if row['label'].endswith('-256')]
        assert len(targets) == 4
        passed = all(row['relar_over_rust'] <= 2 for row in targets)
        assert passed == audit['all_four_large_cases_at_most_twice_rust']
        result = {'status': 'passed', 'measurement_revision': REVISION,
                  'scope': 'Archive integrity, Git blobs, captured source/tool identities, preparation output oracle, timing filesystem bytes, order balance and recomputed statistics; no engine execution.',
                  'counts': dict(counts), 'all_four_large_cases_at_most_twice_rust': passed, 'modes': modes}
        output = json.dumps(result, indent=2) + '\n'
        if args.output:
            args.output.write_text(output)
        print(output)


if __name__ == '__main__':
    main()
