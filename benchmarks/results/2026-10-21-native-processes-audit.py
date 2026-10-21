"""Replay frozen native-process evidence after timing; never execute an engine.

Statistics are recomputed independently. Git-bound process/compute oracles check
all captures, while retained Markdown resolves upstream's permitted set-order
ties without accepting an opaque output hash as evidence of correctness.
"""
import argparse
import collections
from datetime import datetime
import gzip
import hashlib
import itertools
import json
import os
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
REVISION = 'aca68f5631795f01c3cbb2dcdc56fd92866f6620'
ENGINES = ('relar', 'original', 'rust')
VALIDATION_ENGINES = ('relar', 'port', 'original', 'rust')
ASAN_ENGINES = ('original', 'port', 'relar', 'rust')
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
    relative = str(relative)
    if relative not in GIT:
        GIT[relative] = subprocess.check_output(['git', '-C', str(ROOT), 'show', REVISION + ':' + relative])
    return GIT[relative]


def encoded(files):
    return {name: {'sha256': sha(data), 'bytes': len(data)} for name, data in sorted(files.items())}


def check_capture(validator, case, expected, capture, engine, compute=False,
                  directory=None, prepared_process=False, rendered_hint=None):
    """Check captured streams/files; optionally verify and guard retained bytes."""
    stdout, stderr = capture['stdout'], capture['stderr']
    assert 'AddressSanitizer' not in stderr and 'LeakSanitizer' not in stderr
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
                    'ValueError': r'(?:ValueError|UnicodeDecodeError|JSONDecodeError)'}.get(expected['error'], expected['error'])
        assert re.search(category + ':', stderr), (case.label, engine, stderr)
        files = expected['files']
    else:
        assert capture['returncode'] == 0
        if engine == 'original' and prepared_process:
            assert stderr == ''
            if expected['output'] is None:
                assert stdout.endswith('\n')
                rendered = stdout[:-1]
            else:
                assert stdout == ''
                if directory is not None:
                    rendered = (Path(directory) / expected['output']).read_text()
                elif rendered_hint is not None:
                    rendered = rendered_hint
                else:
                    # Small one-shot fixtures have distinct magnitudes. Require
                    # their canonical bytes; never accept an opaque hash alone.
                    rendered = expected['rendered']
            validator.equivalent_original(rendered, expected)
        elif engine == 'original':
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
                guard(path, sha(actual[name]))
        assert actual == files and directories == expected['directories'], ('retained filesystem', directory)
    return files


def independent_orders(engines):
    assert len(engines) == 3
    return [list(order) for order in itertools.permutations(engines)]


