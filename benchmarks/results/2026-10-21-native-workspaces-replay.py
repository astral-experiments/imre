"""Check native-workspace evidence offline without Git, a checkout, or native engines.

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
PREFIX = '2026-10-21-native-workspaces'
REVISION = '7c5c426f9a890b8a805ffd1155ffd3371673ed04'
BUILD_REVISION = '2e0eac85ad0eea462be9c4feeffa76497773822c'
ENGINES = ('relar', 'port', 'original', 'rust')
COMPUTE_ENGINES = ('relar', 'original', 'rust')


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
    assert re.fullmatch('[0-9a-f]{40}', REVISION), 'draft: bind the final source revision before running'
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
    assert index['source_roles'] == manifest['source_roles'] == {
        'measurement': REVISION, 'compiler_build': BUILD_REVISION}
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
    assert build['revision'] == BUILD_REVISION and build['source_matches_revision'] and build['input_guards_passed']
    assert len(build['source_sha256']) == 74
    assert all(source[name]['sha256'] == value for name, value in build['source_sha256'].items())
    original_entries = index['build_source_entries']
    original_tree = source_tree_id(original_entries)
    assert original_tree == index['source_trees'][BUILD_REVISION]
    original_commit = blob(index['source_commits'][BUILD_REVISION])
    assert git_id('commit', original_commit) == BUILD_REVISION
    assert original_commit.splitlines()[0] == b'tree ' + original_tree.encode()
    for name in build['source_sha256']:
        assert original_entries[name] == {key: source[name][key] for key in ('mode', 'git_blob')}
    build_root = Path(manifest['build']['path']).parent
    for key, name in {'compiler': 'relar', 'normal_runtime': 'normal_runtime.a', 'thinlto_runtime': 'thinlto_runtime.a', 'asan_runtime': 'asan_runtime.a'}.items():
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

    # Load only the verified data-checking helpers; their CLI never runs.
    helpers = {'__name__': 'native_workspace_evidence_helpers',
               '__file__': manifest['audit_script']['path']}
    exec(compile(retained(manifest['audit_script']['path'], manifest['audit_script']['sha256']),
                 'archived-audit.py', 'exec'), helpers)
    helpers['ROOT'] = root
    assert helpers['REVISION'] == REVISION
    assert helpers['BUILD_REVISION'] == BUILD_REVISION
    def source_bytes(name, revision=REVISION):
        if revision == REVISION:
            return blob(source[str(name)])
        assert revision == BUILD_REVISION and name == 'tests/processes.py'
        data = retained('/home/dev-user/.cache/relar-workspaces-20261021/validation/language/initial-processes.py')
        assert git_id('blob', data) == original_entries[name]['git_blob']
        return data
    validation_suites = helpers['check_language_data'](
        load, retained, bind, source_bytes)
    assert validation_suites == audit['validation_suites']
    general_checks = validation_suites['ordinary_general_checks']
    affected_counts = validation_suites['affected_checks']

    context = load('/home/dev-user/.cache/relar-workspaces-20261021/timing-context.json')
    helpers['check_timing_context_data'](context, load, retained, bind)
    counts, modes = collections.Counter(), {}
    with tempfile.TemporaryDirectory(prefix='native-workspaces-portable-replay-') as temporary:
        temporary = Path(temporary)
        extracted = temporary / 'source'
        for name, item in source.items():
            path = extracted / safe_name(name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(blob(item))
        benchmark = runpy.run_path(str(extracted / 'benchmarks/memory_process_benchmark.py'))
        validator, process_api = benchmark['validator'], benchmark['process_api']
        check_capture, check_row = helpers['check_capture'], helpers['check_row']

        def check_validation(ordinary, computation, processes, label):
            assert set(ordinary['engines']) == set(computation['engines']) == set(processes['engines']) == set(ENGINES)
            helpers['check_validation_data'](validator, process_api, ordinary, computation,
                processes, counts, label, tuple(ordinary['engines']), artifact_identity=bind)

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
            helpers['check_timing_command'](context['commands'][mode], prepared, Path(manifest['timings'][mode]['path']), Path(manifest['preparations'][mode]['path']), mode, artifact_identity=bind)
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
            filesystem_commands = helpers['original_commands'](benchmark, prepared, paths, extracted, filesystem=True)
            assert set(commands) == set(compute_commands) == set(filesystem_commands) == set(COMPUTE_ENGINES)
            entries = process_inputs['workloads']
            assert len(entries) == 13 and sum(item['compute'] for item in entries) == 4
            assert sum(item['filesystem'] for item in entries) == 2
            reconstructed = helpers['reconstruct_workload_cases'](
                benchmark, validator, prepared, paths, retained, lambda name: blob(source[name]),
                artifact_sha256=lambda path: identity(path)['sha256'])
            cases = {case.label: (item, case) for item, (description, case, computed, filesystem) in zip(entries, reconstructed, strict=True)}
            assert len(cases) == 13
            mode_cases[mode] = cases
            rows = [*timing['workloads'], *timing['compute_workloads'], *timing['filesystem_workloads']]
            assert len(timing['workloads']) == 7 and len(timing['compute_workloads']) == 4
            assert len(timing['filesystem_workloads']) == 2 and len(rows) == 13
            assert {row['label'] for row in rows} == set(cases)
            directory = Path(manifest['timings'][mode]['path']).with_suffix('')
            restored = temporary / mode
            tree_entries = restore_tree(directory, restored)
            assert {PurePosixPath(name).parts[0] for name in tree_entries} == set(cases)
            measured = []
            rows_by_label = {row['label']: row for row in rows}
            for description, case, compute, filesystem in reconstructed:
                row = rows_by_label.pop(case.label)
                item, _ = cases[row['label']]
                assert description['label'] == case.label and description['arguments'] == list(case.arguments)
                assert compute is item['compute'] and filesystem is item['filesystem']
                expected = (validator.compute_oracle(case) if compute else
                            validator.oracle(case) if filesystem else process_api.oracle(case))
                markdown = expected['rendered']
                assert sha(markdown.encode()) == description['expected_markdown_sha256']
                assert len(markdown.encode()) == description['expected_markdown_bytes']
                result = check_row(validator, row, description, case,
                                   compute_commands if compute else filesystem_commands if filesystem else commands,
                                   samples=48, warmups=2, compute=compute, prepared_process=not compute and not filesystem,
                                   process_api=process_api, directory=restored / row['label'])
                counts.update(result['counts'])
                if compute:
                    rounds = description['rounds']
                    assert rounds in (0, 8) and str(rounds) == case.arguments[0] and description['child_processes'] == 0
                    if rounds:
                        amortized = {engine: seconds / rounds for engine, seconds in result['seconds'].items()}
                        assert row['amortized_seconds'] == amortized
                        result['amortized_seconds'] = amortized
                elif filesystem:
                    assert description['child_processes'] == 0
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
            capture_control = next(row for row in measured if row['label'] == 'process-capture-empty-control')
            for row in measured:
                if row['kind'] == 'process' and row is not control:
                    row['two_child_control_fraction'] = {engine: control['seconds'][engine] / row['seconds'][engine] for engine in COMPUTE_ENGINES}
                if row['label'].startswith('persisted-capture-'):
                    row['persisted_capture_control_fraction'] = {engine: capture_control['seconds'][engine] / row['seconds'][engine] for engine in COMPUTE_ENGINES}
                if row['kind'] == 'compute' and row['rounds']:
                    preload = next(other for other in measured if other['label'] == row['label'].removesuffix('8') + '0')
                    row['preload_control_fraction'] = {engine: preload['seconds'][engine] / row['seconds'][engine] for engine in COMPUTE_ENGINES}
            modes[mode] = {'rows': measured}
            assert modes[mode] == audit['modes'][mode]
            preparation_gates = load(input_root / 'workload-validation.json')
            assert set(preparation_gates) == set(cases)
            for description, case, computed, filesystem in reconstructed:
                expected = (validator.compute_oracle(case) if computed else
                            validator.oracle(case) if filesystem else process_api.oracle(case))
                hint = None if computed else retained(directory / case.label / 'original' / 'report.md').decode()
                assert set(preparation_gates[case.label]) == set(ENGINES)
                for engine, capture in preparation_gates[case.label].items():
                    check_capture(validator, case, expected, capture, engine, compute=computed,
                                  prepared_process=not computed and not filesystem, rendered_hint=hint)
                    counts['preparation_workload_executions'] += 1
            real_root = Path(process_inputs['real_capture_validation']['record']).parent
            local_real_root = temporary / (mode + '-real-captures')
            restore_tree(real_root, local_real_root)
            all_commands = helpers['original_commands'](benchmark, prepared, paths, extracted, include_port=True)
            helpers['check_real_captures'](benchmark, validator, prepared, paths, reconstructed[0][1],
                all_commands, counts, retained,
                lambda path: local_real_root / Path(path).relative_to(real_root))
            sidecar = Path(str(prefix) + '-' + mode + '.json.gz')
            if sidecar.exists():
                assert gzip.decompress(sidecar.read_bytes()) == retained(manifest['timings'][mode]['path'])

        asan_path = Path(manifest['asan_application']['preparation']['path'])
        asan_prep = load(asan_path, manifest['asan_application']['preparation']['sha256'])
        assert asan_prep['revision'] == REVISION and asan_prep['input_guards_passed'] and asan_prep['outside_sandbox']
        assert asan_prep['status'] == 'passed'
        assert asan_prep['build_revision'] == BUILD_REVISION and asan_prep['compiler_runtime_sources_match']
        assert asan_prep['generated_code_instrumented'] and asan_prep['runtime_instrumented']
        assert asan_prep['environment']['ASAN_OPTIONS'] == 'detect_leaks=1:halt_on_error=1'
        for name, value in asan_prep['artifacts'].items():
            assert identity(asan_path.parent / name)['sha256'] == value
        for row in asan_prep['commands']:
            command = row['command']
            retained(row['log'])
            if len(command) > 1 and command[1] == 'build':
                assert command[0] == str(build_root / 'relar') and '--sanitize' in command
                assert option(command, '--runtime') == str(build_root / 'asan_runtime.a')
                assert option(command, '--opt-level') == '1'
        cache = asan_path.parent.parent
        assert asan_prep['commands'] == load(cache / 'application-commands.json')
        pipeline = load(cache / 'application-pipeline-status.json')
        assert pipeline['revision'] == REVISION and pipeline['status'] == 'passed' and pipeline['phase'] == 'complete'
        assert pipeline['build_revision'] == BUILD_REVISION and pipeline['compiler_runtime_sources_match']
        bind(cache / 'application-driver.py', pipeline['driver_sha256'])
        assert asan_prep['artifacts']['driver.py'] == pipeline['driver_sha256']
        normal_prepared = load(manifest['preparations']['normal']['path'])
        assert asan_prep['source_and_tool_sha256'] == {
            **normal_prepared['source_and_tool_sha256'], 'asan_runtime': build['sha256']['asan_runtime'],
            'prepared_record': manifest['preparations']['normal']['sha256']}
        asan_tree = retained_tree(asan_path.parent)
        assert set(asan_prep['artifacts']) == {name for name, is_directory in asan_tree.items()
                                             if not is_directory and name != 'record.json'}
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
            filesystem = item['filesystem']
            expected = (validator.compute_oracle(case) if compute else
                        validator.oracle(case) if filesystem else process_api.oracle(case))
            assert capture['command'] == [str(asan_path.parent / ('memory-compute' if compute else 'memory-report')), *case.arguments]
            check_capture(validator, case, expected, capture, 'relar', compute=compute,
                          prepared_process=not compute and not filesystem)
            counts['asan_workload_executions'] += 1
        real_pointer = asan_prep['real_capture_validation']
        real_root = Path(real_pointer['record']).parent
        local_real_root = temporary / 'asan-real-captures'
        restore_tree(real_root, local_real_root)
        real_case = mode_cases['normal']['real-prepared-ty'][1]
        helpers['check_real_captures'](benchmark, validator, normal_prepared,
            normal_prepared['source_and_tool_paths'], real_case,
            {'relar': [str(asan_path.parent / 'memory-report')]}, counts, retained,
            lambda path: local_real_root / Path(path).relative_to(real_root),
            pointer=real_pointer, count_key='asan_real_capture_executions')
        assert {key: counts[key] for key in audit['counts']} == audit['counts']

    result = {'schema': 1, 'status': 'passed', 'measurement_revision': REVISION,
              'compiler_build_revision': BUILD_REVISION, 'equivalent_compiler_runtime_inputs': 74,
              'archive_sha256': manifest['archive']['sha256'], 'manifest_sha256': sha(manifest_data),
              'source_tree': tree_id, 'source_files': len(source), 'omitted_historical_result_files': len(omitted),
              'retained_logical_files': len(index['files']), 'excluded_native_artifacts': len(index['excluded']),
              'rust_unit_tests': validation_suites['compiler_unit_tests'] + validation_suites['runtime_unit_tests'],
              'normal_suites': validation_suites['ordinary_suites'], 'normal_reported_checks': general_checks,
              'base45': {'upstream_calls': 646, 'differential_cases': 578, 'type_rejections': 4},
              'luhn': {'upstream_calls': 30, 'differential_cases': 2288},
              'affected_reported_checks': affected_counts, 'validation_suites': validation_suites,
              'counts': dict(counts), 'modes': modes,
              'limits': 'Data-only replay. Hashes bind archived source, captures and recorded external artifact identities; absent native tools/binaries cannot be rehashed here. No native execution, timing remeasurement, timestamp authentication, or proof of historical host isolation. Repeated-compute timings retain preload/startup/final-output costs and subtract no controls.'}
    data = (json.dumps(result, indent=2) + '\n').encode()
    if args.output:
        with args.output.open('xb') as stream:
            stream.write(data)
    print(json.dumps({key: value for key, value in result.items() if key != 'modes'}, indent=2))


if __name__ == '__main__':
    main()
