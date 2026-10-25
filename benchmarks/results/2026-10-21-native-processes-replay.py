"""Check native-process evidence offline without Git, a checkout, or native engines.

Place this script beside the evidence archive, manifest and logical index. All
oracles and audit helpers come from hash-verified archived source. This checks
recorded outputs/statistics and source identities; it does not remeasure speed.
"""
import argparse
import collections
from datetime import datetime
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import re
import runpy
import sys
import tarfile
import tempfile
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit('replay requires assertions; do not use -O')
sys.dont_write_bytecode = True
PREFIX = '2026-10-21-native-processes'
REVISION = 'aca68f5631795f01c3cbb2dcdc56fd92866f6620'
ENGINES = ('imre', 'port', 'original', 'rust')
COMPUTE_ENGINES = ('imre', 'original', 'rust')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git_id(kind, data):
    return hashlib.sha1(kind.encode() + b' ' + str(len(data)).encode() + b'\0' + data).hexdigest()


def safe_name(name):
    path = PurePosixPath(name)
    assert not path.is_absolute() and '..' not in path.parts and str(path) == name and name != '.'
    return path


def native_kind(header):
    if header.startswith((b'\x7fELF', b'!<arch>\n', b'!<thin>\n', b'BC\xc0\xde', b'\xde\xc0\x17\x0b', b'MZ', b'\0asm')):
        return True
    if header[:4] in {b'\xfe\xed\xfa\xce', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xcf\xfa\xed\xfe',
                      b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca', b'\xca\xfe\xba\xbf', b'\xbf\xba\xfe\xca'}:
        return True
    magic = int.from_bytes(header[:2], 'little')
    return header[:4] == importlib.util.MAGIC_NUMBER or header[2:4] == b'\r\n' and (3000 <= magic < 4000 or magic == 62211)


def source_tree_id(entries):
    root = {}
    for name, item in entries.items():
        parts = safe_name(name).parts
        current = root
        for part in parts[:-1]:
            current = current.setdefault(part, {})
            assert isinstance(current, dict)
        assert parts[-1] not in current
        current[parts[-1]] = (item['mode'], item['git_blob'])
    def tree(node):
        result = bytearray()
        for name, item in sorted(node.items(), key=lambda pair: (pair[0] + ('/' if isinstance(pair[1], dict) else '')).encode()):
            if isinstance(item, dict):
                mode, value = '40000', tree(item)
            else:
                mode, value = format(0o100000 | item[0], 'o'), item[1]
            result.extend(mode.encode() + b' ' + name.encode() + b'\0' + bytes.fromhex(value))
        return git_id('tree', bytes(result))
    return tree(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, default=Path(__file__).with_name(PREFIX))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    prefix = args.prefix.absolute()
    manifest_data = Path(str(prefix) + '-evidence-manifest.json').read_bytes()
    manifest = json.loads(manifest_data)
    assert manifest['status'] == 'passed' and manifest['measurement_revision'] == REVISION
    assert manifest['replay_sha256'] == sha(Path(__file__).read_bytes()), 'replay script differs'
    archive_path = prefix.parent / manifest['archive']['name']
    assert archive_path.name == PREFIX + '-evidence.tar.gz'
    archive_bytes = archive_path.read_bytes()
    assert len(archive_bytes) == manifest['archive']['bytes'] and sha(archive_bytes) == manifest['archive']['sha256']
    index_data = Path(str(prefix) + '-evidence-index.json').read_bytes()
    assert sha(index_data) == manifest['logical_index_sha256']
    index = json.loads(index_data)
    assert index['source_roles'] == manifest['source_roles'] == {'measurement': REVISION}
    payloads = {}
    with tarfile.open(archive_path, 'r:gz') as archive:
        for member in archive:
            safe_name(member.name)
            assert member.isfile() and not member.issym() and not member.islnk() and member.name not in payloads
            assert member.mtime == member.uid == member.gid == 0 and member.uname == member.gname == ''
            data = archive.extractfile(member).read()
            expected = manifest['members'][member.name]
            assert len(data) == expected['bytes'] and sha(data) == expected['sha256'] and member.mode == expected['mode']
            assert not native_kind(data[:16]), member.name
            payloads[member.name] = data
    assert set(payloads) == set(manifest['members']) and len(payloads) == manifest['member_count']
    assert payloads['provenance/logical-index.json'] == index_data
    packaged_metadata = json.loads(payloads['provenance/package.json'])
    assert all(manifest[key] == value for key, value in packaged_metadata.items())
    exclusions = json.loads(payloads['provenance/exclusions.json'])
    assert len(exclusions) == len(index['excluded']) == manifest['excluded_artifact_count']
    for item in exclusions:
        assert {key: item[key] for key in ('sha256', 'bytes', 'reason')} == index['excluded'][item['path']]

    def blob(item, expected=None):
        data = payloads[item['member']]
        assert len(data) == item['bytes'] and sha(data) == item['sha256']
        assert expected is None or item['sha256'] == expected
        return data

    root = Path(manifest['repository'])
    source = index['sources'][REVISION]
    assert len(source) == manifest['logical_source_count']
    assert not any(name.startswith('benchmarks/results/') for name in source)
    omitted = index['omitted_historical_results'][REVISION]
    assert all(name.startswith('benchmarks/results/') for name in omitted) and not (source.keys() & omitted.keys())
    for name, item in source.items():
        safe_name(name)
        assert item['mode'] in (0o644, 0o755) and git_id('blob', blob(item)) == item['git_blob']
    tree_id = source_tree_id({**source, **omitted})
    assert tree_id == index['source_trees'][REVISION]
    commit = blob(index['source_commits'][REVISION])
    assert git_id('commit', commit) == REVISION and commit.splitlines()[0] == b'tree ' + tree_id.encode()
    assert len(index['files']) == manifest['logical_file_count']
    for path, item in index['files'].items():
        assert Path(path).is_absolute()
        blob(item)
    for item in index['captured_source_bindings']:
        assert item['revision'] == REVISION and str(root / item['relative']) == item['path']
        assert all(item[key] == source[item['relative']][key] for key in ('member', 'sha256', 'bytes', 'mode', 'git_blob'))
    for directory, entries in index['trees'].items():
        assert Path(directory).is_absolute()
        for name, is_directory in entries.items():
            safe_name(name)
            assert isinstance(is_directory, bool)
            path = str(Path(directory) / name)
            if not is_directory:
                assert path in index['files'] or path in index['excluded']

    def identity(path):
        name = str(path)
        if name in index['files']:
            return index['files'][name]
        if name in index['excluded']:
            return index['excluded'][name]
        return source[Path(path).relative_to(root).as_posix()]

    def retained(path, expected=None):
        item = identity(path)
        assert 'member' in item, ('native artifact intentionally excluded', path)
        return blob(item, expected)

    def load(path, expected=None):
        return json.loads(retained(path, expected))

    def retained_tree(directory):
        directory = Path(directory)
        if str(directory) in index['trees']:
            return index['trees'][str(directory)]
        ancestors = [(Path(name), entries) for name, entries in index['trees'].items()
                     if directory.is_relative_to(Path(name))]
        assert ancestors, ('no retained ancestor tree', directory)
        parent, entries = max(ancestors, key=lambda pair: len(pair[0].parts))
        relative = directory.relative_to(parent).as_posix()
        assert entries.get(relative) is True, ('retained path is not a directory', directory)
        prefix = relative + '/'
        return {name[len(prefix):]: is_directory for name, is_directory in entries.items()
                if name.startswith(prefix)}

    audit = load(manifest['audit']['path'], manifest['audit']['sha256'])
    assert audit['status'] == 'passed' and audit['measurement_revision'] == REVISION
    assert audit['audit_script_sha256'] == manifest['audit_script']['sha256']
    for path, expected in audit['guarded_inputs'].items():
        assert identity(path)['sha256'] == expected, ('audit identity', path)
    build = load(manifest['build']['path'], manifest['build']['sha256'])
    assert build['revision'] == REVISION and build['source_matches_revision'] and build['input_guards_passed']
    assert all(source[name]['sha256'] == value for name, value in build['source_sha256'].items())
    build_root = Path(manifest['build']['path']).parent
    for key, name in {'compiler': 'imre', 'normal_runtime': 'normal_runtime.a', 'thinlto_runtime': 'thinlto_runtime.a', 'asan_runtime': 'asan_runtime.a'}.items():
        assert identity(build_root / name)['sha256'] == build['sha256'][key]

    def bind(path, expected):
        assert identity(path)['sha256'] == expected, ('recorded identity', path)
        try:
            relative = Path(path).relative_to(root).as_posix()
        except ValueError:
            return
        assert source[relative]['sha256'] == expected, ('source differs from Git', relative)

    def option(command, name):
        return command[command.index(name) + 1]

    assert build['precheck_sources_match'] and build['precheck']['source_guard_passed']
    assert all(step['status'] == 0 for step in build['precheck']['steps'])
    precheck = retained('/tmp/imre-process-precheck.log', build['precheck_log_sha256']).decode()
    unit_results = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed;', precheck)
    assert unit_results and all(int(failed) == 0 for passed, failed in unit_results)
    assert sum(int(passed) for passed, failed in unit_results) == 261
    for step in build['steps']:
        assert 'Finished' in retained(step['log']).decode()

    validation_root = Path(manifest['language_validation_root'])
    language = load(validation_root / 'language/record.json')
    assert language['revision'] == REVISION and language['status'] == 'passed'
    assert language['guards_passed'] and language['revision_unchanged']
    for path, expected in language['guarded_inputs'].items():
        bind(path, expected)
    ci = blob(source['.github/workflows/ci.yml']).decode()
    ordinary_ci = ci.split('- name: Compare native suites with CPython', 1)[1].split('- name:', 1)[0]
    expected_suites = re.search(r'--suites (.*?) \\\n', ordinary_ci).group(1).split()
    assert language['ordinary_suites'] == expected_suites and len(expected_suites) == 33
    assert language['ordinary_workers'] == 4
    assert language['environment_overrides']['ASAN_OPTIONS'] == 'detect_leaks=1:halt_on_error=1'
    assert set(language['results']) == {'ordinary', 'process-thinlto', 'process-asan'}
    for name, result in language['results'].items():
        assert result['returncode'] == 0 and result['seconds'] > 0
        assert result['command'] == language['commands'][name]
        command = result['command']
        assert option(command, '--compiler') == str(build_root / 'imre')
        runtime = {'ordinary': 'normal', 'process-thinlto': 'thinlto', 'process-asan': 'asan'}[name]
        assert option(command, '--runtime') == str(build_root / (runtime + '_runtime.a'))
        log = retained(result['log'], result['log_sha256']).decode()
        if name == 'ordinary':
            assert command[command.index('--suites') + 1:command.index('--')] == expected_suites
            assert 'Completed 33 suites; 0 failed.' in log
        else:
            assert 'Passed 78 process checks' in log
            if name.endswith('asan'):
                assert '--sanitize' in command and command[-2:] == ['0', '1']
                assert 'ASan=True' in log
            else:
                assert '--lto-linker' in command and command[-2:] == ['0', '3']
    ordinary_log = retained(language['results']['ordinary']['log']).decode()
    suite_directory = Path(option(language['commands']['ordinary'], '--log-dir'))
    assert set(language['ordinary_logs']) == {str(suite_directory / (suite + '.log')) for suite in expected_suites}
    general_checks = 0
    for suite in expected_suites:
        path = str(suite_directory / (suite + '.log'))
        log = retained(path, language['ordinary_logs'][path]).decode()
        assert re.search(r'^PASS ' + re.escape(suite) + r' \(', ordinary_log, re.M)
        if suite == 'base45':
            for level in (0, 3):
                assert f'O{level}: Passed 323 upstream native calls, 289 differential cases, and 2 compile-time type rejections' in log
        elif suite == 'luhn':
            for level in (0, 3):
                assert f'O{level}: imre: 15 upstream calls, 1144 differential cases' in log
        elif suite == 'imports':
            general_checks += int(re.search(r'(\d+) import checks passed', log).group(1))
        else:
            general_checks += int(re.search(r'Passed (\d+) ', log).group(1))
    assert general_checks == 1961

    affected = load(manifest['affected_validation']['path'], manifest['affected_validation']['sha256'])
    assert affected['revision'] == REVISION and affected['status'] == 'passed'
    assert affected['inputs_verified_before'] and affected['inputs_verified_after'] and affected['outside_sandbox']
    assert affected['build_record_sha256'] == manifest['build']['sha256']
    assert affected['artifact_sha256'] == build['sha256']
    for path, value in affected['input_sha256'].items():
        bind(path, value)
    affected_counts = {}
    for row in affected['suites']:
        assert row['returncode'] == 0
        command = row['command']
        suites = command[command.index('--suites') + 1:command.index('--')]
        assert len(suites) == 7 and set(suites) == set(row['suites'])
        runner = retained(row['log'], row['log_sha256']).decode()
        directory = Path(option(command, '--log-dir'))
        for suite, item in row['suites'].items():
            log = retained(directory / (suite + '.log'), item['log_sha256']).decode()
            assert item['summary'] in log and item['summary'].startswith(f"Passed {item['checks']} ")
            assert re.search(r'^PASS ' + re.escape(suite) + r' \(', runner, re.M)
        affected_counts[row['mode']] = sum(item['checks'] for item in row['suites'].values())
    assert affected_counts == {'thinlto': 501, 'asan': 668}
    for probe in affected['probes']:
        assert probe['match'] and probe['captures']['native'] == probe['captures']['cpython']
        native = probe['commands']['native']
        if probe['case'] == 'raw_environment':
            bind(native[0], affected['raw_environment_driver_sha256'])
            native = native[1:]
        assert identity(native[0])['sha256'] == probe['binary_sha256']
        if probe['mode'] == 'asan':
            assert probe['generated_llvm_asan'] and probe['runtime_asan']

    context = load('/tmp/imre-process-timing-context.json')
    assert context['revision'] == REVISION and context['status'] == 'passed' and context['cpu'] == 31
    assert context['outside_tool_sandbox'] and context['cwd'] == str(root)
    assert 31 in context['available_cpu_affinity'] and context['idle_probe']['cpu31']['busy_fraction'] < 0.10
    for probe in context['idle_probe'].values():
        values = probe['jiffies']
        assert probe['busy_fraction'] == (sum(values[:8]) - values[3] - values[4]) / sum(values[:8])
    assert [row['mode'] for row in context['windows']] == ['normal', 'thinlto']
    assert all(row['exit_code'] == 0 for row in context['windows'])
    assert context['commands'] == load('/tmp/imre-process-timing-commands.json')
    for row in context['windows']:
        assert row['command'] == context['commands'][row['mode']]
        retained(row['log'])
    counts, modes = collections.Counter(), {}
    with tempfile.TemporaryDirectory(prefix='native-processes-portable-replay-') as temporary:
        temporary = Path(temporary)
        extracted = temporary / 'source'
        for name, item in source.items():
            path = extracted / safe_name(name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(blob(item))
        audit_file = temporary / 'audit.py'
        audit_file.write_bytes(retained(manifest['audit_script']['path'], manifest['audit_script']['sha256']))
        helpers = runpy.run_path(str(audit_file))
        helpers['original_commands'].__globals__['ROOT'] = root
        benchmark = runpy.run_path(str(extracted / 'benchmarks/memory_process_benchmark.py'))
        validator, process_api = benchmark['validator'], benchmark['process_api']
        check_capture, check_row = helpers['check_capture'], helpers['check_row']
        ordinary_cases = validator.cases()
        assert len(ordinary_cases) == 92
        selected = {'directory-common-projects', 'directory-dotfile-unicode-and-multiple-suffix',
                    'directory-stable-project-ties', 'directory-big-integers', 'directory-unchanged', 'directory-ignored-schema-fields'}
        compute_cases = [validator.compute_case(case, rounds) for case, error, original in ordinary_cases
                         if case.label in selected for rounds in (0, 2)]
        assert len(compute_cases) == 12

        def check_validation(ordinary, computation, processes, label):
            assert ordinary['cases'] == len(ordinary['results']) == 92 and ordinary['engine_executions'] == 335
            assert set(ordinary['engines']) == set(computation['engines']) == set(processes['engines']) == set(ENGINES)
            for capture, (case, error, original) in zip(ordinary['results'], ordinary_cases, strict=True):
                assert capture['case'] == validator.case_record(case) and capture['expected_error'] == error
                assert set(capture['engines']) == set(ENGINES) - (set() if original else {'original'})
                for engine, result in capture['engines'].items():
                    check_capture(validator, case, validator.oracle(case, error), result, engine)
                    counts[label + '_application_executions'] += 1
            assert computation['cases'] == len(computation['results']) == 12 and computation['engine_executions'] == 48
            for capture, case in zip(computation['results'], compute_cases, strict=True):
                assert capture['case'] == validator.case_record(case) and set(capture['engines']) == set(ENGINES)
                for engine, result in capture['engines'].items():
                    check_capture(validator, case, validator.compute_oracle(case), result, engine, compute=True)
                    counts[label + '_compute_executions'] += 1
            assert processes['cases'] == len(processes['results']) == 32 and processes['engine_executions'] == 127
            assert processes['producer_source'] == process_api.PRODUCER
            assert sha(processes['producer_source'].encode()) == processes['producer_sha256']
            bind(processes['python'], processes['python_sha256'])
            assert processes['environment'] == {
                'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'LC_ALL': 'C.UTF-8', 'PYTHONHASHSEED': '0',
                'PYTHON_JIT': '0', 'PYTHONPATH': '', 'IMRE_PROCESS_SENTINEL': 'inherited',
                'TY_MEMORY_REPORT': 'overridden', 'TY_MAX_PARALLELISM': 'overridden',
                'ASAN_OPTIONS': 'detect_leaks=1:halt_on_error=1'}
            first = validator.case_from_record(processes['results'][0]['case'])
            flags = dict(zip(first.arguments[1::2], first.arguments[2::2], strict=True))
            producer = Path(json.loads(first.files['projects.json'])['projects'][0]['argv'][1])
            cases = list(process_api.cases(Path(flags['--old-ty']), Path(flags['--new-ty']), producer))
            assert len(cases) == 32
            for capture, (case, error, original, events) in zip(processes['results'], cases, strict=True):
                assert capture['case'] == validator.case_record(case) and capture['expected_error'] == error
                assert set(capture['engines']) == set(ENGINES) - (set() if original else {'original'})
                for engine, result in capture['engines'].items():
                    check_capture(validator, case, process_api.oracle(case, error, events), result, engine, prepared_process=True)
                    counts[label + '_process_executions'] += 1

        def restore_tree(directory, destination):
            entries = retained_tree(directory)
            destination.mkdir(parents=True, exist_ok=True)
            for name, is_directory in entries.items():
                path = destination / safe_name(name)
                if is_directory:
                    path.mkdir(parents=True, exist_ok=True)
                else:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(retained(directory / name))
            return entries

        previous = datetime.fromisoformat(context['started_at'])
        mode_cases = {}
        for mode in ('normal', 'thinlto'):
            timing = load(manifest['timings'][mode]['path'], manifest['timings'][mode]['sha256'])
            prepared = load(manifest['preparations'][mode]['path'], manifest['preparations'][mode]['sha256'])
            assert timing['preparation'] == prepared and timing['preparation_record_sha256'] == manifest['preparations'][mode]['sha256']
            assert timing['revision'] == prepared['revision'] == REVISION and not timing['dirty'] and not prepared['dirty']
            assert timing['input_guards_passed'] and prepared['input_guards_passed'] and prepared['thinlto'] == (mode == 'thinlto')
            assert timing['samples'] == 48 and timing['warmups'] == 2
            helpers['check_timing_command'](context['commands'][mode], prepared, Path(manifest['timings'][mode]['path']), Path(manifest['preparations'][mode]['path']), mode)
            assert timing['cpu']['affinity'] == [31] and timing['python']['version_number'] == '3.14.5'
            python = timing['python']
            assert python['implementation'] == 'cpython' and python['gil_enabled']
            assert not any(python[key] for key in ('free_threaded_build', 'jit_enabled', 'debug_build'))
            assert all(python['pgo'].values()) and python['lto']['lto_flag_present']
            assert python['lto']['configure_option'] == '--with-lto'
            assert prepared['processes']['process_layouts'] == [[64, 1], [2, 32]]
            assert prepared['processes']['compute_rounds'] == 8 and prepared['processes']['input_guards_passed']
            started, completed = (datetime.fromisoformat(timing[key]) for key in ('started_at_utc', 'completed_at_utc'))
            assert previous <= started < completed <= datetime.fromisoformat(context['finished_at'])
            assert datetime.fromisoformat(prepared['completed_at_utc']) <= started
            window = next(row for row in context['windows'] if row['mode'] == mode)
            assert datetime.fromisoformat(window['started_at']) <= started < completed <= datetime.fromisoformat(window['finished_at'])
            previous = completed
            paths = prepared['source_and_tool_paths']
            assert paths.keys() == prepared['source_and_tool_sha256'].keys()
            for name, path in paths.items():
                bind(path, prepared['source_and_tool_sha256'][name])
            assert prepared['source_and_tool_sha256']['compiler'] == build['sha256']['compiler']
            assert prepared['source_and_tool_sha256']['runtime'] == build['sha256'][mode + '_runtime']
            assert prepared['effective_target'] == 'x86_64-unknown-linux-gnu'
            assert prepared['native_elf'] == {'bits': 64, 'byte_order': 'little', 'machine': 62}
            for entry, binary, llvm, key in [('__main__.py', 'binary', 'llvm', 'compile_command'), ('compute.py', 'compute_binary', 'compute_llvm', 'compute_compile_command')]:
                expected = [paths['compiler'], 'build', str(root / 'examples/memory_report' / entry), '--module-root', str(root / 'examples'), '-o', prepared[binary], '--runtime', paths['runtime'], '--clang', paths['clang'], '--opt-level', '3', '--emit-llvm', prepared[llvm], '--target', prepared['effective_target']]
                if mode == 'thinlto':
                    expected += ['--lto-linker', paths['lto_linker']]
                assert prepared[key] == expected
            for name in ('binary', 'llvm', 'validation', 'compute_binary', 'compute_llvm', 'compute_validation', 'process_validation'):
                assert identity(prepared[name])['sha256'] == prepared[name + '_sha256']
            rust = load(paths['rust_build_record'])
            assert rust == prepared['rust_build_record'] and rust['input_guards_passed'] and rust['thinlto'] == (mode == 'thinlto')
            assert rust['binary_sha256'] == prepared['source_and_tool_sha256']['rust']
            assert rust['rustc_verbose'].strip() == build['toolchain'].strip()
            assert rust['llvm_budget'] == (1000 if mode == 'thinlto' else None)
            for name, value in {'CARGO_PROFILE_RELEASE_OPT_LEVEL': '3', 'CARGO_PROFILE_RELEASE_LTO': 'thin' if mode == 'thinlto' else 'off', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS': '16'}.items():
                assert rust['environment'][name] == value
            for kind, key in [('reference', 'rust'), ('producer', 'producer')]:
                item = rust['binaries'][kind]
                assert Path(paths[key]).name == item['name']
                assert identity(paths[key])['sha256'] == item['sha256'] and identity(paths[key])['bytes'] == item['bytes']
            for name, value in rust['tool_sha256'].items():
                bind(Path('/home/dev-user/.local/share/ohm/toolchains/ohm-1.98.1-1/bin') / name, value)
            for name, value in rust['sources'].items():
                data = blob(source['benchmarks/reference/memory_report/' + name])
                assert sha(data) == value and rust['source_contents'][name] == data.decode()
            process_inputs = prepared['processes']
            for path, value in process_inputs['files'].items():
                bind(path, value)
            input_root = Path(process_inputs['directory'])
            input_tree = retained_tree(input_root)
            assert {str(input_root / path) for path, is_directory in input_tree.items() if not is_directory} == set(process_inputs['files'])
            assert sorted(str(input_root / path) for path, is_directory in input_tree.items() if is_directory) == process_inputs['directories']
            check_validation(load(prepared['validation']), load(prepared['compute_validation']), load(prepared['process_validation']), 'preparation')
            commands = helpers['original_commands'](benchmark, prepared, paths, extracted)
            compute_commands = helpers['original_commands'](benchmark, prepared, paths, extracted, compute=True)
            assert set(commands) == set(compute_commands) == set(COMPUTE_ENGINES)
            entries = process_inputs['workloads']
            assert len(entries) == 8 and sum(item['compute'] for item in entries) == 4
            reconstructed = helpers['reconstruct_workload_cases'](
                benchmark, validator, prepared, paths, retained, lambda name: blob(source[name]),
                artifact_sha256=lambda path: identity(path)['sha256'])
            cases = {case.label: (item, case) for item, (description, case, computed) in zip(entries, reconstructed, strict=True)}
            assert len(cases) == 8
            mode_cases[mode] = cases
            rows = [*timing['workloads'], *timing['compute_workloads']]
            assert len(timing['workloads']) == len(timing['compute_workloads']) == 4 and len(rows) == 8
            assert {row['label'] for row in rows} == set(cases)
            directory = Path(manifest['timings'][mode]['path']).with_suffix('')
            restored = temporary / mode
            tree_entries = restore_tree(directory, restored)
            assert {PurePosixPath(name).parts[0] for name in tree_entries} == set(cases)
            measured = []
            rows_by_label = {row['label']: row for row in rows}
            for description, case, compute in reconstructed:
                row = rows_by_label.pop(case.label)
                item, _ = cases[row['label']]
                assert description['label'] == case.label and description['arguments'] == list(case.arguments)
                assert compute is item['compute']
                expected = validator.compute_oracle(case) if compute else process_api.oracle(case)
                markdown = expected['rendered']
                assert sha(markdown.encode()) == description['expected_markdown_sha256']
                assert len(markdown.encode()) == description['expected_markdown_bytes']
                result = check_row(validator, row, description, case, compute_commands if compute else commands,
                                   samples=48, warmups=2, compute=compute, prepared_process=not compute,
                                   process_api=process_api, directory=restored / row['label'])
                counts.update(result['counts'])
                if compute:
                    rounds = description['rounds']
                    assert rounds in (0, 8) and str(rounds) == case.arguments[0] and description['child_processes'] == 0
                    if rounds:
                        amortized = {engine: seconds / rounds for engine, seconds in result['seconds'].items()}
                        assert row['amortized_seconds'] == amortized
                        result['amortized_seconds'] = amortized
                else:
                    assert description['child_processes'] == 2 * description['projects']
                measured.append(result)
                assert set(row['gates']) == set(COMPUTE_ENGINES)
                for engine, capture in row['gates'].items():
                    relative = row['label'] + '/' + engine + '/'
                    actual = {name[len(relative):]: retained(directory / name) for name, is_directory in tree_entries.items()
                              if name.startswith(relative) and not is_directory}
                    directories = sorted(name[len(relative):] for name, is_directory in tree_entries.items()
                                         if name.startswith(relative) and is_directory)
                    assert helpers['encoded'](actual) == capture['files'] and directories == capture['directories']
            assert not rows_by_label
            control = next(row for row in measured if row['label'] == 'process-empty-control')
            for row in measured:
                if row['kind'] == 'process' and row is not control:
                    row['two_child_control_fraction'] = {engine: control['seconds'][engine] / row['seconds'][engine] for engine in COMPUTE_ENGINES}
                if row['kind'] == 'compute' and row['rounds']:
                    preload = next(other for other in measured if other['label'] == row['label'].removesuffix('8') + '0')
                    row['preload_control_fraction'] = {engine: preload['seconds'][engine] / row['seconds'][engine] for engine in COMPUTE_ENGINES}
            modes[mode] = {'rows': measured}
            assert modes[mode] == audit['modes'][mode]
            preparation_gates = load(input_root / 'workload-validation.json')
            assert set(preparation_gates) == set(cases)
            for description, case, computed in reconstructed:
                expected = validator.compute_oracle(case) if computed else process_api.oracle(case)
                hint = None if computed else retained(directory / case.label / 'original' / 'report.md').decode()
                assert set(preparation_gates[case.label]) == set(ENGINES)
                for engine, capture in preparation_gates[case.label].items():
                    check_capture(validator, case, expected, capture, engine, compute=computed, prepared_process=not computed, rendered_hint=hint)
                    counts['preparation_workload_executions'] += 1
            sidecar = Path(str(prefix) + '-' + mode + '.json.gz')
            if sidecar.exists():
                assert gzip.decompress(sidecar.read_bytes()) == retained(manifest['timings'][mode]['path'])

        asan_path = Path(manifest['asan_application']['preparation']['path'])
        asan_prep = load(asan_path, manifest['asan_application']['preparation']['sha256'])
        assert asan_prep['revision'] == REVISION and asan_prep['input_guards_passed'] and asan_prep['outside_sandbox']
        assert asan_prep['generated_code_instrumented'] and asan_prep['runtime_instrumented']
        assert asan_prep['environment']['ASAN_OPTIONS'] == 'detect_leaks=1:halt_on_error=1'
        for name, value in asan_prep['artifacts'].items():
            assert identity(asan_path.parent / name)['sha256'] == value
        for command in asan_prep['commands']:
            if len(command) > 1 and command[1] == 'build':
                assert command[0] == str(build_root / 'imre') and '--sanitize' in command
                assert option(command, '--runtime') == str(build_root / 'asan_runtime.a')
                assert option(command, '--opt-level') == '1'
        sanitizer_step = next(step for step in build['steps'] if step['name'] == 'asan')
        assert sanitizer_step['overrides']['RUSTFLAGS'] == '-Zsanitizer=address -Cforce-frame-pointers=yes'
        assert b'__asan_report_load' in retained(asan_path.parent / 'runtime-symbols.log')
        assert b'__asan_report_store' in retained(asan_path.parent / 'runtime-symbols.log')
        for name in ('memory-report', 'memory-compute'):
            assert b'sanitize_address' in retained(asan_path.parent / (name + '.ll'))
            assert b'__asan_' in retained(asan_path.parent / (name + '-symbols.log'))
        asan = load(manifest['asan_application']['validation']['path'], manifest['asan_application']['validation']['sha256'])
        check_validation(asan, asan['compute'], asan['processes'], 'asan')
        workloads = load(asan_path.parent / 'workload-validation.json')
        assert set(workloads) == set(mode_cases['normal'])
        for label, capture in workloads.items():
            item, case = mode_cases['normal'][label]
            assert capture['case_sha256'] == item['sha256']
            compute = item['compute']
            expected = validator.compute_oracle(case) if compute else process_api.oracle(case)
            assert capture['command'] == [str(asan_path.parent / ('memory-compute' if compute else 'memory-report')), *case.arguments]
            check_capture(validator, case, expected, capture, 'imre', compute=compute, prepared_process=not compute)
            counts['asan_workload_executions'] += 1
        assert {key: counts[key] for key in audit['counts']} == audit['counts']

    result = {'schema': 1, 'status': 'passed', 'measurement_revision': REVISION,
              'archive_sha256': manifest['archive']['sha256'], 'manifest_sha256': sha(manifest_data),
              'source_tree': tree_id, 'source_files': len(source), 'omitted_historical_result_files': len(omitted),
              'retained_logical_files': len(index['files']), 'excluded_native_artifacts': len(index['excluded']),
              'rust_unit_tests': 261, 'normal_suites': 33, 'normal_reported_checks': general_checks,
              'base45': {'upstream_calls': 646, 'differential_cases': 578, 'type_rejections': 4},
              'luhn': {'upstream_calls': 30, 'differential_cases': 2288},
              'affected_reported_checks': affected_counts, 'process_checks_per_extra_mode': 78,
              'counts': dict(counts), 'modes': modes,
              'limits': 'Data-only replay. Hashes bind archived source, captures and recorded external artifact identities; absent native tools/binaries cannot be rehashed here. No native execution, timing remeasurement, timestamp authentication, or proof of historical host isolation. Repeated-compute timings retain preload/startup/final-output costs and subtract no controls.'}
    data = (json.dumps(result, indent=2) + '\n').encode()
    if args.output:
        with args.output.open('xb') as stream:
            stream.write(data)
    print(json.dumps({key: value for key, value in result.items() if key != 'modes'}, indent=2))


if __name__ == '__main__':
    main()