def check_row(validator, row, description, case, commands, *, samples, warmups,
              compute=False, prepared_process=False, process_api=None, directory=None):
    """Recompute orders, medians, inclusive quartiles, and ratios from raw data."""
    engines = tuple(commands)
    assert engines == ENGINES
    assert {key: row[key] for key in description} == description, (case.label, 'description')
    orders = independent_orders(engines)
    assert row['engine_orders'] == orders
    assert set(row['gates']) == set(row['summary']) == set(engines)
    if directory is not None:
        assert {path.name for path in Path(directory).iterdir()} == set(engines)
    expected = (process_api.oracle(case) if prepared_process else
                validator.compute_oracle(case) if compute else validator.oracle(case))
    for engine, capture in row['gates'].items():
        assert set(capture) == {'command', 'elapsed_ns', 'returncode', 'stdout', 'stderr', 'files', 'directories'}
        assert capture['command'] == [*commands[engine], *case.arguments]
        assert type(capture['elapsed_ns']) is int and capture['elapsed_ns'] > 0
        check_capture(validator, case, expected, capture, engine, compute,
                      None if directory is None else Path(directory) / engine,
                      prepared_process=prepared_process)
    for phase, count in [('warmups', warmups), ('samples', samples)]:
        assert len(row[phase]) == count
        for index, capture in enumerate(row[phase]):
            assert set(capture) == {'index', 'order', *engines}
            assert capture['index'] == index and capture['order'] == orders[index % len(orders)]
            for engine in engines:
                assert type(capture[engine]['elapsed_ns']) is int and capture[engine]['elapsed_ns'] > 0
                # Equality binds every invocation to a fully validated gate,
                # including its entire stdout/stderr and all file identities.
                assert {key: value for key, value in capture[engine].items() if key != 'elapsed_ns'} == {
                    key: value for key, value in row['gates'][engine].items() if key != 'elapsed_ns'}
    positions = collections.Counter((engine, position) for capture in row['samples']
                                    for position, engine in enumerate(capture['order']))
    neighbors = collections.Counter(pair for capture in row['samples']
                                    for pair in zip(capture['order'], capture['order'][1:]))
    assert len(positions) == 9 and set(positions.values()) == {samples // 3}
    assert len(neighbors) == 6 and set(neighbors.values()) == {samples // 3}
    seconds, spreads = {}, {}
    for engine in engines:
        values = [capture[engine]['elapsed_ns'] / 1e9 for capture in row['samples']]
        quartiles = statistics.quantiles(values, n=4, method='inclusive')
        summary = {'median_seconds': statistics.median(values), 'min_seconds': min(values),
                   'max_seconds': max(values), 'p25_seconds': quartiles[0], 'p75_seconds': quartiles[2]}
        assert row['summary'][engine] == summary
        seconds[engine] = summary['median_seconds']
        spreads[engine] = quartiles[2] - quartiles[0]
    ratios = {f'{left}_over_{right}': seconds[left] / seconds[right]
              for left, right in [('original', 'relar'), ('relar', 'rust'), ('original', 'rust')]}
    assert {key: value for key, value in row.items() if '_over_' in key} == ratios
    if compute and row['rounds']:
        assert row['amortized_seconds'] == {engine: seconds[engine] / row['rounds'] for engine in engines}
    return {'label': row['label'], 'kind': 'compute' if compute else 'process',
            'records_per_side': row['records_per_side'], 'rounds': row.get('rounds'),
            'child_processes': row['child_processes'], 'seconds': seconds, 'iqr_seconds': spreads, **ratios,
            'counts': {'gates': 3, 'warmups': warmups * 3, 'samples': samples * 3}}


def reconstruct_oracle(prepared, destination):
    for name, captured in prepared['source_and_tool_paths'].items():
        if name.startswith(('helper:', 'example:', 'stdlib:', 'fixture:', 'upstream:')):
            relative = Path(captured).relative_to(ROOT)
            data = source(relative)
            assert sha(data) == prepared['source_and_tool_sha256'][name]
            path = destination / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    benchmark = runpy.run_path(str(destination / 'benchmarks/memory_process_benchmark.py'))
    return benchmark, benchmark['validator']


def original_commands(benchmark, prepared, paths, oracle_root, compute=False, include_port=False):
    function = benchmark['compute_commands_for' if compute else 'process_commands']
    commands = function(prepared, {name: Path(path) for name, path in paths.items()})
    return {engine: [argument.replace(str(oracle_root), str(ROOT)) for argument in command]
            for engine, command in commands.items() if include_port or engine != 'port'}


def check_preparation(prepared, mode):
    assert prepared['revision'] == REVISION and not prepared['dirty'] and prepared['input_guards_passed']
    assert prepared['thinlto'] == (mode == 'thinlto') and prepared['effective_target'] == 'x86_64-unknown-linux-gnu'
    paths, hashes = prepared['source_and_tool_paths'], prepared['source_and_tool_sha256']
    assert paths.keys() == hashes.keys()
    for name, captured in paths.items():
        path = Path(captured)
        guard(path, hashes[name])
        if path.is_relative_to(ROOT):
            assert sha(source(path.relative_to(ROOT))) == hashes[name], (name, 'Git source binding')
    for name in ('binary', 'llvm', 'validation', 'compute_binary', 'compute_llvm', 'compute_validation', 'process_validation'):
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
    for name in (prepared['binary'], prepared['compute_binary'], paths['rust'], paths['producer']):
        with Path(name).open('rb') as stream:
            header = stream.read(20)
        assert header[:6] == b'\x7fELF\x02\x01' and struct.unpack('<H', header[18:20])[0] == 62
    build = load(paths['relar_build_record'])
    assert build['revision'] == REVISION and build['source_matches_revision'] and build['input_guards_passed']
    assert build['sha256']['compiler'] == hashes['compiler'] and build['sha256'][mode + '_runtime'] == hashes['runtime']
    for relative, digest in build['source_sha256'].items():
        assert sha(source(relative)) == digest
    for step in build['steps']:
        guard(step['log'])
    assert build['precheck_sources_match'] and build['precheck']['source_guard_passed']
    assert all(step['status'] == 0 for step in build['precheck']['steps'])
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
    for kind, key in [('reference', 'rust'), ('producer', 'producer')]:
        item = rust['binaries'][kind]
        assert Path(paths[key]).name == item['name'] and hashes[key] == item['sha256']
        assert Path(paths[key]).stat().st_size == item['bytes']
    toolchain = Path('/home/dev-user/.local/share/ohm/toolchains/ohm-1.98.1-1/bin')
    for name, digest in rust['tool_sha256'].items():
        guard(toolchain / name, digest)
    return paths


def check_validation_data(validator, process_api, ordinary, computed, processes, counts, prefix, engines):
    cases = validator.cases()
    assert len(cases) == ordinary['cases'] == 92 and ordinary['engine_executions'] == 335
    assert tuple(ordinary['engines']) == engines
    for capture, (case, error, original) in zip(ordinary['results'], cases, strict=True):
        assert capture['case'] == validator.case_record(case) and capture['expected_error'] == error
        assert set(capture['engines']) == set(engines) - (set() if original else {'original'})
        expected = validator.oracle(case, error)
        for engine, result in capture['engines'].items():
            check_capture(validator, case, expected, result, engine)
            counts[prefix + '_application_executions'] += 1
    selected = {'directory-common-projects', 'directory-dotfile-unicode-and-multiple-suffix',
                'directory-stable-project-ties', 'directory-big-integers', 'directory-unchanged',
                'directory-ignored-schema-fields'}
    compute_cases = [validator.compute_case(case, rounds) for case, error, original in cases
                     if case.label in selected for rounds in (0, 2)]
    assert len(compute_cases) == computed['cases'] == 12 and computed['engine_executions'] == 48
    assert tuple(computed['engines']) == engines
    for capture, case in zip(computed['results'], compute_cases, strict=True):
        assert capture['case'] == validator.case_record(case) and set(capture['engines']) == set(engines)
        expected = validator.compute_oracle(case)
        for engine, result in capture['engines'].items():
            check_capture(validator, case, expected, result, engine, compute=True)
            counts[prefix + '_compute_executions'] += 1
    assert tuple(processes['engines']) == engines and processes['cases'] == 32 and processes['engine_executions'] == 127
    assert processes['environment'] == process_api.environment()
    assert processes['producer_source'] == process_api.PRODUCER
    assert processes['producer_sha256'] == sha(process_api.PRODUCER.encode())
    guard(processes['python'], processes['python_sha256'])
    first = validator.case_from_record(processes['results'][0]['case'])
    flags = dict(zip(first.arguments[1::2], first.arguments[2::2], strict=True))
    producer = Path(json.loads(first.files['projects.json'])['projects'][0]['argv'][1])
    process_cases = list(process_api.cases(Path(flags['--old-ty']), Path(flags['--new-ty']), producer))
    assert len(process_cases) == 32
    for capture, (case, error, original, events) in zip(processes['results'], process_cases, strict=True):
        assert capture['case'] == validator.case_record(case) and capture['expected_error'] == error
        assert set(capture['engines']) == set(engines) - (set() if original else {'original'})
        expected = process_api.oracle(case, error, events)
        for engine, result in capture['engines'].items():
            check_capture(validator, case, expected, result, engine, prepared_process=True)
            counts[prefix + '_process_executions'] += 1


def check_validation(validator, prepared, counts, process_api=None):
    ordinary, computed, processes = (load(prepared[key], prepared[key + '_sha256'])
                                     for key in ('validation', 'compute_validation', 'process_validation'))
    assert (prepared['validation_cases'], prepared['compute_validation_cases'], prepared['process_validation_cases']) == (92, 12, 32)
    check_validation_data(validator, process_api, ordinary, computed, processes, counts, 'preparation', VALIDATION_ENGINES)


def scaled_independently(data, copies):
    result = {key: value * copies for key, value in data.items() if type(value) is int}
    for group in ('structs', 'queries'):
        result[group] = []
        for copy in range(copies):
            for item in data.get(group, []):
                result[group].append({**item, 'name': item['name'] + '::copy' + str(copy)})
    return result


def reconstruct_workload_cases(benchmark, validator, prepared, paths, read_bytes, source_bytes, artifact_sha256=None):
    """Pure case/description replay using caller-supplied archive byte readers."""
    process_api = benchmark['process_api']
    if artifact_sha256 is None:
        artifact_sha256 = lambda path: sha(read_bytes(path))
    record = prepared['processes']
    directory = Path(record['directory'])
    assert record['input_guards_passed'] and record['process_layouts'] == [[64, 1], [2, 32]] and record['compute_rounds'] == 8
    assert record['environment'] == process_api.environment()
    assert record['environment']['PYTHON_JIT'] == '0' and record['environment']['PYTHONHASHSEED'] == '0'
    for role in ('old', 'new'):
        assert artifact_sha256(directory / ('capture-' + role)) == prepared['source_and_tool_sha256']['producer']
    provenance = json.loads(source_bytes('examples/memory_report/sample/provenance.json'))
    real_projects, expected_captures = [], []
    for name, index in [('inventory', 0), ('ruff-scripts', 2)]:
        original = provenance['reports'][index]
        snapshot = directory / 'sources' / name
        for relative, digest in original['producer']['sources'].items():
            assert prepared['source_and_tool_sha256'][f'project:{name}:{relative}'] == digest
            assert sha(read_bytes(snapshot / relative)) == digest
        assert read_bytes(snapshot / 'ty.toml') == b''
        argv = ['{ty}', *original['producer']['command'][1:], '--config-file', str(snapshot / 'ty.toml')]
        project = {'name': name, 'cwd': str(snapshot), 'argv': argv}
        for role, offset in [('old', 0), ('new', 1)]:
            producer = provenance['reports'][index + offset]['producer']
            assert prepared['source_and_tool_sha256'][role + '_ty'] == producer['binary_sha256']
            stdout = read_bytes(directory / f'{name}-{role}.stdout')
            stderr = read_bytes(directory / f'{name}-{role}.stderr')
            project[role] = json.loads(stdout)
            expected_captures.append({'project': name, 'role': role,
                                     'command': [paths[role + '_ty'] if item == '{ty}' else item for item in argv],
                                     'cwd': str(snapshot), 'returncode': 0,
                                     'stdout_bytes': len(stdout), 'stderr_bytes': len(stderr),
                                     'stdout_sha256': sha(stdout), 'stderr_sha256': sha(stderr)})
        real_projects.append(project)
    assert record['captures'] == expected_captures
    real = process_api.case('real-prepared-ty', real_projects, paths['old_ty'], paths['new_ty'])
    description = benchmark['describe'](real, 'Pinned ty 0.0.89 and 0.0.90 checking retained inventory and Ruff Python sources', real_projects)
    description['captured_stdout_bytes'] = sum(item['stdout_bytes'] for item in expected_captures)
    description['captured_stderr_bytes'] = sum(item['stderr_bytes'] for item in expected_captures)
    expected = [(description, real, False)]
    assert read_bytes(directory / 'real-projects.json') == real.files['projects.json']
    empty = [{'name': 'empty', 'old': validator.report(), 'new': validator.report()}]
    control = process_api.case('process-empty-control', empty, directory / 'capture-old', directory / 'capture-new')
    expected.append((benchmark['describe'](control, 'Two native child launches with empty schema-valid reports', empty), control, False))
    old = json.loads(source_bytes('examples/memory_report/sample/ruff-scripts-old.json'))
    new = json.loads(source_bytes('examples/memory_report/sample/ruff-scripts.json'))
    for count, copies in record['process_layouts']:
        before, after = scaled_independently(old, copies), scaled_independently(new, copies)
        projects = [{'name': f'project-{index:04}', 'old': before, 'new': after,
                     'stderr': b'captured ty warning\r\n' * (1024 * copies)} for index in range(count)]
        value = process_api.case(f'captured-process-{count}x{copies}', projects, directory / 'capture-old', directory / 'capture-new')
        details = benchmark['describe'](value, 'Shared native child emits distinct-name copies of retained Ruff reports and captured stderr', projects)
        details.update(copies_per_project=copies, captured_stderr_bytes=sum(2 * len(project['stderr']) for project in projects))
        assert details['records_per_side'] == 16256
        expected.append((details, value, False))
        names = [project['name'] + '.json' for project in projects]
        ordinary = validator.directory_case(f'process-compute-{count}x{copies}', dict.fromkeys(names, before), dict.fromkeys(names, after))
        for rounds in (0, 8):
            value = validator.compute_case(ordinary, rounds)
            oracle = validator.compute_oracle(value)
            metadata = {**details, 'label': value.label, 'child_processes': 0, 'rounds': rounds,
                        'arguments': list(value.arguments), 'recipe': 'Preloaded same reports; repeated decode/compare/render without children',
                        'expected_markdown_sha256': sha(oracle['rendered'].encode()), 'expected_markdown_bytes': len(oracle['rendered'].encode())}
            expected.append((metadata, value, True))
    assert len(expected) == len(record['workloads']) == 8
    for captured, (description, value, computed) in zip(record['workloads'], expected, strict=True):
        assert captured['description'] == description and captured['compute'] is computed
        compressed = read_bytes(Path(captured['case']))
        data = gzip.decompress(compressed)
        assert sha(compressed) == captured['sha256']
        assert json.loads(data) == validator.case_record(value)
        assert not any(name.endswith('/events.log') for name in value.files), ('append-only timed state', value.label)
        if not computed:
            oracle = process_api.oracle(value)
            assert description['expected_markdown_sha256'] == sha(oracle['rendered'].encode())
            assert description['expected_markdown_bytes'] == len(oracle['rendered'].encode())
    return expected


def check_workloads(benchmark, validator, prepared, paths, counts):
    """Guard retained inputs, then replay each case and description independently."""
    record = prepared['processes']
    directory = Path(record['directory'])
    observed = {str(path): guard(path) for path in sorted(directory.rglob('*')) if path.is_file()}
    assert observed == record['files']
    assert record['directories'] == [str(path) for path in sorted(directory.rglob('*')) if path.is_dir()]
    assert not any(path.is_symlink() for path in directory.rglob('*'))
    return reconstruct_workload_cases(benchmark, validator, prepared, paths, lambda path: Path(path).read_bytes(), source)


def check_workload_gates(validator, process_api, prepared, cases, timing_directory, counts):
    captures = load(Path(prepared['processes']['directory']) / 'workload-validation.json')
    assert set(captures) == {case.label for _, case, _ in cases}
    for description, case, computed in cases:
        expected = validator.compute_oracle(case) if computed else process_api.oracle(case)
        assert set(captures[case.label]) == set(VALIDATION_ENGINES)
        hint = None if computed else (Path(timing_directory) / case.label / 'original' / 'report.md').read_text()
        for engine, capture in captures[case.label].items():
            check_capture(validator, case, expected, capture, engine, compute=computed,
                          prepared_process=not computed, rendered_hint=hint)
            counts['preparation_workload_executions'] += 1


def check_asan(validator, process_api, prepared, cases, counts):
    directory = Path('/tmp/relar-process-asan-application')
    record = load(directory / 'record.json')
    assert record['revision'] == REVISION and record['input_guards_passed']
    assert record['generated_code_instrumented'] and record['runtime_instrumented'] and record['outside_sandbox']
    assert record['environment'] == {'ASAN_OPTIONS': 'detect_leaks=1:halt_on_error=1', 'PYTHON_JIT': '0'}
    for name, digest in record['artifacts'].items():
        guard(directory / name, digest)
    expected_hashes = {**prepared['source_and_tool_sha256'],
                       'asan_runtime': guard('/tmp/relar-process-build/asan_runtime.a'),
                       'prepared_record': guard('/tmp/relar-process-benchmark-normal-prepared.json')}
    assert record['source_and_tool_sha256'] == expected_hashes
    build = load(prepared['source_and_tool_paths']['relar_build_record'])
    assert build['sha256']['asan_runtime'] == expected_hashes['asan_runtime']
    step = next(step for step in build['steps'] if step['name'] == 'asan')
    assert step['overrides']['RUSTFLAGS'] == '-Zsanitizer=address -Cforce-frame-pointers=yes'
    assert '-Zsanitizer=address' in Path(step['log']).read_text() or 'Finished' in Path(step['log']).read_text()
    commands = []
    for name, entry in [('memory-report', '__main__.py'), ('memory-compute', 'compute.py')]:
        commands.append(['/tmp/relar-process-build/relar', 'build', str(ROOT / 'examples/memory_report' / entry),
                         '--module-root', str(ROOT / 'examples'), '-o', str(directory / name),
                         '--runtime', '/tmp/relar-process-build/asan_runtime.a', '--clang', '/usr/lib/llvm-18/bin/clang',
                         '--opt-level', '1', '--sanitize', '--emit-llvm', str(directory / (name + '.ll'))])
        commands.append(['/usr/bin/nm', str(directory / name)])
        assert 'sanitize_address' in (directory / (name + '.ll')).read_text()
        binary = (directory / name).read_bytes()
        assert b'__asan_init' in binary and b'__asan_report_load' in binary
        assert (directory / (name + '-compile.log')).read_text() == ''
    commands.append(['/usr/bin/nm', '-u', '/tmp/relar-process-build/asan_runtime.a'])
    symbols = (directory / 'runtime-symbols.log').read_text()
    assert '__asan_report_load' in symbols and '__asan_report_store' in symbols
    commands.append([prepared['source_and_tool_paths']['python'], str(ROOT / 'benchmarks/memory_validate.py'),
                     '--binary', str(directory / 'memory-report'), '--compute-binary', str(directory / 'memory-compute'),
                     '--rust', prepared['source_and_tool_paths']['rust'], '--python', prepared['source_and_tool_paths']['python'],
                     '--output', str(directory / 'validation.json')])
    assert record['commands'] == commands
    validation = load(directory / 'validation.json')
    check_validation_data(validator, process_api, validation, validation['compute'], validation['processes'], counts, 'asan', ASAN_ENGINES)
    gates = load(directory / 'workload-validation.json')
    assert set(gates) == {case.label for _, case, _ in cases}
    for description, case, computed in cases:
        capture = gates[case.label]
        assert capture['case_sha256'] == next(item['sha256'] for item in prepared['processes']['workloads'] if item['description']['label'] == case.label)
        assert capture['command'] == [str(directory / ('memory-compute' if computed else 'memory-report')), *case.arguments]
        expected = validator.compute_oracle(case) if computed else process_api.oracle(case)
        check_capture(validator, case, expected, capture, 'relar', compute=computed, prepared_process=not computed)
        counts['asan_workload_executions'] += 1
    assert (record['ordinary_cases'], record['ordinary_executions'], record['compute_cases'], record['compute_executions'],
            record['process_cases'], record['process_executions'], record['workload_cases']) == (92, 335, 12, 48, 32, 127, 8)
    assert 'AddressSanitizer' not in json.dumps(validation) and 'LeakSanitizer' not in json.dumps(validation)
    guard('/tmp/relar-process-asan-application.log')
    guard('/tmp/relar-process-asan-application.py', record['artifacts']['driver.py'])


def check_timing_command(command, prepared, timing_path, prepared_path, mode):
    paths = prepared['source_and_tool_paths']
    assert command[:2] == [paths['python'], 'benchmarks/memory_process_benchmark.py']
    options, key = {}, None
    for argument in command[2:]:
        if argument.startswith('--'):
            assert argument not in options
            key = argument
            options[key] = []
        else:
            assert key is not None
            options[key].append(argument)
    expected = {'--compiler': paths['compiler'], '--runtime': paths['runtime'], '--rust': paths['rust'],
                '--rust-build-record': paths['rust_build_record'], '--relar-build-record': paths['relar_build_record'],
                '--python': paths['python'], '--clang': paths['clang'], '--old-ty': paths['old_ty'], '--new-ty': paths['new_ty'],
                '--ruff-source': '/home/dev-user/code/oss/ruff', '--artifact-dir': str(timing_path.with_suffix('')),
                '--output': str(timing_path), '--prepared-record': str(prepared_path), '--cpu': '31', '--samples': '48', '--warmups': '2'}
    if mode == 'thinlto':
        expected['--lto-linker'] = paths['lto_linker']
    for key, value in expected.items():
        assert options.pop(key) == [value], (key, command)
    permitted = {'--target': [prepared['effective_target']], '--compute-rounds': ['8'], '--process-layouts': ['64:1', '2:32']}
    assert all(value == permitted.get(key) for key, value in options.items())


def check_language_validation():
    """Bind compiler/runtime suite logs and the reviewed differential probes."""
    def option(command, name):
        return command[command.index(name) + 1]

    def bound(path, digest):
        guard(path, digest)
        path = Path(path)
        if path.is_relative_to(ROOT):
            assert sha(source(path.relative_to(ROOT))) == digest

    build_path = Path('/tmp/relar-process-build/build-record.json')
    build = load(build_path)
    precheck = load('/tmp/relar-process-precheck-record.json')
    assert precheck == build['precheck'] and precheck['source_guard_passed']
    assert [step['command'][2] if step['command'][0] == 'sh' else step['command'][1]
            for step in precheck['steps']] == ['fmt', 'clippy', 'test', 'diff']
    assert all(step['status'] == 0 and step['seconds'] > 0 for step in precheck['steps'])
    guard('/tmp/relar-process-precheck.log', build['precheck_log_sha256'])
    precheck_log = Path('/tmp/relar-process-precheck.log').read_text()
    unit_results = re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed;', precheck_log)
    assert [(int(passed), int(failed)) for status, passed, failed in unit_results if status == 'ok'] == [(138, 0), (0, 0), (123, 0), (0, 0), (0, 0)]
    assert all(status == 'ok' for status, passed, failed in unit_results)
    for relative, digest in load('/tmp/relar-process-precheck-sources.json').items():
        assert sha(source(relative)) == digest
    directory = Path('/tmp/relar-process-validation/language')
    language = load(directory / 'record.json')
    assert language['revision'] == REVISION and language['status'] == 'passed'
    assert language['guards_passed'] and language['revision_unchanged']
    for path, digest in language['guarded_inputs'].items():
        bound(path, digest)
    ci = source('.github/workflows/ci.yml').decode()
    ordinary_ci = ci.split('- name: Compare native suites with CPython', 1)[1].split('- name:', 1)[0]
    suite_line = next(line for line in ordinary_ci.splitlines() if '--suites ' in line)
    suites = suite_line.split('--suites ', 1)[1].rstrip().removesuffix('\\').split()
    assert language['ordinary_suites'] == suites and len(suites) == 33 and language['ordinary_workers'] == 4
    assert language['environment_overrides']['ASAN_OPTIONS'] == 'detect_leaks=1:halt_on_error=1'
    assert set(language['results']) == {'ordinary', 'process-thinlto', 'process-asan'}
    for name, result in language['results'].items():
        assert result['returncode'] == 0 and result['seconds'] > 0
        command = result['command']
        assert command == language['commands'][name]
        assert option(command, '--compiler') == str(build_path.parent / 'relar')
        runtime = {'ordinary': 'normal', 'process-thinlto': 'thinlto', 'process-asan': 'asan'}[name]
        assert option(command, '--runtime') == str(build_path.parent / (runtime + '_runtime.a'))
        guard(result['log'], result['log_sha256'])
        log = Path(result['log']).read_text()
        assert 'AddressSanitizer' not in log and 'LeakSanitizer' not in log
        if name == 'ordinary':
            assert command[command.index('--suites') + 1:command.index('--')] == suites
            assert 'Completed 33 suites; 0 failed.' in log
        else:
            assert 'Passed 78 process checks' in log
            if name.endswith('asan'):
                assert '--sanitize' in command and command[-2:] == ['0', '1'] and 'ASan=True' in log
            else:
                assert '--lto-linker' in command and command[-2:] == ['0', '3']
    ordinary = Path(language['results']['ordinary']['log']).read_text()
    suite_directory = Path(option(language['commands']['ordinary'], '--log-dir'))
    assert set(language['ordinary_logs']) == {str(suite_directory / (suite + '.log')) for suite in suites}
    general_checks = 0
    for suite in suites:
        path = suite_directory / (suite + '.log')
        guard(path, language['ordinary_logs'][str(path)])
        log = path.read_text()
        assert re.search(r'^PASS ' + re.escape(suite) + r' \(', ordinary, re.M)
        if suite == 'base45':
            for level in (0, 3):
                assert f'O{level}: Passed 323 upstream native calls, 289 differential cases, and 2 compile-time type rejections' in log
        elif suite == 'luhn':
            for level in (0, 3):
                assert f'O{level}: relar: 15 upstream calls, 1144 differential cases' in log
        elif suite == 'imports':
            general_checks += int(re.search(r'(\d+) import checks passed', log).group(1))
        else:
            general_checks += int(re.search(r'Passed (\d+) ', log).group(1))
    assert general_checks == 1961
    affected = load('/tmp/relar-process-affected-record.json')
    assert affected['revision'] == REVISION and affected['status'] == 'passed'
    assert affected['inputs_verified_before'] and affected['inputs_verified_after'] and affected['outside_sandbox']
    assert affected['build_record_sha256'] == guard(build_path) and affected['artifact_sha256'] == build['sha256']
    for path, digest in affected['input_sha256'].items():
        bound(path, digest)
    affected_counts = {}
    for row in affected['suites']:
        assert row['returncode'] == 0
        command = row['command']
        selected = command[command.index('--suites') + 1:command.index('--')]
        assert len(selected) == 7 and set(selected) == set(row['suites'])
        guard(row['log'], row['log_sha256'])
        runner = Path(row['log']).read_text()
        log_directory = Path(option(command, '--log-dir'))
        for suite, item in row['suites'].items():
            path = log_directory / (suite + '.log')
            guard(path, item['log_sha256'])
            log = path.read_text()
            assert item['summary'] in log and item['summary'].startswith(f"Passed {item['checks']} ")
            assert re.search(r'^PASS ' + re.escape(suite) + r' \(', runner, re.M)
            assert 'AddressSanitizer' not in log and 'LeakSanitizer' not in log
        affected_counts[row['mode']] = sum(item['checks'] for item in row['suites'].values())
    assert affected_counts == {'thinlto': 501, 'asan': 668}
    assert len(affected['probes']) == 10
    for probe in affected['probes']:
        assert probe['match'] and probe['captures']['native'] == probe['captures']['cpython']
        command = probe['commands']['native']
        if probe['case'] == 'raw_environment':
            guard(command[0], affected['raw_environment_driver_sha256'])
            binary = command[1]
        else:
            binary = command[0]
        guard(binary, probe['binary_sha256'])
        if probe['mode'] == 'asan':
            assert probe['generated_llvm_asan'] and probe['runtime_asan']
        for capture in probe['captures'].values():
            assert capture['returncode'] == 0
            assert capture['stderr_bytes'] == 0 and capture['stderr_sha256'] == sha(b'')
            assert capture['stdout_bytes'] > 0 and re.fullmatch('[0-9a-f]{64}', capture['stdout_sha256'])
    return {'compiler_unit_tests': 138, 'runtime_unit_tests': 123,
            'ordinary_suites': 33, 'ordinary_general_checks': general_checks,
            'base45_upstream_calls': 646, 'base45_differential_cases': 578, 'base45_type_rejections': 4,
            'luhn_upstream_calls': 30, 'luhn_differential_cases': 2288,
            'process_thinlto_checks': 78, 'process_asan_checks': 78,
            'affected_checks': affected_counts, 'focused_differential_probes': 10,
            'language_record_sha256': guard(directory / 'record.json'),
            'affected_record_sha256': guard('/tmp/relar-process-affected-record.json')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--timings-complete', required=True, action='store_true')
    parser.add_argument('--output', type=Path, default=Path('/tmp/2026-10-21-native-processes-audit.json'))
    parser.add_argument('--context', type=Path, default=Path('/tmp/relar-process-timing-context.json'))
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
        assert Path(window['log']).read_text().rstrip().endswith('Saved /tmp/relar-process-timing-' + window['mode'] + '.json')
    counts, modes, identities = collections.Counter(), {}, {}
    first = load('/tmp/relar-process-benchmark-normal-prepared.json')
    previous = datetime.fromisoformat(context['started_at'])
    with tempfile.TemporaryDirectory(prefix='relar-process-data-audit-') as temporary:
        oracle_root = Path(temporary)
        benchmark, validator = reconstruct_oracle(first, oracle_root)
        process_api = benchmark['process_api']
        for mode in ('normal', 'thinlto'):
            timing_path = Path(f'/tmp/relar-process-timing-{mode}.json')
            timing = load(timing_path)
            prepared_path = Path(f'/tmp/relar-process-benchmark-{mode}-prepared.json')
            prepared = load(prepared_path, timing['preparation_record_sha256'])
            assert timing['preparation'] == prepared and timing['revision'] == REVISION and not timing['dirty']
            assert timing['kind'] == 'memory-process-benchmark' and timing['input_guards_passed']
            started, completed = (datetime.fromisoformat(timing[key]) for key in ('started_at_utc', 'completed_at_utc'))
            assert previous <= started < completed <= datetime.fromisoformat(context['finished_at'])
            assert datetime.fromisoformat(prepared['completed_at_utc']) <= started
            window = next(window for window in context['windows'] if window['mode'] == mode)
            assert datetime.fromisoformat(window['started_at']) <= started < completed <= datetime.fromisoformat(window['finished_at'])
            previous = completed
            assert timing['samples'] == 48 and timing['warmups'] == 2
            assert timing['cpu']['affinity'] == [31] and timing['cpu']['model']
            python = timing['python']
            assert python['implementation'] == 'cpython' and python['version_number'] == '3.14.5'
            assert python['gil_enabled'] and not any(python[key] for key in ('free_threaded_build', 'jit_enabled', 'debug_build'))
            assert all(python['pgo'].values()) and python['lto']['lto_flag_present']
            assert python['lto']['configure_option'] == '--with-lto'
            paths = check_preparation(prepared, mode)
            check_timing_command(context['commands'][mode], prepared, timing_path, prepared_path, mode)
            check_validation(validator, prepared, counts, process_api=process_api)
            cases = check_workloads(benchmark, validator, prepared, paths, counts)
            commands = original_commands(benchmark, prepared, paths, oracle_root)
            compute_commands = original_commands(benchmark, prepared, paths, oracle_root, compute=True)
            directory = timing_path.with_suffix('')
            process_rows = {row['label']: row for row in timing['workloads']}
            compute_rows = {row['label']: row for row in timing['compute_workloads']}
            assert len(process_rows) == len(timing['workloads']) == 4
            assert len(compute_rows) == len(timing['compute_workloads']) == 4
            assert {path.name for path in directory.iterdir()} == {case.label for _, case, _ in cases}
            measured = []
            for description, case, computed in cases:
                row = (compute_rows if computed else process_rows).pop(case.label)
                result = check_row(validator, row, description, case, compute_commands if computed else commands,
                                   samples=48, warmups=2, compute=computed, prepared_process=not computed,
                                   process_api=process_api, directory=directory / case.label)
                counts.update(result['counts'])
                measured.append(result)
            assert not process_rows and not compute_rows
            check_workload_gates(validator, process_api, prepared, cases, directory, counts)
            if mode == 'normal':
                check_asan(validator, process_api, prepared, cases, counts)
            control = next(row for row in measured if row['label'] == 'process-empty-control')
            for row in measured:
                if row['kind'] == 'process' and row is not control:
                    row['two_child_control_fraction'] = {engine: control['seconds'][engine] / row['seconds'][engine] for engine in ENGINES}
                if row['kind'] == 'compute' and row['rounds']:
                    preload = next(other for other in measured if other['label'] == row['label'].removesuffix('8') + '0')
                    row['amortized_seconds'] = {engine: row['seconds'][engine] / row['rounds'] for engine in ENGINES}
                    row['preload_control_fraction'] = {engine: preload['seconds'][engine] / row['seconds'][engine] for engine in ENGINES}
            modes[mode] = {'rows': measured}
            identities[mode] = {'preparation': guard(prepared_path), 'timing': guard(timing_path),
                                'compiler': paths['compiler'], 'binary_sha256': prepared['binary_sha256'],
                                'compute_binary_sha256': prepared['compute_binary_sha256'],
                                'runtime_sha256': prepared['source_and_tool_sha256']['runtime'],
                                'rust_sha256': prepared['source_and_tool_sha256']['rust'],
                                'producer_sha256': prepared['source_and_tool_sha256']['producer']}
    validation_suites = check_language_validation()
    for path, digest in list(INPUTS.items()):
        guard(path, digest)
    processes = [row for mode in modes.values() for row in mode['rows'] if row['label'].startswith('captured-process-')]
    compute = [row for mode in modes.values() for row in mode['rows'] if row['kind'] == 'compute' and row['rounds']]
    real = [row for mode in modes.values() for row in mode['rows'] if row['label'] == 'real-prepared-ty']
    assert len(processes) == len(compute) == 4 and len(real) == 2
    assert counts['samples'] == 2304 and counts['warmups'] == 96 and counts['gates'] == 48
    result = {'schema': 1, 'status': 'passed', 'measurement_revision': REVISION,
              'scope': 'Independent replay of every process/compute/preload capture, retained filesystem, preparation/ASan result, balanced order, raw statistic, and Git/build/tool/source identity; no engine execution.',
              'counts': dict(counts), 'modes': modes, 'identities': identities, 'validation_suites': validation_suites,
              'all_captured_process_cases_at_most_twice_rust': all(row['relar_over_rust'] <= 2 for row in processes),
              'all_repeated_compute_cases_at_most_twice_rust': all(row['relar_over_rust'] <= 2 for row in compute),
              'all_real_ty_cases_at_most_twice_rust': all(row['relar_over_rust'] <= 2 for row in real),
              'audit_script_sha256': INPUTS[str(Path(__file__).absolute())], 'guarded_inputs': INPUTS,
              'limits': 'Reuses independently reviewed frozen oracles. Captures cannot prove historical host isolation or instruction execution. Real-workflow timings include complete ty checking. Repeated-compute estimates include preload/startup/final output and subtract no control. Upstream ties are checked semantically using retained Markdown, then their exact bytes are bound to every capture.'}
    with args.output.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({'output': str(args.output), 'counts': dict(counts),
                      'process_relar_over_rust': [row['relar_over_rust'] for row in processes],
                      'compute_relar_over_rust': [row['relar_over_rust'] for row in compute]}, indent=2))


if __name__ == '__main__':
    main()
