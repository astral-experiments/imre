"""Independently check memory-report captures, raw profiles, report and archive.

This performs data-only replay using Git-bound oracle sources. It never executes
measured engines. Native artifacts are rehashed locally; portable oracle replay
is supplied separately with the publication.
"""
import argparse
import collections
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import re
import runpy
import statistics
import struct
import subprocess
import sys
import tarfile
import tempfile
from datetime import datetime

if sys.flags.optimize:
    raise SystemExit('audit requires assertions; do not use -O')
sys.dont_write_bytecode = True
ROOT = Path('/home/dev-user/code/oss/relar')
FINAL = '4fb67dcd36ae47c91be7e78fdc80cfd8b2f58b3a'
BASE = '73ec04ba338bc0ce6dba7b7070d22c6fce0f2a6b'
LIBRARY = {'baseline': 'c28b93960b40ffeb51402ce02ec6cbf0d80dcd3b', 'final': '7119fb22d97f7057e662d1735700256ff0c853be'}
ENGINES = ('relar', 'port', 'original', 'rust')
FIELDS = ('tb', 'tbk', 'gb', 'gbk', 'eb', 'ebk', 'rb', 'wb')
INPUTS = {}
GIT = {}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def guard(path, expected=None):
    path = Path(path).absolute()
    with path.open('rb') as stream:
        actual = hashlib.file_digest(stream, 'sha256').hexdigest()
    assert actual == INPUTS.setdefault(str(path), actual), ('input changed', path)
    assert expected is None or actual == expected, ('hash differs', path)
    return actual


def raw(path, expected=None):
    path = Path(path).absolute()
    data = path.read_bytes()
    actual = sha(data)
    assert actual == INPUTS.setdefault(str(path), actual), ('input changed', path)
    assert expected is None or actual == expected, ('hash differs', path)
    return data


def load(path, expected=None):
    return json.loads(raw(path, expected))


def git(*arguments):
    return subprocess.check_output(['git', '-C', str(ROOT), *arguments])


def source(revision, path):
    key = (revision, str(path))
    if key not in GIT:
        GIT[key] = git('show', revision + ':' + str(path))
    return GIT[key]


def captured(path, expected, revision):
    path = Path(path).absolute()
    if path.is_relative_to(ROOT):
        assert sha(source(revision, path.relative_to(ROOT))) == expected, ('Git capture mismatch', revision, path)
    else:
        guard(path, expected)


def encoded(files):
    return {name: {'sha256': sha(data), 'bytes': len(data)} for name, data in sorted(files.items())}


def check_result(validator, case, expected, result, directory=None):
    stdout, stderr = result['stdout'], result['stderr']
    if expected['error']:
        assert result['returncode'] != 0 and stdout == ''
        category = {'OSError': '(?:OSError|FileNotFoundError|IsADirectoryError|NotADirectoryError)', 'ValueError': '(?:ValueError|UnicodeDecodeError)'}.get(expected['error'], expected['error'])
        assert re.search(category + ':', stderr)
        files = expected['files']
    else:
        assert result['returncode'] == 0
        if result['_engine'] == 'original':
            if expected['output'] is None:
                assert stderr == '' and stdout.endswith('\n')
                rendered = stdout[:-1]
            else:
                prefix = f"Output written to {expected['output']}\n"
                assert stdout == '' and stderr.startswith(prefix) and stderr.endswith('\n')
                rendered = stderr[len(prefix):-1]
            validator.equivalent_original(rendered, expected)
        else:
            rendered = expected['rendered']
            assert stderr == '' and stdout == (rendered + '\n' if expected['output'] is None else '')
        files = validator.apply_output(case, expected, rendered)
    assert result['files'] == encoded(files) and result['directories'] == expected['directories']
    if directory is not None:
        actual = {}
        directories = []
        for path in sorted(directory.rglob('*')):
            assert not path.is_symlink(), path
            if path.is_dir():
                directories.append(path.relative_to(directory).as_posix())
            else:
                actual[path.relative_to(directory).as_posix()] = raw(path)
        assert actual == files and directories == expected['directories'], ('retained filesystem', directory)


