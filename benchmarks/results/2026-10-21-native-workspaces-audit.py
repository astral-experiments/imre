"""Replay frozen native-workspace evidence after timing; never execute an engine.

Statistics are recomputed independently. Git-bound process/compute oracles check
all captures, while retained Markdown resolves upstream's permitted set-order
ties without accepting an opaque output hash as evidence of correctness.
"""
import argparse
import collections
from dataclasses import replace
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
ROOT = Path('/home/dev-user/code/oss/imre-workspaces-20261021')
REVISION = '7c5c426f9a890b8a805ffd1155ffd3371673ed04'
BUILD_REVISION = '2e0eac85ad0eea462be9c4feeffa76497773822c'
ENGINES = ('imre', 'original', 'rust')
VALIDATION_ENGINES = ('imre', 'port', 'original', 'rust')
ASAN_ENGINES = ('original', 'port', 'imre', 'rust')
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


def guarded_bytes(path):
    guard(path)
    return Path(path).read_bytes()


def source(relative, revision=None):
    relative = str(relative)
    revision = REVISION if revision is None else revision
    key = (revision, relative)
    if key not in GIT:
        GIT[key] = subprocess.check_output(['git', '-C', str(ROOT), 'show', revision + ':' + relative])
    return GIT[key]


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
        category = {'OSError': r'(?:OSError|FileNotFoundError|IsADirectoryError|NotADirectoryError|PermissionError|FileExistsError)',
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
    symlinks = expected.get('symlinks', {})
    if not compute:
        assert capture['symlinks'] == symlinks, (case.label, engine, 'symlinks')
    if directory is not None:
        directory = Path(directory)
        actual, directories, observed_links = {}, [], {}
        for path in sorted(directory.rglob('*')):
            name = path.relative_to(directory).as_posix()
            if path.is_symlink():
                observed_links[name] = os.readlink(path)
            elif path.is_dir():
                directories.append(name)
            else:
                assert path.is_file(), path
                actual[name] = path.read_bytes()
                guard(path, sha(actual[name]))
        assert actual == files and directories == expected['directories'] and observed_links == symlinks, ('retained filesystem', directory)
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
        keys = {'command', 'elapsed_ns', 'returncode', 'stdout', 'stderr', 'files', 'directories'}
        assert set(capture) == keys | (set() if compute else {'symlinks'})
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
              for left, right in [('original', 'imre'), ('imre', 'rust'), ('original', 'rust')]}
    assert {key: value for key, value in row.items() if '_over_' in key} == ratios
    if compute and row['rounds']:
        assert row['amortized_seconds'] == {engine: seconds[engine] / row['rounds'] for engine in engines}
    return {'label': row['label'], 'kind': 'compute' if compute else 'process' if prepared_process else 'filesystem',
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


def original_commands(benchmark, prepared, paths, oracle_root, compute=False, filesystem=False, include_port=False):
    assert not (compute and filesystem)
    function = (benchmark['base'].commands_for if filesystem else
                benchmark['compute_commands_for' if compute else 'process_commands'])
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
    build = load(paths['imre_build_record'])
    assert build['revision'] == BUILD_REVISION and build['source_matches_revision'] and build['input_guards_passed']
    assert len(build['source_sha256']) == 74
    assert build['sha256']['compiler'] == hashes['compiler'] and build['sha256'][mode + '_runtime'] == hashes['runtime']
    for relative, digest in build['source_sha256'].items():
        assert sha(source(relative)) == sha(source(relative, BUILD_REVISION)) == digest
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


def check_validation_data(validator, process_api, ordinary, computed, processes, counts, prefix, engines,
                          artifact_identity=None):
    cases = validator.cases()
    assert len(cases) == ordinary['cases'] == len(ordinary['results'])
    assert ordinary['engine_executions'] == sum(len(engines) - int(not original) for _, _, original in cases)
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
    assert len(compute_cases) == computed['cases'] == len(computed['results']) == 12
    assert computed['engine_executions'] == len(compute_cases) * len(engines)
    assert tuple(computed['engines']) == engines
    for capture, case in zip(computed['results'], compute_cases, strict=True):
        assert capture['case'] == validator.case_record(case) and set(capture['engines']) == set(engines)
        expected = validator.compute_oracle(case)
        for engine, result in capture['engines'].items():
            check_capture(validator, case, expected, result, engine, compute=True)
            counts[prefix + '_compute_executions'] += 1
    assert tuple(processes['engines']) == engines
    assert processes['environment'] == process_api.environment()
    assert processes['producer_source'] == process_api.PRODUCER
    assert processes['producer_sha256'] == sha(process_api.PRODUCER.encode())
    (guard if artifact_identity is None else artifact_identity)(processes['python'], processes['python_sha256'])
    first = validator.case_from_record(processes['results'][0]['case'])
    flags = dict(zip(first.arguments[1::2], first.arguments[2::2], strict=True))
    producer = Path(json.loads(first.files['projects.json'])['projects'][0]['argv'][1])
    process_cases = list(process_api.validation_cases(Path(flags['--old-ty']), Path(flags['--new-ty']), producer))
    assert len(process_cases) == processes['cases'] == len(processes['results'])
    assert processes['engine_executions'] == sum(len(engines) - int(not row[2]) for row in process_cases)
    for capture, (case, error, original, events, state, controls) in zip(processes['results'], process_cases, strict=True):
        assert capture['case'] == validator.case_record(case) and capture['expected_error'] == error
        assert capture['events'] == json.loads(json.dumps(events)) and capture['capture_state'] == state and capture['child_controls'] == controls, case.label
        assert set(capture['engines']) == set(engines) - (set() if original else {'original'})
        for engine, result in capture['engines'].items():
            expected = process_api.oracle(case, error, events, state)
            process_api.apply_effect_witnesses(case, expected, result['effect_witnesses'])
            check_capture(validator, case, expected, result, engine, prepared_process=True)
            counts[prefix + '_process_executions'] += 1


def check_validation(validator, prepared, counts, process_api=None):
    ordinary, computed, processes = (load(prepared[key], prepared[key + '_sha256'])
                                     for key in ('validation', 'compute_validation', 'process_validation'))
    assert (prepared['validation_cases'], prepared['compute_validation_cases'], prepared['process_validation_cases']) == (ordinary['cases'], computed['cases'], processes['cases'])
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
    expected = [(description, real, False, False)]
    assert read_bytes(directory / 'real-projects.json') == real.files['projects.json']
    empty = [{'name': 'empty', 'old': validator.report(), 'new': validator.report()}]
    control = process_api.case('process-empty-control', empty, directory / 'capture-old', directory / 'capture-new')
    expected.append((benchmark['describe'](control, 'Two native child launches with empty schema-valid reports', empty), control, False, False))
    capture_control = replace(control, label='process-capture-empty-control',
                              arguments=(*control.arguments, '--capture-dir', 'captures'))
    expected.append((benchmark['describe'](capture_control,
                    'Two child launches, retained empty-report transcripts, and atomic Markdown publication', empty),
                    capture_control, False, False))
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
        expected.append((details, value, False, False))
        persisted = replace(value, label=f'persisted-capture-{count}x{copies}',
                            arguments=(*value.arguments, '--capture-dir', 'captures'))
        persisted_description = benchmark['describe'](persisted,
            'Fixed native child output; persisted decoded transcripts and atomic Markdown publication', projects)
        persisted_description['copies_per_project'] = copies
        expected.append((persisted_description, persisted, False, False))
        names = [project['name'] + '.json' for project in projects]
        filesystem = validator.directory_case(f'process-filesystem-{count}x{copies}',
                                              dict.fromkeys(names, before), dict.fromkeys(names, after))
        filesystem_expected = validator.oracle(filesystem)
        assert sha(filesystem_expected['rendered'].encode()) == details['expected_markdown_sha256']
        filesystem_description = {**details, 'label': filesystem.label, 'child_processes': 0,
                                  'arguments': list(filesystem.arguments),
                                  'captured_stdout_bytes': 0, 'captured_stderr_bytes': 0,
                                  'report_json_bytes': sum(len(data) for name, data in filesystem.files.items() if name.endswith('.json')),
                                  'recipe': 'Same reports read from directories; decode/compare/render and atomic publication without child launches'}
        expected.append((filesystem_description, filesystem, False, True))
        ordinary = validator.directory_case(f'process-compute-{count}x{copies}', dict.fromkeys(names, before), dict.fromkeys(names, after))
        for rounds in (0, 8):
            value = validator.compute_case(ordinary, rounds)
            oracle = validator.compute_oracle(value)
            metadata = {**details, 'label': value.label, 'child_processes': 0, 'rounds': rounds,
                        'captured_stdout_bytes': 0, 'captured_stderr_bytes': 0,
                        'arguments': list(value.arguments), 'recipe': 'Preloaded same reports; repeated decode/compare/render without children',
                        'expected_markdown_sha256': sha(oracle['rendered'].encode()), 'expected_markdown_bytes': len(oracle['rendered'].encode())}
            expected.append((metadata, value, True, False))
    assert len(expected) == len(record['workloads']) == 13
    for captured, (description, value, computed, filesystem) in zip(record['workloads'], expected, strict=True):
        assert captured['description'] == description and captured['compute'] is computed and captured['filesystem'] is filesystem
        assert not (computed and filesystem)
        compressed = read_bytes(Path(captured['case']))
        data = gzip.decompress(compressed)
        assert sha(compressed) == captured['sha256']
        assert json.loads(data) == validator.case_record(value)
        assert not any(name.endswith('/events.log') for name in value.files), ('append-only timed state', value.label)
        if not computed:
            oracle = validator.oracle(value) if filesystem else process_api.oracle(value)
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


def check_real_captures(benchmark, validator, prepared, paths, base_case, commands, counts,
                        read_bytes, local_directory, *, pointer=None, count_key='real_capture_executions'):
    """Replay actual-child witnesses; tee wrappers are correctness-only inputs."""
    if pointer is None:
        pointer = prepared['processes']['real_capture_validation']
    data = read_bytes(Path(pointer['record']))
    assert sha(data) == pointer['sha256']
    record = json.loads(data)
    root = Path(pointer['record']).parent
    assert record['timed'] is False and set(record['engines']) == set(commands)
    assert pointer['engine_executions'] == len(record['engines']) == len(commands)
    assert record['python'] == {'path': paths['python'], 'sha256': prepared['source_and_tool_sha256']['python']}
    expected_source = '#!' + paths['python'] + ' -I\n' + benchmark['TEE_SOURCE']
    assert record['wrapper_source'] == expected_source
    identities = {role: {'path': paths[role + '_ty'], 'sha256': prepared['source_and_tool_sha256'][role + '_ty']}
                  for role in ('old', 'new')}
    assert record['ty'] == identities and set(record['wrappers']) == {'old', 'new'}
    for role, item in record['wrappers'].items():
        assert item['path'] == str(root / ('ty-capture-' + role))
        assert item['sha256'] == sha(expected_source.encode())
        assert read_bytes(Path(item['path'])) == expected_source.encode()
    process_api = benchmark['process_api']
    for engine, item in record['engines'].items():
        directory = root / engine
        assert item['directory'] == str(directory)
        manifest = json.loads(base_case.files['projects.json'])
        for index, project in enumerate(manifest['projects']):
            assert project['argv'][0] == '{ty}'
            project['argv'] = ['{ty}', identities['old']['path'], identities['new']['path'],
                               str(directory / 'tee-raw' / ('project-' + str(index))), *project['argv'][1:]]
        arguments = list(base_case.arguments)
        for role in ('old', 'new'):
            arguments[arguments.index('--' + role + '-ty') + 1] = record['wrappers'][role]['path']
        case = replace(base_case, label='real-capture-' + engine,
                       files={**base_case.files, 'projects.json': json.dumps(manifest).encode()},
                       arguments=(*arguments, '--capture-dir', 'captures'))
        compressed = read_bytes(Path(item['case']))
        assert sha(compressed) == item['case_sha256']
        assert json.loads(gzip.decompress(compressed)) == validator.case_record(case)
        assert item['command'] == [*commands[engine], *case.arguments]
        local = local_directory(directory)
        expected, children = benchmark['real_capture_oracle'](case, local, identities)
        assert children == item['children']
        assert item['expected_markdown_sha256'] == sha(expected['rendered'].encode())
        assert read_bytes(root / (engine + '.stdout')).decode() == item['result']['stdout']
        assert read_bytes(root / (engine + '.stderr')).decode() == item['result']['stderr']
        check_capture(validator, case, expected, item['result'], engine,
                      prepared_process=True, directory=local)
        counts[count_key] += 1


def check_workload_gates(validator, process_api, prepared, cases, timing_directory, counts):
    captures = load(Path(prepared['processes']['directory']) / 'workload-validation.json')
    assert set(captures) == {case.label for _, case, _, _ in cases}
    for description, case, computed, filesystem in cases:
        expected = (validator.compute_oracle(case) if computed else
                    validator.oracle(case) if filesystem else process_api.oracle(case))
        assert set(captures[case.label]) == set(VALIDATION_ENGINES)
        hint = None if computed else (Path(timing_directory) / case.label / 'original' / 'report.md').read_text()
        for engine, capture in captures[case.label].items():
            check_capture(validator, case, expected, capture, engine, compute=computed,
                          prepared_process=not computed and not filesystem, rendered_hint=hint)
            counts['preparation_workload_executions'] += 1


def check_asan(benchmark, validator, process_api, prepared, cases, counts):
    directory = Path('/home/dev-user/.cache/imre-workspaces-20261021/asan-application')
    record = load(directory / 'record.json')
    assert record['revision'] == REVISION and record['input_guards_passed'] and record['status'] == 'passed'
    assert record['build_revision'] == BUILD_REVISION and record['compiler_runtime_sources_match']
    assert record['generated_code_instrumented'] and record['runtime_instrumented'] and record['outside_sandbox']
    assert record['environment'] == {'ASAN_OPTIONS': 'detect_leaks=1:halt_on_error=1', 'PYTHON_JIT': '0'}
    for name, digest in record['artifacts'].items():
        guard(directory / name, digest)
    assert set(record['artifacts']) == {path.relative_to(directory).as_posix()
        for path in directory.rglob('*') if path.is_file() and path != directory / 'record.json'}
    expected_hashes = {**prepared['source_and_tool_sha256'],
                       'asan_runtime': guard('/home/dev-user/.cache/imre-workspaces-20261021/build/asan_runtime.a'),
                       'prepared_record': guard('/home/dev-user/.cache/imre-workspaces-20261021/benchmark-normal-prepared.json')}
    assert record['source_and_tool_sha256'] == expected_hashes
    build = load(prepared['source_and_tool_paths']['imre_build_record'])
    assert build['sha256']['asan_runtime'] == expected_hashes['asan_runtime']
    step = next(step for step in build['steps'] if step['name'] == 'asan')
    assert step['overrides']['RUSTFLAGS'] == '-Zsanitizer=address -Cforce-frame-pointers=yes'
    assert '-Zsanitizer=address' in Path(step['log']).read_text() or 'Finished' in Path(step['log']).read_text()
    driver = (directory / 'driver.py').read_text()
    executable_paths = {name: re.search(r"^" + name.upper() + r" = '([^']+)'$", driver, re.M).group(1)
                        for name in ('python', 'clang')}
    for name, path in executable_paths.items():
        guard(path, prepared['source_and_tool_sha256'][name])
    commands = []
    for name, entry in [('memory-report', '__main__.py'), ('memory-compute', 'compute.py')]:
        commands.append(['/home/dev-user/.cache/imre-workspaces-20261021/build/imre', 'build', str(ROOT / 'examples/memory_report' / entry),
                         '--module-root', str(ROOT / 'examples'), '-o', str(directory / name),
                         '--runtime', '/home/dev-user/.cache/imre-workspaces-20261021/build/asan_runtime.a', '--clang', executable_paths['clang'],
                         '--opt-level', '1', '--sanitize', '--emit-llvm', str(directory / (name + '.ll'))])
        commands.append(['/usr/bin/nm', str(directory / name)])
        assert 'sanitize_address' in (directory / (name + '.ll')).read_text()
        binary = (directory / name).read_bytes()
        assert b'__asan_init' in binary and b'__asan_report_load' in binary
        assert (directory / (name + '-compile.log')).read_text() == ''
    commands.append(['/usr/bin/nm', '-u', '/home/dev-user/.cache/imre-workspaces-20261021/build/asan_runtime.a'])
    symbols = (directory / 'runtime-symbols.log').read_text()
    assert '__asan_report_load' in symbols and '__asan_report_store' in symbols
    commands.append([executable_paths['python'], str(ROOT / 'benchmarks/memory_validate.py'),
                     '--binary', str(directory / 'memory-report'), '--compute-binary', str(directory / 'memory-compute'),
                     '--rust', prepared['source_and_tool_paths']['rust'], '--python', executable_paths['python'],
                     '--output', str(directory / 'validation.json')])
    command_records = [row for row in record['commands'] if row['phase'] == 'asan-application']
    assert [row['command'] for row in command_records] == commands
    for row in record['commands']:
        guard(row['log'])
    assert record['commands'] == load(directory.parent / 'application-commands.json')
    pipeline = load(directory.parent / 'application-pipeline-status.json')
    assert pipeline['status'] == 'passed' and pipeline['phase'] == 'complete' and pipeline['revision'] == REVISION
    assert pipeline['build_revision'] == BUILD_REVISION and pipeline['compiler_runtime_sources_match']
    guard(directory.parent / 'application-driver.py', pipeline['driver_sha256'])
    assert pipeline['driver_sha256'] == record['artifacts']['driver.py']
    validation = load(directory / 'validation.json')
    check_validation_data(validator, process_api, validation, validation['compute'], validation['processes'], counts, 'asan', ASAN_ENGINES)
    gates = load(directory / 'workload-validation.json')
    assert set(gates) == {case.label for _, case, _, _ in cases}
    for description, case, computed, filesystem in cases:
        capture = gates[case.label]
        assert capture['case_sha256'] == next(item['sha256'] for item in prepared['processes']['workloads'] if item['description']['label'] == case.label)
        assert capture['command'] == [str(directory / ('memory-compute' if computed else 'memory-report')), *case.arguments]
        expected = (validator.compute_oracle(case) if computed else
                    validator.oracle(case) if filesystem else process_api.oracle(case))
        check_capture(validator, case, expected, capture, 'imre', compute=computed, prepared_process=not computed and not filesystem)
        counts['asan_workload_executions'] += 1
    assert (record['ordinary_cases'], record['ordinary_executions'], record['compute_cases'], record['compute_executions'],
            record['process_cases'], record['process_executions'], record['workload_cases']) == (
                validation['cases'], validation['engine_executions'], validation['compute']['cases'],
                validation['compute']['engine_executions'], validation['processes']['cases'],
                validation['processes']['engine_executions'], len(cases))
    assert 'AddressSanitizer' not in json.dumps(validation) and 'LeakSanitizer' not in json.dumps(validation)
    guard(directory.parent / 'application-pipeline.log')
    check_real_captures(benchmark, validator, prepared, prepared['source_and_tool_paths'], cases[0][1],
        {'imre': [str(directory / 'memory-report')]}, counts,
        guarded_bytes, lambda path: path,
        pointer=record['real_capture_validation'], count_key='asan_real_capture_executions')


def check_timing_command(command, prepared, timing_path, prepared_path, mode, artifact_identity=None):
    if artifact_identity is None:
        artifact_identity = guard
    paths = prepared['source_and_tool_paths']
    assert Path(command[0]).is_absolute() and command[1] == 'benchmarks/memory_process_benchmark.py'
    artifact_identity(command[0], prepared['source_and_tool_sha256']['python'])
    options, key = {}, None
    for argument in command[2:]:
        if argument.startswith('--'):
            assert argument not in options
            key = argument
            options[key] = []
        else:
            assert key is not None
            options[key].append(argument)
    expected = {'--rust-build-record': paths['rust_build_record'], '--imre-build-record': paths['imre_build_record'],
                '--ruff-source': '/home/dev-user/.cache/imre-workspaces-20261021/input-sources/ruff-scripts', '--artifact-dir': str(timing_path.with_suffix('')),
                '--output': str(timing_path), '--prepared-record': str(prepared_path), '--cpu': '31', '--samples': '48', '--warmups': '2'}
    tools = ['compiler', 'runtime', 'rust', 'python', 'clang', 'old_ty', 'new_ty']
    if mode == 'thinlto':
        tools.append('lto_linker')
    for name in tools:
        values = options.pop('--' + name.replace('_', '-'))
        assert len(values) == 1 and Path(values[0]).is_absolute()
        artifact_identity(values[0], prepared['source_and_tool_sha256'][name])
    for key, value in expected.items():
        assert options.pop(key) == [value], (key, command)
    permitted = {'--target': [prepared['effective_target']], '--compute-rounds': ['8'], '--process-layouts': ['64:1', '2:32']}
    assert all(value == permitted.get(key) for key, value in options.items())


def check_timing_context_data(context, read_json, read_bytes, bind):
    """Bind readiness and serial windows without trusting stored summary ratios."""
    cache = Path('/home/dev-user/.cache/imre-workspaces-20261021')
    assert context['revision'] == REVISION and context['cwd'] == str(ROOT)
    assert context['status'] == 'passed' and context['outside_tool_sandbox']
    assert context['cpu'] == 31 and 31 in context['available_cpu_affinity']
    assert context['machine'] == 'x86_64' and context['kernel'] and context['cpuinfo']['model name']
    ready_path = cache / 'timing-ready.json'
    ready_data = read_bytes(ready_path)
    assert sha(ready_data) == context['timing_ready_sha256']
    ready = json.loads(ready_data)
    assert ready == context['validation'] and ready['revision'] == REVISION
    assert ready['status'] == 'passed' and ready['all_validation_jobs_finished']
    bind(cache / 'timing-ready.py', ready['driver_sha256'])
    required = {str(cache / name) for name in (
        'build/build-record.json', 'validation/language/record.json',
        'asan-application/record.json', 'benchmark-normal-prepared.json',
        'benchmark-thinlto-prepared.json')}
    assert required <= ready['records'].keys()
    for path, digest in ready['records'].items():
        bind(path, digest)
    bind(cache / 'run-timings.py', context['driver_sha256'])
    assert context['commands'] == read_json(cache / 'timing-commands.json')
    assert tuple(context['commands']) == ('normal', 'thinlto')
    assert [row['mode'] for row in context['windows']] == ['normal', 'thinlto']
    assert [row['mode'] for row in context['idle_probes']] == ['normal', 'thinlto']
    previous = datetime.fromisoformat(context['started_at'])
    finished = datetime.fromisoformat(context['finished_at'])
    for window, idle in zip(context['windows'], context['idle_probes'], strict=True):
        assert window['command'] == context['commands'][window['mode']] and window['exit_code'] == 0
        started, completed = (datetime.fromisoformat(window[key]) for key in ('started_at', 'finished_at'))
        assert previous <= datetime.fromisoformat(idle['at']) <= started < completed <= finished
        previous = completed
        assert set(idle['probe']) == {'cpu', 'cpu31'}
        for probe in idle['probe'].values():
            values = probe['jiffies']
            assert len(values) >= 8 and all(type(value) is int and value >= 0 for value in values)
            assert sum(values[:8]) > 0
            fraction = (sum(values[:8]) - values[3] - values[4]) / sum(values[:8])
            assert probe['busy_fraction'] == fraction and 0 <= fraction <= 1
        assert idle['probe']['cpu31']['busy_fraction'] < 0.10
        log = read_bytes(window['log'])
        assert sha(log) == window['log_sha256']
        assert log.decode().rstrip().endswith('Saved ' + str(cache / ('timing-' + window['mode'] + '.json')))


def check_language_data(read_json, read_bytes, bind, source_bytes):
    """Verify all current unit/differential evidence through a byte-reader boundary."""
    def option(command, name):
        return command[command.index(name) + 1]

    cache = Path('/home/dev-user/.cache/imre-workspaces-20261021')
    build_path = cache / 'build/build-record.json'
    build = read_json(build_path)
    assert build['revision'] == BUILD_REVISION and build['source_matches_revision'] and build['input_guards_passed']
    assert len(build['source_sha256']) == 74
    for relative, digest in build['source_sha256'].items():
        assert sha(source_bytes(relative)) == digest
    for path, digest in build['driver_sha256'].items():
        bind(path, digest)
    precheck = read_json(cache / 'precheck-record.json')
    assert precheck == build['precheck'] and precheck['source_guard_passed'] and build['precheck_sources_match']
    assert [step['command'][2] if step['command'][0].endswith('/cargo.sh') else step['command'][1]
            for step in precheck['steps']] == ['fmt', 'clippy', 'test', 'diff']
    assert all(step['status'] == 0 and step['seconds'] > 0 for step in precheck['steps'])
    precheck_log = read_bytes(cache / 'precheck.log')
    assert sha(precheck_log) == build['precheck_log_sha256']
    assert read_bytes(cache / 'build/precheck.log') == precheck_log
    unit_results = re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed;', precheck_log.decode())
    assert unit_results and all(status == 'ok' and int(failed) == 0 for status, passed, failed in unit_results)
    unit_counts = [int(passed) for status, passed, failed in unit_results if int(passed)]
    assert len(unit_counts) == 2 and unit_counts[0] >= 138 and unit_counts[1] == 131
    assert read_json(cache / 'precheck-sources.json') == build['source_sha256']
    for step in build['steps']:
        assert step['seconds'] > 0 and 'Finished' in read_bytes(step['log']).decode()
    directory = cache / 'validation/language'
    language = read_json(directory / 'record.json')
    assert language['revision'] == REVISION and language['artifact_revision'] == BUILD_REVISION
    assert language['status'] == 'passed' and language['guards_passed'] and language['revision_unchanged']
    assert language['compiler_sources_match_measurement'] and language['compiler_input_count'] == 74
    assert language['validation_sources_match_revision']
    assert language['build_record'] == str(build_path) and language['cwd'] == str(ROOT)
    for path, digest in language['guarded_inputs'].items():
        bind(path, digest)
        path = Path(path)
        if path.is_relative_to(ROOT):
            assert sha(source_bytes(path.relative_to(ROOT).as_posix())) == digest
    driver_path = cache / 'run-language-validation.py'
    driver = read_bytes(driver_path)
    assert sha(driver) == language['guarded_inputs'][str(driver_path)]
    assert str(cache / 'resume-language-validation.py') in language['guarded_inputs']
    driver = driver.decode()
    suites = re.search(r"^SUITES = '([a-z0-9_ ]+)'\.split\(\)$", driver, re.M).group(1).split()
    affected = re.search(r"^AFFECTED = '([a-z0-9_ ]+)'\.split\(\)$", driver, re.M).group(1).split()
    assert len(suites) == len(set(suites)) == 34 and len(affected) == 6
    assert language['ordinary_suites'] == suites and language['affected_suites'] == affected
    assert language['ordinary_workers'] == language['affected_workers'] == 4
    assert language['environment_overrides'] == {'ASAN_OPTIONS': 'detect_leaks=1:halt_on_error=1',
                                                 'PYTHONDONTWRITEBYTECODE': '1'}
    assert set(language['results']) == set(language['commands']) == {'processes-retry', 'thinlto', 'asan'}

    prior = language['initial_validation']
    initial_data = read_bytes(prior['record'])
    assert sha(initial_data) == prior['record_sha256']
    initial = json.loads(initial_data)
    assert initial['revision'] == prior['revision'] == BUILD_REVISION and initial['status'] == 'failed'
    assert initial['validation_sources_match_revision']
    assert initial['ordinary_suites'] == suites and initial['affected_suites'] == affected
    assert initial['results'] == prior['results'] and set(initial['results']) == {'ordinary'}
    old_result = initial['results']['ordinary']
    assert old_result['returncode'] == 1 and old_result['command'] == initial['commands']['ordinary']
    old_log = read_bytes(old_result['log'])
    assert sha(old_log) == old_result['log_sha256']
    old_outcomes = re.findall(r'^(PASS|FAIL) ([a-z0-9_]+) \(', old_log.decode(), re.M)
    reused = [suite for suite in suites if suite != 'processes']
    assert len(old_outcomes) == 34 and {name for status, name in old_outcomes if status == 'PASS'} == set(reused)
    assert [name for status, name in old_outcomes if status == 'FAIL'] == ['processes']
    assert prior['reused_suites'] == reused and prior['failed_suite'] == 'processes'
    assert prior['failure_reason'] == 'Obsolete unsupported_mkdir rejection; mkdir is supported by this milestone'
    failure = read_bytes(prior['failed_log'])
    assert sha(failure) == prior['failed_log_sha256']
    assert b"AssertionError: ('unsupported_mkdir'," in failure
    old_process = read_bytes(prior['old_process_source'])
    assert sha(old_process) == prior['old_process_source_sha256']
    assert old_process == source_bytes('tests/processes.py', BUILD_REVISION)
    process_path = str(ROOT / 'tests/processes.py')
    new_process = source_bytes('tests/processes.py')
    removed = b'    "unsupported_mkdir": "Path(\'directory\').mkdir(parents=True, exist_ok=True)",\n'
    assert old_process.count(removed) == 1 and old_process.replace(removed, b'') == new_process
    assert sha(old_process) == initial['guarded_inputs'][process_path]
    changed = {path: {'before': digest, 'after': language['guarded_inputs'][path]}
               for path, digest in initial['guarded_inputs'].items()
               if digest != language['guarded_inputs'][path]}
    assert changed == {process_path: {'before': sha(old_process), 'after': sha(new_process)}}
    equivalence = language['reused_guard_equivalence']
    assert equivalence == {'original_input_count': len(initial['guarded_inputs']),
                           'unchanged_input_count': len(initial['guarded_inputs']) - 1,
                           'changed_inputs': changed, 'removed_line': removed.decode(),
                           'single_line_removal_verified': True}
    old_command = old_result['command']
    assert option(old_command, '--workers') == '4'
    assert old_command[old_command.index('--suites') + 1:old_command.index('--')] == suites
    assert option(old_command, '--compiler') == str(build_path.parent / 'imre')
    assert option(old_command, '--runtime') == str(build_path.parent / 'normal_runtime.a')

    expected_logs = {'ordinary': {}, 'affected': {}}
    records_to_count = []
    assert set(language['ordinary_suite_runs']) == set(suites)
    for suite in suites:
        run = language['ordinary_suite_runs'][suite]
        reused_run = suite != 'processes'
        assert run['status'] == 'passed' and run['reused'] is reused_run
        assert run['revision'] == (BUILD_REVISION if reused_run else REVISION)
        assert run['script'] == str(ROOT / 'tests' / (suite + '.py'))
        assert run['script_sha256'] == language['guarded_inputs'][run['script']]
        assert run['command_group'] == ('initial_validation.results.ordinary' if reused_run else 'results.processes-retry')
        if reused_run:
            assert run['script_sha256'] == initial['guarded_inputs'][run['script']]
            assert run['log'] == str(Path(option(old_command, '--log-dir')) / (suite + '.log'))
        else:
            retry = language['results']['processes-retry']
            assert run['log'] == retry['log'] and run['log_sha256'] == retry['log_sha256']
        data = read_bytes(run['log'])
        assert sha(data) == run['log_sha256']
        expected_logs['ordinary'][run['log']] = sha(data)
        records_to_count.append(('ordinary', suite, data))

    for mode, result in language['results'].items():
        assert result['returncode'] == 0 and result['seconds'] > 0
        assert datetime.fromisoformat(result['started_at']) < datetime.fromisoformat(result['finished_at'])
        command = result['command']
        assert command == language['commands'][mode]
        assert option(command, '--compiler') == str(build_path.parent / 'imre')
        runtime = 'normal' if mode == 'processes-retry' else mode
        assert option(command, '--runtime') == str(build_path.parent / (runtime + '_runtime.a'))
        data = read_bytes(result['log'])
        assert sha(data) == result['log_sha256']
        runner = data.decode()
        assert 'AddressSanitizer' not in runner and 'LeakSanitizer' not in runner
        if mode == 'processes-retry':
            assert command[1] == 'tests/processes.py'
            continue
        assert option(command, '--workers') == '4'
        assert command[command.index('--suites') + 1:command.index('--')] == affected
        assert f'Completed {len(affected)} suites; 0 failed.' in runner
        outcomes = re.findall(r'^(PASS|FAIL) ([a-z0-9_]+) \(', runner, re.M)
        assert len(outcomes) == 6 and all(status == 'PASS' for status, _ in outcomes)
        assert {name for _, name in outcomes} == set(affected)
        if mode == 'thinlto':
            assert '--lto-linker' in command and command[-2:] == ['0', '3']
        else:
            assert '--sanitize' in command and command[-2:] == ['0', '1']
        for suite in affected:
            path = Path(option(command, '--log-dir')) / (suite + '.log')
            data = read_bytes(path)
            expected_logs['affected'][str(path)] = sha(data)
            records_to_count.append((mode, suite, data))

    mode_counts, suite_counts = collections.Counter(), {}
    for mode, suite, data in records_to_count:
        log = data.decode()
        assert 'AddressSanitizer' not in log and 'LeakSanitizer' not in log
        if suite == 'base45':
            for level in (0, 3):
                assert f'O{level}: Passed 323 upstream native calls, 289 differential cases, and 2 compile-time type rejections' in log
        elif suite == 'luhn':
            for level in (0, 3):
                assert f'O{level}: imre: 15 upstream calls, 1144 differential cases' in log
        else:
            pattern = r'(\d+) import checks passed' if suite == 'imports' else r'Passed (\d+) '
            count = int(re.search(pattern, log).group(1))
            mode_counts[mode] += count
            suite_counts[mode + ':' + suite] = count
    assert language['ordinary_logs'] == expected_logs['ordinary']
    assert language['affected_logs'] == expected_logs['affected']
    return {'compiler_unit_tests': unit_counts[0], 'runtime_unit_tests': unit_counts[1],
            'compiler_build_revision': BUILD_REVISION, 'measurement_revision': REVISION,
            'equivalent_compiler_runtime_inputs': 74, 'ordinary_reused_suites': len(reused),
            'ordinary_retried_suites': ['processes'], 'initial_failed_suites': ['processes'],
            'ordinary_suites': len(suites), 'ordinary_general_checks': mode_counts['ordinary'],
            'base45_upstream_calls': 646, 'base45_differential_cases': 578, 'base45_type_rejections': 4,
            'luhn_upstream_calls': 30, 'luhn_differential_cases': 2288,
            'affected_checks': {mode: mode_counts[mode] for mode in ('thinlto', 'asan')},
            'reported_checks': suite_counts,
            'language_record_sha256': sha(read_bytes(directory / 'record.json'))}

def check_language_validation():
    def read_bytes(path):
        guard(path)
        return Path(path).read_bytes()
    return check_language_data(load, read_bytes, guard, source)

def main():
    assert re.fullmatch('[0-9a-f]{40}', REVISION), 'draft: bind the final source revision before running'
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--timings-complete', required=True, action='store_true')
    parser.add_argument('--output', type=Path, default=Path('/tmp/2026-10-21-native-workspaces-audit.json'))
    parser.add_argument('--context', type=Path, default=Path('/home/dev-user/.cache/imre-workspaces-20261021/timing-context.json'))
    args = parser.parse_args()
    guard(__file__)
    context = load(args.context)
    check_timing_context_data(context, load, guarded_bytes, guard)
    counts, modes, identities = collections.Counter(), {}, {}
    first = load('/home/dev-user/.cache/imre-workspaces-20261021/benchmark-normal-prepared.json')
    previous = datetime.fromisoformat(context['started_at'])
    with tempfile.TemporaryDirectory(prefix='imre-workspaces-data-audit-') as temporary:
        oracle_root = Path(temporary)
        benchmark, validator = reconstruct_oracle(first, oracle_root)
        process_api = benchmark['process_api']
        for mode in ('normal', 'thinlto'):
            timing_path = Path(f'/home/dev-user/.cache/imre-workspaces-20261021/timing-{mode}.json')
            timing = load(timing_path)
            prepared_path = Path(f'/home/dev-user/.cache/imre-workspaces-20261021/benchmark-{mode}-prepared.json')
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
            filesystem_commands = original_commands(benchmark, prepared, paths, oracle_root, filesystem=True)
            directory = timing_path.with_suffix('')
            process_rows = {row['label']: row for row in timing['workloads']}
            compute_rows = {row['label']: row for row in timing['compute_workloads']}
            filesystem_rows = {row['label']: row for row in timing['filesystem_workloads']}
            assert len(process_rows) == len(timing['workloads']) == 7
            assert len(compute_rows) == len(timing['compute_workloads']) == 4
            assert len(filesystem_rows) == len(timing['filesystem_workloads']) == 2
            assert {path.name for path in directory.iterdir()} == {case.label for _, case, _, _ in cases}
            measured = []
            for description, case, computed, filesystem in cases:
                row = (compute_rows if computed else filesystem_rows if filesystem else process_rows).pop(case.label)
                result = check_row(validator, row, description, case,
                                   compute_commands if computed else filesystem_commands if filesystem else commands,
                                   samples=48, warmups=2, compute=computed, prepared_process=not computed and not filesystem,
                                   process_api=process_api, directory=directory / case.label)
                counts.update(result['counts'])
                measured.append(result)
            assert not process_rows and not compute_rows and not filesystem_rows
            check_workload_gates(validator, process_api, prepared, cases, directory, counts)
            all_commands = original_commands(benchmark, prepared, paths, oracle_root, include_port=True)
            check_real_captures(benchmark, validator, prepared, paths, cases[0][1], all_commands, counts,
                                guarded_bytes, lambda path: path)
            if mode == 'normal':
                check_asan(benchmark, validator, process_api, prepared, cases, counts)
            control = next(row for row in measured if row['label'] == 'process-empty-control')
            capture_control = next(row for row in measured if row['label'] == 'process-capture-empty-control')
            for row in measured:
                if row['kind'] == 'process' and row is not control:
                    row['two_child_control_fraction'] = {engine: control['seconds'][engine] / row['seconds'][engine] for engine in ENGINES}
                if row['label'].startswith('persisted-capture-'):
                    row['persisted_capture_control_fraction'] = {engine: capture_control['seconds'][engine] / row['seconds'][engine] for engine in ENGINES}
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
    persisted = [row for mode in modes.values() for row in mode['rows'] if row['label'].startswith('persisted-capture-')]
    filesystem = [row for mode in modes.values() for row in mode['rows'] if row['kind'] == 'filesystem']
    assert len(processes) == len(compute) == 4 and len(real) == 2
    assert len(persisted) == len(filesystem) == 4
    assert counts['samples'] == 3744 and counts['warmups'] == 156 and counts['gates'] == 78
    result = {'schema': 1, 'status': 'passed', 'measurement_revision': REVISION,
              'compiler_build_revision': BUILD_REVISION, 'equivalent_compiler_runtime_inputs': 74,
              'scope': 'Independent replay of every process/compute/preload capture, retained filesystem, preparation/ASan result, balanced order, raw statistic, and Git/build/tool/source identity; no engine execution.',
              'counts': dict(counts), 'modes': modes, 'identities': identities, 'validation_suites': validation_suites,
              'all_captured_process_cases_at_most_twice_rust': all(row['imre_over_rust'] <= 2 for row in processes),
              'all_persisted_capture_cases_at_most_twice_rust': all(row['imre_over_rust'] <= 2 for row in persisted),
              'all_atomic_filesystem_cases_at_most_twice_rust': all(row['imre_over_rust'] <= 2 for row in filesystem),
              'all_repeated_compute_cases_at_most_twice_rust': all(row['imre_over_rust'] <= 2 for row in compute),
              'all_real_ty_cases_at_most_twice_rust': all(row['imre_over_rust'] <= 2 for row in real),
              'audit_script_sha256': INPUTS[str(Path(__file__).absolute())], 'guarded_inputs': INPUTS,
              'limits': 'Reuses independently reviewed frozen oracles. Captures cannot prove historical host isolation or instruction execution. Real-workflow timings include complete ty checking. Repeated-compute estimates include preload/startup/final output and subtract no control. Upstream ties are checked semantically using retained Markdown, then their exact bytes are bound to every capture.'}
    with args.output.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({'output': str(args.output), 'counts': dict(counts),
                      'process_imre_over_rust': [row['imre_over_rust'] for row in processes],
                      'compute_imre_over_rust': [row['imre_over_rust'] for row in compute]}, indent=2))


if __name__ == '__main__':
    main()
