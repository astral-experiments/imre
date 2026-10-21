"""Replay native-Path benchmark evidence after timing; never run measured engines.

The local audit binds captures to Git sources, build records, tools, and native
artifacts. Portable replay can reuse check_capture/check_row with directory=None;
those helpers validate all recorded streams, filesystem hashes, and statistics.
"""
import argparse
import collections
from datetime import datetime
import hashlib
import itertools
import json
from pathlib import Path
import re
import runpy
import statistics
import struct
import subprocess
import sys
import tempfile

if sys.flags.optimize:
    raise SystemExit('audit requires assertions; do not use -O')
sys.dont_write_bytecode = True
ROOT = Path('/home/dev-user/code/oss/relar')
REVISION = 'cec52a5a963343f974ac774db5e9f25a7e7fc5ba'
ENGINES = ('relar', 'port', 'original', 'rust')
COMPUTE_ENGINES = ('relar', 'original', 'rust')
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


def load(path, expected=None):
    guard(path, expected)
    return json.loads(Path(path).read_text())


def source(relative):
    key = str(relative)
    if key not in GIT:
        GIT[key] = subprocess.check_output(['git', '-C', str(ROOT), 'show', REVISION + ':' + key])
    return GIT[key]


def encoded(files):
    return {name: {'sha256': sha(data), 'bytes': len(data)} for name, data in sorted(files.items())}


def check_capture(validator, case, expected, capture, engine, compute=False, directory=None):
    """Verify one capture and optional retained files; return exact expected bytes."""
    stdout, stderr = capture['stdout'], capture['stderr']
    if compute:
        assert capture['returncode'] == 0 and stderr == ''
        checksum, rendered = stdout.split('\n', 1)
        assert rendered.endswith('\n')
        rendered = rendered[:-1]
        if engine == 'original':
            validator.equivalent_original(rendered, expected)
        else:
            assert rendered == expected['rendered'], (case.label, engine, 'compute report')
        assert checksum == str(expected['rounds'] * len(rendered)), (case.label, engine, 'checksum')
        files = expected['files']
    elif expected['error']:
        assert capture['returncode'] != 0 and stdout == ''
        category = {'OSError': r'(?:OSError|FileNotFoundError|IsADirectoryError|NotADirectoryError)',
                    'ValueError': r'(?:ValueError|UnicodeDecodeError)'}.get(expected['error'], expected['error'])
        assert re.search(category + ':', stderr), (case.label, engine, stderr)
        files = expected['files']
    else:
        assert capture['returncode'] == 0
        if engine == 'original':
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
    assert capture['files'] == encoded(files), (case.label, engine, 'file hashes')
    assert capture['directories'] == expected['directories'], (case.label, engine, 'directories')
    if directory is not None:
        directory = Path(directory)
        actual, directories = {}, []
        for path in sorted(directory.rglob('*')):
            assert not path.is_symlink(), path
            name = path.relative_to(directory).as_posix()
            if path.is_dir():
                directories.append(name)
            else:
                actual[name] = path.read_bytes()
        assert actual == files and directories == expected['directories'], ('retained filesystem', directory)
    return files


def independent_orders(engines):
    if len(engines) == 3:
        return [list(order) for order in itertools.permutations(engines)]
    assert len(engines) == 4
    return [[engines[(index + rotation) % 4] for index in (0, 1, 3, 2)] for rotation in range(4)]