def independent_callgrind(data):
    names, own, calls, edge_cost, edge_calls = {}, collections.Counter(), collections.Counter(), collections.Counter(), collections.Counter()
    current = target = None
    pending = None
    positions = None
    summary = None
    for line in data.decode().splitlines():
        if line.startswith('positions:'):
            positions = len(line.split()) - 1
        elif line.startswith('events:'):
            assert line == 'events: Ir'
        elif line.startswith('summary:'):
            summary = int(line.split()[1])
        elif line.startswith(('fn=', 'cfn=')):
            match = re.fullmatch(r'(c?fn)=\((\d+)\)(?: (.*))?', line)
            assert match, line
            identifier = int(match[2])
            if match[3] is not None:
                assert names.setdefault(identifier, match[3]) == match[3]
            if match[1] == 'fn':
                current = identifier
            else:
                target = identifier
        elif line.startswith('calls='):
            pending = int(line[6:].split()[0])
        elif line and line[0] in '0123456789+-*':
            values = line.split()
            assert positions is not None and len(values) in (positions, positions + 1), line
            cost = int(values[positions]) if len(values) > positions else 0
            if pending is None:
                own[current] += cost
            else:
                calls[target] += pending
                edge_calls[current, target] += pending
                edge_cost[current, target] += cost
                pending = None
    assert pending is None and summary == sum(own.values())
    functions = {}
    for identifier in set(own) | set(calls):
        name = names[identifier]
        item = functions.setdefault(name, {'self_instructions': 0, 'calls': 0})
        item['self_instructions'] += own[identifier]
        item['calls'] += calls[identifier]
    edges = collections.Counter()
    for (caller, callee), cost in edge_cost.items():
        edges[names[caller], names[callee]] += cost
    return {'total': summary, 'functions': functions, 'edges': edges}


def native_kind(data):
    if data.startswith(b'\x7fELF'): return 'ELF executable, object, or shared library'
    if data.startswith((b'!<arch>\n', b'!<thin>\n')): return 'static archive'
    if data.startswith((b'BC\xc0\xde', b'\xde\xc0\x17\x0b')): return 'LLVM bitcode'
    if data[:4] in {bytes.fromhex(value) for value in ('feedface','cefaedfe','feedfacf','cffaedfe','cafebabe','bebafeca','cafebabf','bfbafeca')}: return 'Mach-O native code or Java bytecode'
    if data.startswith(b'MZ'): return 'PE executable or object'
    if data.startswith(b'\0asm'): return 'WebAssembly bytecode'
    if data[:4] == importlib.util.MAGIC_NUMBER or data[2:4] == b'\r\n' and (3000 <= int.from_bytes(data[:2], 'little') < 4000 or int.from_bytes(data[:2], 'little') == 62211): return 'Python bytecode'
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, default=Path('/tmp/2026-10-21-memory-report'))
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--skip-archive', action='store_true')
    args = parser.parse_args()
    guard(__file__)
    records, preps, summary = {}, {}, {}
    counts = collections.Counter()
    with tempfile.TemporaryDirectory(prefix='memory-independent-audit-') as temporary:
        oracle_root = Path(temporary) / 'source'
        final_prep = load('/tmp/relar-memory-prepared-normal-final-v1.json')
        for name, path in final_prep['source_and_tool_paths'].items():
            if name.startswith(('helper:', 'example:', 'stdlib:', 'fixture:', 'upstream:')):
                relative = Path(path).relative_to(ROOT)
                data = source(FINAL, relative)
                assert sha(data) == final_prep['source_and_tool_sha256'][name]
                destination = oracle_root / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(data)
        benchmark = runpy.run_path(str(oracle_root / 'benchmarks/memory_benchmark.py'))
        validator = benchmark['validator']
        empty = validator.case('empty-control', validator.report(), validator.report())
        workloads = [(benchmark['describe'](empty, 0, 'empty-schema-valid-reports'), empty), *benchmark['workloads']([32, 256])]
        application_cases = validator.cases()
        assert len(application_cases) == 72
        for stage in ('baseline', 'final'):
            revision = BASE if stage == 'baseline' else FINAL
            suffix = 'v2' if stage == 'baseline' else 'final-v1'
            sample_count = 4 if stage == 'baseline' else 12
            for mode in ('normal', 'thinlto'):
                path = Path(f'/tmp/relar-memory-timing-{mode}-{suffix}.json')
                record = load(path)
                prep = load(f'/tmp/relar-memory-prepared-{mode}-{suffix}.json', record['preparation_record_sha256'])
                assert record['preparation'] == prep
                records[stage, mode], preps[stage, mode] = record, prep
                assert prep['input_guards_passed'] and record['input_guards_passed']
                assert prep['revision'] == record['revision'] == (LIBRARY['baseline'] if stage == 'baseline' else FINAL)
                assert prep['dirty'] == record['dirty'] == (stage == 'baseline')
                assert prep['thinlto'] == (mode == 'thinlto') and prep['effective_target'] == 'x86_64-unknown-linux-gnu'
                assert record['samples'] == sample_count and record['warmups'] == 2 and record['copies'] == [32, 256]
                assert record['cpu']['affinity'] == [31]
                assert record['cpu']['model'] == 'AMD EPYC-Milan Processor'
                python = record['python']
                assert python['implementation'] == 'cpython' and python['version_number'] == '3.14.5'
                assert python['gil_enabled'] and not python['free_threaded_build'] and not python['jit_enabled'] and not python['debug_build']
                assert all(python['pgo'].values()) and python['lto']['lto_flag_present'] and python['lto']['configure_option'] == '--with-lto'
                assert record['python_environment'] == {'PYTHONHASHSEED': '0', 'PYTHONPATH': '', 'PYTHONHOME': None}
                paths, hashes = prep['source_and_tool_paths'], prep['source_and_tool_sha256']
                assert paths.keys() == hashes.keys()
                for name, captured_path in paths.items(): captured(captured_path, hashes[name], revision)
                for name in ('binary', 'llvm', 'validation'): guard(prep[name], prep[name + '_sha256'])
                expected_command = [paths['compiler'], 'build', str(ROOT / 'examples/memory_report/__main__.py'), '--module-root', str(ROOT / 'examples'), '-o', prep['binary'], '--runtime', paths['runtime'], '--clang', paths['clang'], '--opt-level', '3', '--emit-llvm', prep['llvm'], '--target', prep['effective_target']]
                if mode == 'thinlto': expected_command += ['--lto-linker', paths['lto_linker']]
                assert prep['compile_command'] == expected_command
                for binary in (prep['binary'], paths['rust']):
                    with Path(binary).open('rb') as stream: header = stream.read(20)
                    assert header[:6] == b'\x7fELF\x02\x01' and struct.unpack('<H', header[18:20])[0] == 62
                assert prep['native_elf'] == {'bits': 64, 'byte_order': 'little', 'machine': 62}
                build = load(paths['relar_build_record'])
                assert build['revision'] == LIBRARY[stage] and build['source_matches_revision'] and build['input_guards_passed']
                assert build['sha256']['compiler'] == hashes['compiler'] and build['sha256'][mode + '_runtime'] == hashes['runtime']
                for relative, digest in build['source_sha256'].items(): assert sha(source(build['revision'], relative)) == digest
                rust = load(paths['rust_build_record'])
                assert rust == prep['rust_build_record'] and rust['binary_sha256'] == hashes['rust'] and rust['input_guards_passed']
                assert rust['thinlto'] == (mode == 'thinlto') and rust['llvm_budget'] == (1000 if mode == 'thinlto' else None)
                for key, value in {'CARGO_PROFILE_RELEASE_OPT_LEVEL': '3', 'CARGO_PROFILE_RELEASE_LTO': 'thin' if mode == 'thinlto' else 'off', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS': '16'}.items(): assert rust['environment'][key] == value
                for relative, digest in rust['sources'].items(): assert sha(source(revision, 'benchmarks/reference/memory_report/' + relative)) == digest
                validation = load(prep['validation'])
                assert validation['cases'] == prep['validation_cases'] == 72 and validation['engine_executions'] == 264
                assert tuple(validation['engines']) == ENGINES and len(validation['results']) == len(application_cases)
                for retained, (case, error, original) in zip(validation['results'], application_cases, strict=True):
                    assert retained['case'] == validator.case_record(case) and retained['expected_error'] == error
                    assert set(retained['engines']) == set(ENGINES) - (set() if original else {'original'})
                    expected = validator.oracle(case, error)
                    for engine, result in retained['engines'].items():
                        check_result(validator, case, expected, {**result, '_engine': engine})
                        counts['preparation_executions'] += 1
                commands = benchmark['commands_for'](prep, paths)
                commands = {engine: [arg.replace(str(oracle_root), str(ROOT)) for arg in command] for engine, command in commands.items()}
                orders = [list(order) for order in benchmark['engine_orders'](commands)]
                assert record['engine_orders'] == orders
                rows = [record['startup_control'], *record['workloads']]
                assert len(rows) == len(workloads) == 7
                directory = path.with_suffix('')
                assert {entry.name for entry in directory.iterdir()} == {row['label'] for row in rows}
                computed = []
                for row, (metadata, case) in zip(rows, workloads, strict=True):
                    assert {key: row[key] for key in metadata} == metadata
                    assert set(row['gates']) == set(ENGINES)
                    assert {entry.name for entry in (directory / row['label']).iterdir()} == set(ENGINES)
                    expected = validator.oracle(case)
                    for engine, gate in row['gates'].items():
                        assert gate['command'] == [*commands[engine], *case.arguments] and gate['elapsed_ns'] > 0
                        check_result(validator, case, expected, {**gate, '_engine': engine}, directory / row['label'] / engine)
                    for phase, count in (('warmups', 2), ('samples', sample_count)):
                        assert len(row[phase]) == count
                        for position, run in enumerate(row[phase]):
                            assert set(run) == {'index', 'order', *ENGINES} and run['index'] == position and run['order'] == orders[position % 4]
                            for engine in ENGINES:
                                assert run[engine]['elapsed_ns'] > 0
                                assert {k: v for k, v in run[engine].items() if k != 'elapsed_ns'} == {k: v for k, v in row['gates'][engine].items() if k != 'elapsed_ns'}
                    positions = collections.Counter((engine, position) for run in row['samples'] for position, engine in enumerate(run['order']))
                    neighbors = collections.Counter(pair for run in row['samples'] for pair in zip(run['order'], run['order'][1:]))
                    assert len(positions) == 16 and set(positions.values()) == {sample_count // 4}
                    assert len(neighbors) == 12 and set(neighbors.values()) == {sample_count // 4}
                    medians = {}
                    for engine in ENGINES:
                        values = [run[engine]['elapsed_ns'] / 1e9 for run in row['samples']]
                        q = statistics.quantiles(values, n=4, method='inclusive')
                        statistics_row = {'median_seconds': statistics.median(values), 'min_seconds': min(values), 'max_seconds': max(values), 'p25_seconds': q[0], 'p75_seconds': q[2]}
                        assert statistics_row == row['summary'][engine]
                        medians[engine] = statistics_row['median_seconds']
                    for left, right in (('original', 'relar'), ('port', 'relar'), ('relar', 'rust'), ('original', 'rust'), ('port', 'rust')):
                        assert row[left + '_over_' + right] == medians[left] / medians[right]
                    if computed:
                        fractions = {engine: computed[0]['seconds'][engine] / medians[engine] for engine in ENGINES}
                        assert fractions == row['startup_control_fraction']
                    else: fractions = None
                    computed.append({'label': row['label'], 'seconds': medians, 'relar_over_rust': medians['relar'] / medians['rust'], 'control_fraction': fractions})
                    counts[stage + '_gates'] += 4
                    counts[stage + '_warmups'] += 8
                    counts[stage + '_measured'] += sample_count * 4
                summary[stage + '-' + mode] = computed
        for mode in ('normal', 'thinlto'):
            before, after = (preps[stage, mode]['source_and_tool_sha256'] for stage in ('baseline', 'final'))
            for name in before:
                if name.startswith(('helper:', 'stdlib:', 'fixture:', 'upstream:', 'rust_source:')) or name in ('python', 'clang', 'rust', 'rust_build_record'):
                    assert before[name] == after[name], ('comparison context changed', name)
        profiles = audit_profiles(validator, benchmark, preps, records)
        report_checks = audit_report(args.prefix, summary, profiles)
        archive = None if args.skip_archive else audit_archive(args.prefix)
    for path, expected in list(INPUTS.items()): guard(path, expected)
    targets = [row for key, rows in summary.items() if key.startswith('final-') for row in rows if row['label'].endswith('-256')]
    assert len(targets) == 4 and all(row['relar_over_rust'] <= 2 for row in targets)
    result = {'schema': 1, 'status': 'passed', 'scope': 'Independent raw timing statistics/order/output replay, Git/build/preparation identities, raw Callgrind/DHAT recomputation, report claims' + (', and full archive/index membership.' if archive else '; archive not yet audited.'), 'measurement_revision': FINAL, 'counts': dict(counts), 'timings': summary, 'profiles': profiles, 'report': report_checks, 'archive': archive, 'all_four_large_cases_at_most_twice_rust': True, 'audit_script_sha256': INPUTS[str(Path(__file__).absolute())], 'guarded_inputs': INPUTS, 'limits': 'No engines executed. Historical process isolation is not reconstructed. Preparation records retain complete stream values and file hashes; final timing/profile files are independently read. Oracle logic comes from the separately reviewed exact Git-bound benchmark validator.'}
    with args.output.open('x') as stream: json.dump(result, stream, indent=2); stream.write('\n')
    print(json.dumps({'output': str(args.output), 'sha256': guard(args.output), 'counts': result['counts'], 'archive': archive, 'targets': [{key: row[key] for key in ('label', 'relar_over_rust')} for row in targets]}, indent=2))


def audit_profiles(validator, benchmark, preps, records):
    runs, bindings, provenance = {}, {}, {}
    expected_keys = {(workload, engine, tool) for workload in ('scaled-full-32', 'scaled-unchanged-32') for engine in ('relar', 'rust') for tool in ('callgrind', 'dhat')}
    workloads = {metadata['label']: (metadata, case) for metadata, case in benchmark['workloads']([32])}
    for stage, suffix in (('baseline', 'baseline-v2'), ('final', 'optimized-v1')):
        directory = Path('/tmp/relar-memory-profile-' + suffix)
        manifest = load(directory / 'manifest.json')
        actual = {path.relative_to(directory).as_posix() for path in directory.rglob('*') if path.is_file() and path.name != 'manifest.json'}
        assert actual == set(manifest)
        for relative, identity in manifest.items():
            assert (directory / relative).stat().st_size == identity['bytes']
            guard(directory / relative, identity['sha256'])
        prov, captured_summary = load(directory / 'provenance.json'), load(directory / 'summary.json')
        provenance[stage] = prov
        prep = preps[stage, 'thinlto']
        assert prov['status'] == 'passed' and prov['input_guards_passed'] and prov['thinlto'] and prov['cpu'] == 0
        assert prov['compile_command'] == prep['compile_command'] and prov['effective_target'] == prep['effective_target']
        assert prov['revision'] == prep['revision'] and prov['dirty'] == prep['dirty']
        for identity in prov['inputs'].values(): captured(identity['path'], identity['sha256'], BASE if stage == 'baseline' else FINAL)
        assert prov['inputs']['binary']['sha256'] == prep['binary_sha256']
        assert prov['inputs']['llvm']['sha256'] == prep['llvm_sha256']
        assert prov['inputs']['record']['sha256'] == guard('/tmp/relar-memory-prepared-thinlto-' + ('v2' if stage == 'baseline' else 'final-v1') + '.json')
        assert len(prov['runs']) == len(captured_summary['runs']) == 8
        claimed = {(run['workload'], run['engine'], run['tool']): run for run in captured_summary['runs']}
        assert set(claimed) == expected_keys
        seen = set()
        for run in prov['runs']:
            key = run['workload'], run['engine'], run['tool']
            assert key not in seen and key in expected_keys
            seen.add(key)
            assert run['returncode'] == 0 and not run['timed_out'] and run['complete_output_verified']
            metadata, case = workloads[run['workload']]
            expected = validator.oracle(case)
            check_result(validator, case, expected, {**run['result'], '_engine': run['engine']}, directory / run['cwd'])
            assert run['stdout_bytes'] == run['stderr_bytes'] == 0
            assert run['stdout_sha256'] == run['stderr_sha256'] == sha(b'')
            stem = '.'.join(key)
            for stream in ('stdout', 'stderr'):
                assert raw(directory / (stem + '.' + stream), run[stream + '_sha256']) == run['result'][stream].encode()
            binary = prep['binary'] if run['engine'] == 'relar' else prep['source_and_tool_paths']['rust']
            identity = prep['binary_sha256'] if run['engine'] == 'relar' else prep['source_and_tool_sha256']['rust']
            assert run['binary_sha256'] == identity and run['command'][-len(case.arguments)-1:] == [binary, *case.arguments]
            assert run['command'][0] == prov['inputs']['profiler:valgrind']['path']
            assert '--tool=' + run['tool'] in run['command']
            trace = raw(directory / run['trace'], run['trace_sha256'])
            assert run['trace_sha256'] == claimed[key]['trace_sha256']
            guard(directory / run['log'], run['log_sha256'])
            if run['tool'] == 'callgrind':
                parsed = independent_callgrind(trace)
                assert parsed['total'] == claimed[key]['total_instructions']
                for function in claimed[key]['functions']:
                    assert parsed['functions'][function['name']] == {name: function[name] for name in ('self_instructions', 'calls')}
                for edge in claimed[key]['edges']:
                    assert parsed['edges'][edge['caller'], edge['callee']] == edge['inclusive_instructions']
            else:
                data = json.loads(trace)
                assert data['dhatFileVersion'] == 2 and data['mode'] == 'heap'
                totals = {field: sum(point.get(field, 0) for point in data['pps']) for field in FIELDS}
                assert totals == claimed[key]['totals']
                points = [{'id': index, **{field: point.get(field, 0) for field in FIELDS}, 'stack': [data['ftbl'][frame] for frame in point['fs']]} for index, point in enumerate(data['pps'])]
                for point in claimed[key]['points']:
                    assert all(point[field] == points[point['id']][field] for field in ('stack', *FIELDS))
                parsed = {'totals': totals, 'points': points}
            runs[stage, *key] = parsed
        assert seen == expected_keys
        interval = datetime.fromisoformat(prov['started_at_utc']), datetime.fromisoformat(prov['completed_at_utc'])
        for record in records.values():
            timing = datetime.fromisoformat(record['started_at_utc']), datetime.fromisoformat(record['completed_at_utc'])
            assert interval[1] <= timing[0] or timing[1] <= interval[0], 'profiling overlaps wall measurements'
        bindings[stage] = {name + '_sha256': guard(directory / (name + '.json')) for name in ('manifest', 'summary', 'provenance')}
    assert provenance['baseline']['workloads'] == provenance['final']['workloads']
    comparison = load('/tmp/relar-memory-profile-comparison.json')
    for stage, label in (('baseline', 'baseline'), ('final', 'optimized')):
        assert all(comparison[label][key] == value for key, value in bindings[stage].items())
    rows = []
    for workload in ('scaled-full-32', 'scaled-unchanged-32'):
        for engine in ('relar', 'rust'):
            before, after = (runs[stage, workload, engine, 'callgrind']['total'] for stage in ('baseline', 'final'))
            old, new = (runs[stage, workload, engine, 'dhat']['totals'] for stage in ('baseline', 'final'))
            row = {'workload': workload, 'engine': engine, 'baseline_instructions': before, 'optimized_instructions': after, 'instruction_reduction_fraction': (before-after)/before, 'baseline_heap': old, 'optimized_heap': new, 'allocation_delta': {field: new[field]-old[field] for field in ('tb','tbk','gb','gbk')}}
            assert row in comparison['rows']
            rows.append(row)
    details = load('/tmp/relar-memory-profile-details.json')
    allocation = []
    for workload in ('scaled-full-32', 'scaled-unchanged-32'):
        old, new = (runs[stage, workload, 'relar', 'dhat']['points'] for stage in ('baseline', 'final'))
        assert len(old) == len(new)
        changes = [index for index, (left, right) in enumerate(zip(old, new, strict=True)) if any(left[field] != right[field] for field in ('tb','tbk'))]
        assert changes == [4]
        assert old[4]['tb'] == 51 and new[4]['tb'] == 57 and old[4]['tbk'] == new[4]['tbk'] == 1
        assert all(any('relar_sys_argv' in frame for frame in point['stack']) for point in (old[4],new[4]))
        assert len(preps['final','thinlto']['binary'].encode()) - len(preps['baseline','thinlto']['binary'].encode()) == 6
        claim = next(item for item in details['argv_allocation_explanation'] if item['workload'] == workload)
        assert claim['baseline_stack'] == old[4]['stack'] and claim['optimized_stack'] == new[4]['stack']
        allocation.append({'workload': workload, 'changed_point': 4, 'byte_delta': 6, 'block_delta': 0, 'all_other_allocation_bytes_and_blocks_equal': True})
    lookup, decoder = {}, {}
    name = '<relar_runtime::objects::mappings::Tables<usize>>::lookup'
    for stage, label in (('baseline','baseline'),('final','optimized')):
        parsed = runs[stage,'scaled-full-32','relar','callgrind']
        lookup[label] = parsed['functions'][name]
        assert all(details['dictionary_lookup'][label][key] == value for key,value in lookup[label].items())
        entry = parsed['functions']['relar_json_decode']['self_instructions']
        search = sum(value for (caller,callee),value in parsed['edges'].items() if caller == 'relar_json_decode' and 'memchr' in callee)
        decoder[label] = entry + search
        assert decoder[label] == details['decoder_entry_work'][label]['entry_plus_search_instructions']
    final_functions = runs['final','scaled-full-32','relar','callgrind']['functions']
    for item in details['remaining_top_functions']:
        assert final_functions[item['name']]['self_instructions'] == item['self_instructions']
    points = runs['final','scaled-full-32','relar','dhat']['points']
    attributed = []
    for point in points:
        names = [match[1] for frame in point['stack'] if (match := re.search(r'\b(relar_(?!runtime\b)[A-Za-z0-9_]+)\b', frame))]
        if names and names[0] == 'relar_json_decode': attributed.append(point)
    decoder_allocations = {field: sum(point[field] for point in attributed) for field in FIELDS}
    assert all(details['remaining_decoder_allocations'][field] == value for field,value in decoder_allocations.items())
    return {'runs': 16, 'rows': rows, 'bindings': bindings, 'argv_allocation': allocation, 'lookup': lookup, 'decoder_entry_instructions': decoder, 'decoder_allocations': decoder_allocations, 'top_functions': details['remaining_top_functions'][:2]}


def audit_report(prefix, summaries, profiles):
    report_path = prefix.with_suffix('.md')
    report = raw(report_path).decode()
    render_path = Path(str(prefix) + '-render.json')
    render = load(render_path)
    assert render['output'] == str(report_path) and render['output_sha256'] == sha(report.encode())
    assert '/tmp/relar-memory-render-report.py' in render['inputs']
    for path, expected in render['inputs'].items(): guard(path, expected)
    expected_rows = []
    large, small = [], []
    for mode, title in (('normal','Ordinary'),('thinlto','ThinLTO')):
        rows = summaries['final-' + mode]
        for row in rows[1:]:
            seconds = row['seconds']; values = [seconds[engine]*1000 for engine in ('original','port','relar','rust')]
            expected_rows.append(f"| {title} | `{row['label']}` | " + ' | '.join(f'{value:.3f}' for value in values) + f" | {row['relar_over_rust']:.3f}× |")
            if row['label'].endswith('-256'):
                large.append(row)
                case = 'Full comparison' if '-full-' in row['label'] else 'Unchanged report'
                expected_rows.append(f"| {title} | {case} | " + ' | '.join(f'{value:.3f}' for value in values) + f" | {row['relar_over_rust']:.3f}× | {row['control_fraction']['relar']:.2%} |")
                old = next(item for item in summaries['baseline-' + mode] if item['label'] == row['label'])['seconds']['relar']
                expected_rows.append(f"| {title} | `{row['label']}` | {old*1000:.3f} | {seconds['relar']*1000:.3f} | {1-seconds['relar']/old:.2%} |")
            if row['label'].startswith('real-'): small.extend(row['control_fraction'][engine] for engine in ('relar','rust'))
        expected_rows.append(f'| {title} | ' + ' | '.join(f"{rows[0]['seconds'][engine]*1000:.3f}" for engine in ('original','port','relar','rust')) + ' |')
    for workload in ('scaled-full-32','scaled-unchanged-32'):
        relar = next(row for row in profiles['rows'] if row['workload'] == workload and row['engine'] == 'relar')
        rust = next(row for row in profiles['rows'] if row['workload'] == workload and row['engine'] == 'rust')
        expected_rows.append(f"| `{workload}` | {relar['baseline_instructions']:,} → {relar['optimized_instructions']:,} | {relar['instruction_reduction_fraction']:.2%} | {rust['optimized_instructions']:,} | {relar['optimized_heap']['tbk']:,} / {rust['optimized_heap']['tbk']:,} |")
    for stage in ('baseline','final'):
        for mode in ('normal','thinlto'):
            path = f'/tmp/relar-memory-timing-{mode}-' + ('v2' if stage == 'baseline' else 'final-v1') + '.json'
            suffix = ('baseline-' if stage == 'baseline' else '') + mode
            expected_rows.append(f'| [{stage} {mode}]({prefix.name}-{suffix}.json.gz) | `{guard(path)}` |')
    actual_rows = [line for line in report.splitlines() if line.startswith(('| Ordinary |','| ThinLTO |','| `scaled-','| [baseline ','| [final '))]
    assert collections.Counter(actual_rows) == collections.Counter(expected_rows), ('report numeric table mismatch', set(expected_rows)-set(actual_rows), set(actual_rows)-set(expected_rows))
    assert f"**{min(row['relar_over_rust'] for row in large):.2f}–{max(row['relar_over_rust'] for row in large):.2f}× Rust’s time**" in report
    fractions = [row['control_fraction']['relar'] for row in large]
    assert f'{min(fractions)*100:.2f}–{max(fractions):.2%}' in report
    assert f'{min(small)*100:.2f}–{max(small):.2%}' in report
    before, after = (profiles['lookup'][label]['calls'] for label in ('baseline','optimized'))
    assert (before-after)/before == 0.4 and f'{before:,} to {after:,} (40%)' in report
    before, after = (profiles['decoder_entry_instructions'][label] for label in ('baseline','optimized'))
    assert f'{before:,} to {after:,} guest instructions' in report
    assert profiles['decoder_allocations']['tbk'] == 48820 and profiles['decoder_allocations']['tb'] == 2281528
    assert [round(item['self_instructions']/1e6,2) for item in profiles['top_functions']] == [11.58,9.27]
    for caveat in ('separate sessions','descriptive rather than isolated causal estimates','six-byte','`argv[0]`','startup-heavy','No control time is subtracted','not RSS','not hardware cycles','two changes were measured together','without `fsync`'):
        assert caveat in report, ('missing caveat',caveat)
    assert 'warm' in report and 'page cache' in report
    units = raw('/tmp/relar-memory-build-optimized/units.log').decode()
    assert sum(map(int,re.findall(r'test result: ok\. (\d+) passed; 0 failed;',units))) == 241
    return {'path': str(report_path), 'sha256': sha(report.encode()), 'render_sha256': guard(render_path), 'numeric_rows': len(expected_rows), 'large_ratio_range': [min(row['relar_over_rust'] for row in large),max(row['relar_over_rust'] for row in large)], 'small_native_control_fraction_range': [min(small),max(small)]}


def audit_archive(prefix):
    archive_path = Path(str(prefix) + '-evidence.tar.gz')
    manifest_path = Path(str(prefix) + '-evidence-manifest.json')
    index_path = Path(str(prefix) + '-evidence-index.json')
    manifest, index = load(manifest_path), load(index_path)
    assert guard(archive_path,manifest['archive']['sha256']) and archive_path.stat().st_size == manifest['archive']['bytes']
    assert raw(archive_path)[:10] == b'\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff'
    data = {}
    with tarfile.open(archive_path,'r:gz') as archive:
        assert archive.getnames() == sorted(manifest['members'])
        assert len(archive.getnames()) == len(set(archive.getnames())) == manifest['member_count']
        for member in archive:
            name = PurePosixPath(member.name)
            assert member.isfile() and not name.is_absolute() and '..' not in name.parts
            assert member.uid == member.gid == member.mtime == 0 and member.uname == member.gname == ''
            value = archive.extractfile(member).read()
            assert native_kind(value[:16]) is None
            assert {'sha256': sha(value),'bytes':len(value),'mode':member.mode} == manifest['members'][member.name]
            data[member.name] = value
    assert data['provenance/logical-index.json'] == raw(index_path)
    package = json.loads(data['provenance/package.json'])
    assert all(manifest[key] == value for key,value in package.items())
    assert package['measurement_revision'] == FINAL and package['logical_index_sha256'] == guard(index_path)
    referenced = set()
    def verify(item):
        name = item['member']; referenced.add(name)
        assert name == 'blobs/sha256/' + item['sha256']
        value = data[name]
        assert len(value) == item['bytes'] and sha(value) == item['sha256']
        assert manifest['members'][name]['mode'] == 0o644
        return value
    for path,item in index['files'].items():
        assert Path(path).is_absolute() and verify(item) == raw(path)
    source_count = 0
    for revision, files in index['sources'].items():
        expected, omitted = {}, {}
        for entry in filter(None,git('ls-tree','-r','-z',revision).split(b'\0')):
            metadata,name = entry.split(b'\t',1);mode,kind,oid = metadata.split(); name = name.decode()
            assert kind == b'blob' and mode in (b'100644',b'100755')
            (omitted if name.startswith('benchmarks/results/') else expected)[name] = {'mode':int(mode,8)&0o777,'git_blob':oid.decode()}
        assert set(files) == set(expected) and index['omitted_historical_results'][revision] == omitted
        assert index['source_trees'][revision] == git('rev-parse',revision+'^{tree}').decode().strip()
        for name,item in files.items():
            value = verify(item)
            assert item['mode'] == expected[name]['mode'] and item['git_blob'] == expected[name]['git_blob']
            assert hashlib.sha1(b'blob '+str(len(value)).encode()+b'\0'+value).hexdigest() == expected[name]['git_blob']
            source_count += 1
    for binding in index['captured_source_bindings']:
        item = index['sources'][binding['revision']][binding['relative']]
        assert all(binding[key] == value for key,value in item.items())
        assert str(ROOT / binding['relative']) == binding['path']
    for directory, entries in index['trees'].items():
        directory = Path(directory)
        assert not directory.is_symlink()
        actual = {}
        for path in directory.rglob('*'):
            assert not path.is_symlink() and (path.is_file() or path.is_dir())
            actual[path.relative_to(directory).as_posix()] = path.is_dir()
        assert actual == entries
        for name,is_directory in entries.items():
            if not is_directory: assert str(directory/name) in index['files'] or str(directory/name) in index['excluded']
    exclusions = json.loads(data['provenance/exclusions.json'])
    assert len(exclusions) == manifest['excluded_artifact_count'] == len(index['excluded'])
    for item in exclusions:
        path = Path(item['path']); expected = index['excluded'][str(path)]
        assert all(item[key] == value for key,value in expected.items())
        assert item['would_be_member'] == 'external/' + sha(str(path).encode())
        assert str(path) not in index['files'] and path.stat().st_size == item['bytes']
        guard(path,item['sha256'])
        with path.open('rb') as stream: assert native_kind(stream.read(16)) == item['reason']
    assert referenced == {name for name in data if name.startswith('blobs/')}
    assert set(data) == referenced | {'README.md','provenance/logical-index.json','provenance/package.json','provenance/exclusions.json'}
    assert source_count == package['logical_source_count'] and len(index['files']) == package['logical_file_count']
    for stage in ('baseline','final'):
        for mode in ('normal','thinlto'):
            original = f'/tmp/relar-memory-timing-{mode}-' + ('v2' if stage == 'baseline' else 'final-v1') + '.json'
            suffix = ('baseline-' if stage == 'baseline' else '') + mode
            compressed = raw(str(prefix)+'-'+suffix+'.json.gz')
            assert gzip.decompress(compressed) == raw(original)
    assert raw(str(prefix)+'-audit.json') == raw('/tmp/relar-memory-final-audit.json')
    assert raw(str(prefix)+'-profile-comparison.json') == raw('/tmp/relar-memory-profile-comparison.json')
    assert package['report_sha256'] == guard(prefix.with_suffix('.md'))
    assert package['renderer_sha256'] == guard('/tmp/relar-memory-render-report.py')
    assert package['replay_sha256'] == guard('/tmp/2026-10-21-memory-report-audit.py')
    return {'sha256':guard(archive_path),'manifest_sha256':guard(manifest_path),'index_sha256':guard(index_path),'members':len(data),'logical_files':len(index['files']),'source_entries':source_count,'source_revisions':len(index['sources']),'native_exclusions':len(exclusions)}


if __name__ == '__main__':
    main()