def check_row(validator, row, description, case, commands, *, samples, warmups, compute=False, directory=None):
    """Check a complete measured row; return medians, ratios, and execution counts.

    commands must contain original recorded absolute command strings. directory,
    when given, is this row's retained root with one subdirectory per engine.
    """
    engines = tuple(commands)
    assert engines == (COMPUTE_ENGINES if compute else ENGINES)
    assert {key: row[key] for key in description} == description, (case.label, 'description')
    orders = independent_orders(engines)
    assert row['engine_orders'] == orders
    assert set(row['gates']) == set(row['summary']) == set(engines)
    if directory is not None:
        assert {path.name for path in Path(directory).iterdir()} == set(engines)
    expected = validator.compute_oracle(case) if compute else validator.oracle(case)
    for engine, capture in row['gates'].items():
        assert set(capture) == {'command', 'elapsed_ns', 'returncode', 'stdout', 'stderr', 'files', 'directories'}
        assert capture['command'] == [*commands[engine], *case.arguments]
        assert type(capture['elapsed_ns']) is int and capture['elapsed_ns'] > 0
        check_capture(validator, case, expected, capture, engine, compute,
                      None if directory is None else Path(directory) / engine)
    for phase, count in [('warmups', warmups), ('samples', samples)]:
        assert len(row[phase]) == count
        for index, capture in enumerate(row[phase]):
            assert set(capture) == {'index', 'order', *engines}
            assert capture['index'] == index and capture['order'] == orders[index % len(orders)]
            for engine in engines:
                assert type(capture[engine]['elapsed_ns']) is int and capture[engine]['elapsed_ns'] > 0
                assert {key: value for key, value in capture[engine].items() if key != 'elapsed_ns'} == {
                    key: value for key, value in row['gates'][engine].items() if key != 'elapsed_ns'}
    positions = collections.Counter((engine, position) for capture in row['samples']
                                    for position, engine in enumerate(capture['order']))
    neighbors = collections.Counter(pair for capture in row['samples']
                                    for pair in zip(capture['order'], capture['order'][1:]))
    assert len(positions) == len(engines) ** 2 and set(positions.values()) == {samples // len(engines)}
    assert len(neighbors) == len(engines) * (len(engines) - 1)
    assert set(neighbors.values()) == {samples // len(engines)}
    seconds = {}
    for engine in engines:
        values = [capture[engine]['elapsed_ns'] / 1e9 for capture in row['samples']]
        quartiles = statistics.quantiles(values, n=4, method='inclusive')
        summary = {'median_seconds': statistics.median(values), 'min_seconds': min(values),
                   'max_seconds': max(values), 'p25_seconds': quartiles[0], 'p75_seconds': quartiles[2]}
        assert row['summary'][engine] == summary
        seconds[engine] = summary['median_seconds']
    ratios = {f'{left}_over_{right}': seconds[left] / seconds[right]
              for left, right in [('original', 'relar'), ('port', 'relar'), ('relar', 'rust'),
                                  ('original', 'rust'), ('port', 'rust')]
              if left in seconds and right in seconds}
    assert {key: value for key, value in row.items() if '_over_' in key} == ratios
    return {'label': row['label'], 'kind': 'compute' if compute else 'application',
            'records': row['records'], 'rounds': row.get('rounds'), 'seconds': seconds, **ratios,
            'counts': {'gates': len(engines), 'warmups': warmups * len(engines), 'samples': samples * len(engines)}}


def reconstruct_oracle(prepared, destination):
    for name, captured in prepared['source_and_tool_paths'].items():
        if name.startswith(('helper:', 'example:', 'stdlib:', 'fixture:', 'upstream:')):
            relative = Path(captured).relative_to(ROOT)
            data = source(relative)
            assert sha(data) == prepared['source_and_tool_sha256'][name]
            path = destination / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    benchmark = runpy.run_path(str(destination / 'benchmarks/memory_benchmark.py'))
    return benchmark, benchmark['validator']


def original_commands(benchmark, prepared, paths, oracle_root, compute=False):
    function = benchmark['compute_commands_for' if compute else 'commands_for']
    commands = function(prepared, paths)
    return {engine: [argument.replace(str(oracle_root), str(ROOT)) for argument in command]
            for engine, command in commands.items() if not compute or engine != 'port'}


def check_preparation(prepared, mode):
    assert prepared['revision'] == REVISION and not prepared['dirty'] and prepared['input_guards_passed']
    assert prepared['thinlto'] == (mode == 'thinlto') and prepared['effective_target'] == 'x86_64-unknown-linux-gnu'
    paths, hashes = prepared['source_and_tool_paths'], prepared['source_and_tool_sha256']
    assert paths.keys() == hashes.keys()
    for name, captured in paths.items():
        path = Path(captured)
        if path.is_relative_to(ROOT):
            assert sha(source(path.relative_to(ROOT))) == hashes[name], (name, 'Git source binding')
        else:
            guard(path, hashes[name])
    for name in ('binary', 'llvm', 'validation', 'compute_binary', 'compute_llvm', 'compute_validation'):
        guard(prepared[name], prepared[name + '_sha256'])
    for entry, binary, llvm, key in [('__main__.py', 'binary', 'llvm', 'compile_command'),
                                    ('compute.py', 'compute_binary', 'compute_llvm', 'compute_compile_command')]:
        expected = [paths['compiler'], 'build', str(ROOT / 'examples/memory_report' / entry), '--module-root',
                    str(ROOT / 'examples'), '-o', prepared[binary], '--runtime', paths['runtime'], '--clang',
                    paths['clang'], '--opt-level', '3', '--emit-llvm', prepared[llvm], '--target', prepared['effective_target']]
        if mode == 'thinlto':
            expected += ['--lto-linker', paths['lto_linker']]
        assert prepared[key] == expected
    assert prepared['native_elf'] == {'bits': 64, 'byte_order': 'little', 'machine': 62}
    for name in (prepared['binary'], prepared['compute_binary'], paths['rust']):
        with Path(name).open('rb') as stream:
            header = stream.read(20)
        assert header[:6] == b'\x7fELF\x02\x01' and struct.unpack('<H', header[18:20])[0] == 62
    build = load(paths['relar_build_record'])
    assert build['revision'] == REVISION and build['source_matches_revision'] and build['input_guards_passed']
    assert build['sha256']['compiler'] == hashes['compiler'] and build['sha256'][mode + '_runtime'] == hashes['runtime']
    for relative, digest in build['source_sha256'].items():
        assert sha(source(relative)) == digest
    rust = load(paths['rust_build_record'])
    assert rust == prepared['rust_build_record'] and rust['input_guards_passed'] and rust['binary_sha256'] == hashes['rust']
    assert rust['thinlto'] == (mode == 'thinlto') and rust['llvm_budget'] == (1000 if mode == 'thinlto' else None)
    assert rust['rustc_verbose'].strip() == build['toolchain'].strip()
    for key, value in {'CARGO_PROFILE_RELEASE_OPT_LEVEL': '3', 'CARGO_PROFILE_RELEASE_LTO': 'thin' if mode == 'thinlto' else 'off',
                       'CARGO_PROFILE_RELEASE_CODEGEN_UNITS': '16'}.items():
        assert rust['environment'][key] == value
    for relative, digest in rust['sources'].items():
        data = source('benchmarks/reference/memory_report/' + relative)
        assert sha(data) == digest and rust['source_contents'][relative] == data.decode()
    return paths


def check_validation(validator, prepared, counts):
    cases = validator.cases()
    validation = load(prepared['validation'])
    assert len(cases) == validation['cases'] == prepared['validation_cases'] == 92
    assert tuple(validation['engines']) == ENGINES and validation['engine_executions'] == 335
    for capture, (case, error, original) in zip(validation['results'], cases, strict=True):
        assert capture['case'] == validator.case_record(case) and capture['expected_error'] == error
        assert set(capture['engines']) == set(ENGINES) - (set() if original else {'original'})
        expected = validator.oracle(case, error)
        for engine, result in capture['engines'].items():
            check_capture(validator, case, expected, result, engine)
            counts['preparation_application_executions'] += 1
    selected = {'directory-common-projects', 'directory-dotfile-unicode-and-multiple-suffix',
                'directory-stable-project-ties', 'directory-big-integers', 'directory-unchanged',
                'directory-ignored-schema-fields'}
    compute_cases = [validator.compute_case(case, rounds) for case, error, original in cases
                     if case.label in selected for rounds in (0, 2)]
    validation = load(prepared['compute_validation'])
    assert len(compute_cases) == validation['cases'] == prepared['compute_validation_cases'] == 12
    assert tuple(validation['engines']) == ENGINES and validation['engine_executions'] == 48
    for capture, case in zip(validation['results'], compute_cases, strict=True):
        assert capture['case'] == validator.case_record(case) and set(capture['engines']) == set(ENGINES)
        expected = validator.compute_oracle(case)
        for engine, result in capture['engines'].items():
            check_capture(validator, case, expected, result, engine, compute=True)
            counts['preparation_compute_executions'] += 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--timings-complete', required=True, action='store_true')
    parser.add_argument('--output', type=Path, default=Path('/tmp/2026-10-21-native-paths-audit.json'))
    parser.add_argument('--context', type=Path, default=Path('/tmp/relar-path-timing-context.json'))
    args = parser.parse_args()
    guard(__file__)
    context = load(args.context)
    assert tuple(context['commands']) == ('normal', 'thinlto')
    assert context['revision'] == REVISION and context['cwd'] == str(ROOT)
    assert context['status'] == 'passed' and context['outside_tool_sandbox']
    assert context['cpu'] == 31 and 31 in context['available_cpu_affinity']
    assert context['idle_probe']['cpu31']['busy_fraction'] < 0.10
    for probe in context['idle_probe'].values():
        values = probe['jiffies']
        assert probe['busy_fraction'] == (sum(values[:8]) - values[3] - values[4]) / sum(values[:8])
    assert [window['mode'] for window in context['windows']] == ['normal', 'thinlto']
    previous = datetime.fromisoformat(context['started_at'])
    for window in context['windows']:
        assert window['command'] == context['commands'][window['mode']] and window['exit_code'] == 0
        started, finished = (datetime.fromisoformat(window[key]) for key in ('started_at', 'finished_at'))
        assert previous <= started < finished <= datetime.fromisoformat(context['finished_at'])
        previous = finished
        guard(window['log'])
        assert Path(window['log']).read_text().rstrip().endswith('Saved /tmp/relar-path-timing-' + window['mode'] + '.json')
    previous = datetime.fromisoformat(context['started_at'])
    counts, modes, identities = collections.Counter(), {}, {}
    first = load('/tmp/relar-path-benchmark-normal-prepared.json')
    with tempfile.TemporaryDirectory(prefix='relar-path-data-audit-') as temporary:
        oracle_root = Path(temporary)
        benchmark, validator = reconstruct_oracle(first, oracle_root)
        empty = validator.case('empty-control', validator.report(), validator.report())
        descriptions = [(benchmark['describe'](empty, 0, 'empty-schema-valid-reports'), empty),
                        *benchmark['workloads']([32, 256], [(256, 1), (4, 64)])]
        assert len(descriptions) == 11
        for mode in ('normal', 'thinlto'):
            timing_path = Path(f'/tmp/relar-path-timing-{mode}.json')
            timing = load(timing_path)
            prepared_path = Path(f'/tmp/relar-path-benchmark-{mode}-prepared.json')
            prepared = load(prepared_path, timing['preparation_record_sha256'])
            assert timing['preparation'] == prepared and timing['revision'] == REVISION and not timing['dirty']
            assert timing['kind'] == 'memory-benchmark' and timing['input_guards_passed']
            started, completed = (datetime.fromisoformat(timing[key]) for key in ('started_at_utc', 'completed_at_utc'))
            assert previous <= started < completed <= datetime.fromisoformat(context['finished_at'])
            assert datetime.fromisoformat(prepared['completed_at_utc']) <= started
            window = next(window for window in context['windows'] if window['mode'] == mode)
            assert datetime.fromisoformat(window['started_at']) <= started < completed <= datetime.fromisoformat(window['finished_at'])
            previous = completed
            assert timing['samples'] == 12 and timing['warmups'] == 2 and timing['copies'] == [32, 256]
            assert timing['directory_layouts'] == [[256, 1], [4, 64]] and timing['compute_rounds'] == 8
            assert timing['cpu']['affinity'] == [31] and timing['cpu']['model'] == 'AMD EPYC-Milan Processor'
            python = timing['python']
            assert python['implementation'] == 'cpython' and python['version_number'] == '3.14.5'
            assert python['gil_enabled'] and not any(python[key] for key in ('free_threaded_build', 'jit_enabled', 'debug_build'))
            assert all(python['pgo'].values()) and python['lto']['lto_flag_present']
            assert python['lto']['configure_option'] == '--with-lto'
            assert timing['python_environment'] == {'PYTHONHASHSEED': '0', 'PYTHONPATH': '', 'PYTHONHOME': None}
            paths = check_preparation(prepared, mode)
            check_validation(validator, prepared, counts)
            commands = original_commands(benchmark, prepared, paths, oracle_root)
            compute_commands = original_commands(benchmark, prepared, paths, oracle_root, compute=True)
            assert timing['engine_orders'] == independent_orders(ENGINES)
            directory = timing_path.with_suffix('')
            expected_command = [paths['python'], 'benchmarks/memory_benchmark.py', '--compiler', paths['compiler'],
                                '--runtime', paths['runtime'], '--rust', paths['rust'], '--rust-build-record',
                                paths['rust_build_record'], '--relar-build-record', paths['relar_build_record'],
                                '--python', paths['python'], '--clang', paths['clang'], '--target', prepared['effective_target'],
                                '--artifact-dir', str(directory), '--output', str(timing_path)]
            if mode == 'thinlto':
                expected_command += ['--lto-linker', paths['lto_linker']]
            expected_command += ['--prepared-record', str(prepared_path), '--cpu', '31', '--samples', '12', '--warmups', '2']
            assert context['commands'][mode] == expected_command
            rows = [timing['startup_control'], *timing['workloads']]
            result_rows = []
            for row, (description, case) in zip(rows, descriptions, strict=True):
                result = check_row(validator, row, description, case, commands, samples=12, warmups=2,
                                   directory=directory / row['label'])
                counts.update(result['counts'])
                if result_rows:
                    fractions = {engine: result_rows[0]['seconds'][engine] / result['seconds'][engine] for engine in ENGINES}
                    assert row['startup_control_fraction'] == fractions
                    result['startup_control_fraction'] = fractions
                result_rows.append(result)
            full = [(description, case) for description, case in descriptions if description.get('mode') == 'full']
            assert len(full) == len(timing['compute_workloads']) == 2
            compute_rows = []
            labels = {row['label'] for row in rows}
            for row, (description, ordinary) in zip(timing['compute_workloads'], full, strict=True):
                case = validator.compute_case(ordinary, 8)
                metadata = {**description, 'label': case.label, 'arguments': list(case.arguments), 'rounds': 8}
                result = check_row(validator, row, metadata, case, compute_commands, samples=12, warmups=2,
                                   compute=True, directory=directory / row['label'])
                control = validator.compute_case(ordinary, 0)
                control_metadata = {**description, 'label': control.label, 'arguments': list(control.arguments), 'rounds': 0,
                                    'expected_markdown_sha256': sha(b''), 'expected_markdown_bytes': 0}
                preload = check_row(validator, row['preload_control'], control_metadata, control, compute_commands,
                                    samples=12, warmups=2, compute=True, directory=directory / control.label)
                counts.update(result['counts'])
                counts.update(preload['counts'])
                amortized = {engine: seconds / 8 for engine, seconds in result['seconds'].items()}
                fractions = {engine: preload['seconds'][engine] / result['seconds'][engine] for engine in COMPUTE_ENGINES}
                assert row['amortized_seconds'] == amortized and row['preload_control_fraction'] == fractions
                result.update(amortized_seconds=amortized, preload_control_fraction=fractions, preload_control=preload)
                compute_rows.append(result)
                labels.update((case.label, control.label))
            assert {path.name for path in directory.iterdir()} == labels
            modes[mode] = {'application': result_rows, 'compute': compute_rows}
            identities[mode] = {'preparation': guard(prepared_path), 'timing': guard(timing_path),
                                'compiler': paths['compiler'], 'binary_sha256': prepared['binary_sha256'],
                                'compute_binary_sha256': prepared['compute_binary_sha256'],
                                'runtime_sha256': prepared['source_and_tool_sha256']['runtime'],
                                'rust_sha256': prepared['source_and_tool_sha256']['rust']}
    for path, digest in list(INPUTS.items()):
        guard(path, digest)
    large = [row for mode in modes.values() for row in mode['application'] if row['label'].endswith('-256')]
    directory = [row for mode in modes.values() for row in mode['application'] if row['label'].startswith('directory-')]
    compute = [row for mode in modes.values() for row in mode['compute']]
    assert len(large) == 4 and len(directory) == 8 and len(compute) == 4
    result = {'schema': 1, 'status': 'passed', 'measurement_revision': REVISION,
              'scope': 'Independent data replay of current preparation, application/compute/preload outputs, retained files, balanced orders, raw statistics, and Git/build/tool/artifact identities; no engine execution or historical benchmark dependency.',
              'counts': dict(counts), 'modes': modes, 'identities': identities,
              'all_large_file_cases_at_most_twice_rust': all(row['relar_over_rust'] <= 2 for row in large),
              'all_directory_cases_at_most_twice_rust': all(row['relar_over_rust'] <= 2 for row in directory),
              'all_repeated_compute_cases_at_most_twice_rust': all(row['relar_over_rust'] <= 2 for row in compute),
              'audit_script_sha256': INPUTS[str(Path(__file__).absolute())], 'guarded_inputs': INPUTS,
              'limits': 'Reuses the independently reviewed Git-bound application oracle. Captures cannot reconstruct historical host isolation or prove which instructions ran. Native identities are rehashed without execution; repeated-compute estimates include preload/startup/final output and subtract no control.'}
    with args.output.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({'output': str(args.output), 'counts': dict(counts),
                      'directory_relar_over_rust': [row['relar_over_rust'] for row in directory],
                      'compute_relar_over_rust': [row['relar_over_rust'] for row in compute]}, indent=2))


if __name__ == '__main__':
    main()
