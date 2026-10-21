"""Independently verify final archive, public sidecars, and report numeric tables.

No producer modules or archived code are imported. Only immutable Git objects
are read through subprocesses; no workloads, builds, profiles, or API calls run.
"""

import argparse
import base64
from collections import Counter
from datetime import datetime, timezone
import gzip
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import statistics
import subprocess
import sys
import tarfile

ENGINES = ('relar', 'rust', 'cpython')
SCENARIOS = ('report-mixed', 'alert-quiet', 'within-quiet', 'latest-quiet')
PILOT_NUMBERS = ('003', '005', '006', '007', '008', '009', '010')
GUARDS = {}


def check(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def guard(path, expected=None):
    path = Path(path).absolute()
    with path.open('rb') as stream:
        actual = hashlib.file_digest(stream, 'sha256').hexdigest()
    check(expected is None or actual == expected, 'input identity differs: ' + str(path))
    check(GUARDS.setdefault(str(path), actual) == actual, 'input changed: ' + str(path))
    return actual


def read(path, expected=None):
    guard(path, expected)
    return json.loads(Path(path).read_bytes())


def forbidden(header):
    four = header[:4]
    return (four in {b'\x7fELF', b'BC\xc0\xde', b'\xde\xc0\x17\x0b', b'\0asm',
                     b'\xfe\xed\xfa\xce', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf',
                     b'\xcf\xfa\xed\xfe', b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca',
                     b'\xca\xfe\xba\xbf', b'\xbf\xba\xfe\xca'}
            or header.startswith((b'!<arch>\n', b'!<thin>\n', b'MZ'))
            or four[2:] == b'\r\n' and (3000 <= int.from_bytes(four[:2], 'little') < 4000
                                         or int.from_bytes(four[:2], 'little') == 62211))


def normalized_gzip(path):
    with Path(path).open('rb') as stream:
        header = stream.read(10)
    check(header[:3] == b'\x1f\x8b\x08' and header[3] == 0 and header[4:8] == b'\0' * 4,
          'gzip metadata is not normalized: ' + str(path))


def ms(value):
    return f'{value * 1000:.3f}'


def ratio(value):
    return f'{value:.3f}×'


def interval(value):
    return f"{ms(value['median_seconds'])} [{ms(value['p25_seconds'])}, {ms(value['p75_seconds'])}]"


def main():
    check(sys.flags.optimize == 0, 'run without Python -O')
    script_sha = guard(__file__)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, required=True, help='Full path to published filename prefix')
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--render-record', type=Path, required=True)
    parser.add_argument('--renderer', type=Path, default=Path('/tmp/relar-iterator-performance-render-report.py'))
    for number in PILOT_NUMBERS:
        parser.add_argument(f'--pilot-{number}-audit', type=Path,
                            default=Path(f'/tmp/relar-telemetry-candidate-{number}-audit.json'))
    parser.add_argument('--branch-audit', type=Path, default=Path('/tmp/relar-telemetry-candidate-005-branch-audit.json'))
    parser.add_argument('--root', type=Path, default=Path('/home/dev-user/code/oss/relar'))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    check(not args.output.exists() and not args.output.is_symlink(), 'use a new audit output')
    prefix, root = args.prefix.absolute(), args.root.resolve(strict=True)
    archive_path = Path(str(prefix) + '-evidence.tar.gz')
    manifest_path = Path(str(prefix) + '-evidence-manifest.json')
    manifest = read(manifest_path)
    check(manifest['status'] == 'passed' and manifest['kind'] == 'iterator-performance-final-evidence', 'final package is incomplete')
    guard(archive_path, manifest['archive']['sha256'])
    check(archive_path.name == manifest['archive']['name'] and archive_path.stat().st_size == manifest['archive']['bytes'], 'archive name/size differs')
    normalized_gzip(archive_path)
    keep = {
        'provenance/package.json', 'provenance/exclusions.json',
        'provenance/final-plan.json', 'provenance/candidate-plan.json',
        'validation/index.json', 'validation/build.json', 'validation/evidence-sha256.json',
        'validation/source-before.json', 'validation/source-after.json',
        'audits/final-timing-profile.json', 'audits/full-validation.json',
        'preparation/status.json', 'preparation/normal.json', 'preparation/thinlto.json',
        'profiles/final/summary.json', 'profiles/final/manifest.json',
        'historical-baselines/full-thinlto.json',
        *(f'timing/status/{phase}.json' for phase in ('full', 'early')),
        *(f'timing/records/{phase}-{mode}.json' for phase in ('full', 'early') for mode in ('normal', 'thinlto')),
    }
    members, values, total = {}, {}, 0
    top_levels = {'source', 'validation', 'audits', 'helpers', 'provenance', 'preparation',
                  'tools', 'historical-baselines', 'external', 'references', 'timing', 'profiles', 'exploratory'}
    with tarfile.open(archive_path, 'r|gz') as archive:
        for item in archive:
            name, path = item.name, PurePosixPath(item.name)
            check(item.isfile() and str(path) == name and not path.is_absolute() and '..' not in path.parts
                  and path.parts[0] in top_levels and name not in members, 'unsafe/unexpected archive member: ' + name)
            check(item.uid == item.gid == item.mtime == 0 and not item.uname and not item.gname
                  and item.mode in (0o644, 0o755), 'archive metadata differs: ' + name)
            check(name in manifest['members'], 'unmanifested member: ' + name)
            stream, hasher, count, first, chunks = archive.extractfile(item), hashlib.sha256(), 0, b'', []
            while chunk := stream.read(1024 * 1024):
                if not first:
                    first = chunk[:16]
                hasher.update(chunk)
                count += len(chunk)
                if name in keep:
                    chunks.append(chunk)
            check(not forbidden(first), 'native code or bytecode archived: ' + name)
            entry = {'sha256': hasher.hexdigest(), 'bytes': count, 'mode': item.mode}
            check(count == item.size and entry == manifest['members'][name], 'member identity differs: ' + name)
            check(name.startswith('source/') or item.mode == 0o644, 'non-source executable member: ' + name)
            members[name] = entry
            total += count
            if name in keep:
                values[name] = json.loads(b''.join(chunks))
    check(list(members) == sorted(manifest['members']) and len(members) == manifest['member_count'], 'archive order/inventory differs')
    check(not any(name.startswith('source/benchmarks/results/') or name.endswith(('-evidence.tar.gz', '.tar', '.tar.xz', '.tgz', '.zip')) for name in members), 'nested historical archive')
    package, exclusions = values['provenance/package.json'], values['provenance/exclusions.json']
    check(all(manifest[name] == value for name, value in package.items()), 'manifest/package provenance differs')
    excluded = {entry['would_be_member']: entry for entry in exclusions}
    check(len(excluded) == len(exclusions) == manifest['excluded_artifact_count'] and not set(excluded) & set(members), 'exclusion set differs')
    for name, entry in excluded.items():
        path = Path(entry['path'])
        guard(path, entry['sha256'])
        check(path.stat().st_size == entry['bytes'], 'excluded artifact size differs')
        with path.open('rb') as stream:
            check(entry['reason'] == 'external tool executable' or forbidden(stream.read(16)), 'unjustified exclusion: ' + name)

    def bound(name, expected, omitted=False):
        check(name in members or omitted and name in excluded, 'required evidence missing: ' + name)
        entry = members[name] if name in members else excluded[name]
        check(entry['sha256'] == expected, 'evidence identity differs: ' + name)

    def subtree(prefix):
        return {name.removeprefix(prefix + '/') for name in [*members, *excluded] if name.startswith(prefix + '/')}

    audit, vaudit = values['audits/final-timing-profile.json'], values['audits/full-validation.json']
    index, build = values['validation/index.json'], values['validation/build.json']
    plan, candidate = values['provenance/final-plan.json'], values['provenance/candidate-plan.json']
    revision = audit['revision']
    check(audit['status'] == vaudit['status'] == index['status'] == 'passed'
          and audit['input_guards_passed'] and audit['validation_full_matrix_passed'] is True and audit['exploratory'] is False,
          'final audits are incomplete')
    check(index['mode'] == 'full-validation' and index['full_validation_passed'] is True
          and index['input_guards_passed'] and index['source_unchanged'] and build['input_guards_passed'] and build['source_matches_revision'], 'validation/build guards differ')
    check(all(value == revision for value in (vaudit['validation_revision'], index['revision'], build['revision'], plan['revision'], candidate['revision'], package['build_revision'], package['validation_revision'], package['timing_revision'])), 'source revision differs')
    check(package['post_validation_paths'] == [], 'post-validation source overlay is forbidden')
    check(package['final_plan_sha256'] == audit['plan']['sha256']
          and package['candidate_plan_sha256'] == audit['candidate_plan']['sha256']
          and package['validation_index_sha256'] == audit['validation']['index_sha256']
          and package['build_record_sha256'] == audit['validation']['build_sha256']
          and package['validation_audit_sha256'] == audit['validation']['audit_sha256']
          and package['final_auditor_sha256'] == audit['audit_script_sha256'], 'package/audit cross-bindings differ')
    for name, expected in {
        'audits/final-timing-profile.json': package['final_audit_sha256'],
        'audits/full-validation.json': package['validation_audit_sha256'],
        'validation/index.json': package['validation_index_sha256'],
        'validation/build.json': package['build_record_sha256'],
        'validation/runner.py': index['runner_sha256'],
        'validation/evidence-sha256.json': index['evidence_manifest_sha256'],
        'provenance/final-plan.json': package['final_plan_sha256'],
        'provenance/candidate-plan.json': package['candidate_plan_sha256'],
        'helpers/full-validation-audit.py': vaudit['audit_script_sha256'],
        'helpers/final-timing-profile-audit.py': audit['audit_script_sha256'],
        'helpers/independent-oracle-profile-parsers.py': audit['parser_dependency']['sha256'],
        'helpers/package-final-evidence.py': package['packager_sha256'],
        'helpers/archive-machinery.py': package['archive_machinery_sha256'],
    }.items():
        bound(name, expected)
    check(audit['validation']['counts'] == vaudit['counts'] == package['validation_counts']
          and len(index['jobs']) == vaudit['counts']['passed_jobs'] == 138
          and index['job_counts'] == {'passed': 138, 'failed': 0}
          and index['unit_tests_passed'] == vaudit['counts']['rust_units']['total'], 'validation coverage differs')
    evidence = values['validation/evidence-sha256.json']
    check(subtree('validation') == set(evidence) | {'index.json', 'evidence-sha256.json'}, 'validation archive inventory differs')
    for name, expected in evidence.items():
        bound('validation/' + name, expected, omitted=True)
    validation_dir = Path(audit['validation']['index']).parent
    for job in index['jobs']:
        check(job['status'] == 'passed' and job['exit_code'] == 0 and job['error'] is None, 'failed archived validation job')
        for key in ('record', 'stdout', 'stderr'):
            bound('validation/' + Path(job[key]).relative_to(validation_dir).as_posix(), job[key + '_sha256'], omitted=True)
    source = values['validation/source-before.json']
    check(source == values['validation/source-after.json'] and source['revision'] == revision and source['status'] == '', 'source capture differs')
    check(subtree('source') == set(source['files']) and package['source_files'] == len(source['files']), 'source member inventory differs')
    def git(*args):
        return subprocess.check_output(['git', '-C', str(root), *args])
    check(source['tree'] == package['source_tree'] == git('rev-parse', revision + '^{tree}').decode().strip(), 'source tree differs')
    source_count = 0
    with tarfile.open(fileobj=io.BytesIO(git('archive', '--format=tar', revision, '--', *sorted(source['files'])))) as archive:
        originals = set()
        for item in archive:
            if item.isdir():
                continue
            check(item.isfile() and item.name in source['files'], 'unexpected Git source entry')
            raw = archive.extractfile(item).read()
            identity = digest(raw)
            entry = source['files'][item.name]
            check(identity == entry['sha256'] and bool(item.mode & 0o111) == entry['executable'], 'Git source identity differs')
            bound('source/' + item.name, identity)
            check(members['source/' + item.name]['mode'] == (0o755 if entry['executable'] else 0o644), 'source executable mode differs')
            originals.add(item.name)
            source_count += 1
        check(originals == set(source['files']), 'Git source coverage differs')
    for name, expected in build['source_sha256'].items():
        bound('source/' + name, expected)
    preparation_status = values['preparation/status.json']
    bound('preparation/status.json', audit['preparation_status']['sha256'])
    check(preparation_status['status'] == 'passed' and [run['mode'] for run in preparation_status['runs']] == ['normal', 'thinlto'], 'preparation phase incomplete')
    for run in preparation_status['runs']:
        mode = run['mode']
        bound('preparation/' + mode + '.json', audit['preparations'][mode]['sha256'])
        prepared = values['preparation/' + mode + '.json']
        check(prepared['relar_build_record'] == build and prepared['input_guards_passed'] and prepared['revision'] == revision, 'prepared build differs')
        for key in ('stdout', 'stderr'):
            bound(f'preparation/logs/{mode}.{key}', run[key + '_sha256'])
        directory = Path(prepared['binary']).parent
        for key in ('binary', 'llvm', 'validation'):
            path = Path(prepared['validation_path' if key == 'validation' else key])
            bound('preparation/' + mode + '/' + path.relative_to(directory).as_posix(), prepared[key + '_sha256'], omitted=key == 'binary')
    records, raw_output_files, gzip_records = {}, 0, {}
    for phase in ('full', 'early'):
        status = values['timing/status/' + phase + '.json']
        bound('timing/status/' + phase + '.json', audit['statuses'][phase]['sha256'])
        check(status['status'] == 'passed' and status['exploratory'] is False and status['revision'] == revision, 'archived timing status differs')
        for run in status['runs']:
            key = phase + '-' + run['mode']
            expected = audit['records'][key]['record_sha256']
            bound('timing/records/' + key + '.json', expected)
            check(run['output_sha256'] == expected and package['timing_records'][key]['record_sha256'] == expected, 'timing record binding differs')
            for stream in ('stdout', 'stderr'):
                bound('timing/logs/' + key + '.' + stream, run[stream + '_sha256'])
            record = values['timing/records/' + key + '.json']
            check(record['revision'] == revision and record['preparation'] == values['preparation/' + run['mode'] + '.json'], 'timing preparation differs')
            records[key] = record
            expected_files = set()
            for row in [record['startup_control'], *record['workloads']]:
                for engine in ENGINES:
                    for name, encoded in row['expected_files'].items():
                        raw = base64.b64decode(encoded['base64'], validate=True)
                        check(digest(raw) == encoded['sha256'] and len(raw) == encoded['bytes'], 'encoded timing file differs')
                        relative = row['label'] + '/' + engine + '/' + name
                        bound('timing/artifacts/' + key + '/' + relative, encoded['sha256'])
                        expected_files.add(relative)
                        raw_output_files += 1
            check(subtree('timing/artifacts/' + key) == expected_files, 'timing artifact member inventory differs')
            compressed = Path(str(prefix) + '-' + key + '.json.gz')
            compressed_sha = guard(compressed)
            normalized_gzip(compressed)
            hasher, size = hashlib.sha256(), 0
            with gzip.open(compressed, 'rb') as stream:
                while chunk := stream.read(1024 * 1024):
                    hasher.update(chunk)
                    size += len(chunk)
            check(hasher.hexdigest() == expected and size == members['timing/records/' + key + '.json']['bytes'], 'public gzip does not contain the exact raw JSON')
            gzip_records[key] = {'compressed_sha256': compressed_sha, 'uncompressed_sha256': expected, 'uncompressed_bytes': size}
    check(set(records) == {'full-normal', 'full-thinlto', 'early-normal', 'early-thinlto'}, 'four timing records missing')
    profile = values['profiles/final/summary.json']
    bound('profiles/final/summary.json', audit['profile']['summary_sha256'])
    bound('profiles/final/manifest.json', audit['profile']['manifest_sha256'])
    profile_manifest = values['profiles/final/manifest.json']
    check(subtree('profiles/final') == set(profile_manifest) | {'manifest.json'}, 'profile member inventory differs')
    for name, expected in profile_manifest.items():
        bound('profiles/final/' + name, expected)
    check(profile['status'] == 'passed' and profile['complete'] and profile['exploratory'] is False and len(profile['profiles']) == 6, 'final profiles incomplete')
    sidecars = {'audit': ('audits/final-timing-profile.json', audit), 'validation-audit': ('audits/full-validation.json', vaudit), 'profile': ('profiles/final/summary.json', profile)}
    for suffix, (name, value) in sidecars.items():
        path = Path(str(prefix) + '-' + suffix + '.json')
        check(read(path, members[name]['sha256']) == value, 'public audit/profile sidecar differs')
    report_bytes = args.report.read_bytes()
    report_sha = guard(args.report)
    check(hashlib.sha256(report_bytes).hexdigest() == report_sha, 'report changed while reading')
    report = report_bytes.decode('utf-8')
    render = read(args.render_record)
    check(render['status'] == 'passed' and render['kind'] == 'iterator-performance-report-render'
          and render['revision'] == revision and render['published_prefix'] == prefix.name
          and render['report_sha256'] == report_sha and render['performance_target'] == audit['performance_target'], 'render record differs')
    for path, expected in render['inputs'].items():
        guard(path, expected)
    renderer_path = str(args.renderer.resolve(strict=True))
    check(render['inputs'].get(renderer_path) == render['renderer_sha256'] == guard(args.renderer), 'named renderer input identity differs')
    lines, expected_rows = report.splitlines(), []
    tables = {name: [] for name in ('normal', 'thinlto', 'control', 'target', 'cpu', 'trajectory', 'raw')}
    def expect(line):
        check(line in lines, 'missing numeric report row: ' + line)
        expected_rows.append(line)
    medians = {}
    for key, record in records.items():
        for row in [record['startup_control'], *record['workloads']]:
            for engine in ENGINES:
                samples = [sample[engine]['elapsed_ns'] / 1e9 for sample in row['samples']]
                check(len(samples) == 12 and all(value > 0 for value in samples), 'raw sample schedule differs')
                q = statistics.quantiles(samples, n=4, method='inclusive')
                summary = {'median_seconds': statistics.median(samples), 'min_seconds': min(samples), 'max_seconds': max(samples), 'p25_seconds': q[0], 'p75_seconds': q[2]}
                check(summary == row['summary'][engine] == audit['records'][key]['ratios_and_medians'][row['label']]['summary'][engine], 'raw timing distribution differs')
            timings = row['summary']
            relar_rust = timings['relar']['median_seconds'] / timings['rust']['median_seconds']
            cpython_relar = timings['cpython']['median_seconds'] / timings['relar']['median_seconds']
            check(relar_rust == row['relar_over_rust'] and cpython_relar == row['cpython_over_relar'], 'raw timing ratio differs')
            medians[(key, row['label'])] = row
            if row['label'] != 'empty-control':
                rendered = f"| `{row['scenario']}` / {row['records']:,} | {ms(timings['relar']['median_seconds'])} | {ms(timings['rust']['median_seconds'])} | {ms(timings['cpython']['median_seconds'])} | {ratio(cpython_relar)} | {ratio(relar_rust)} |"
                expect(rendered)
                tables['thinlto' if record['thinlto'] else 'normal'].append(rendered)
        timings = record['startup_control']['summary']
        fraction = max(record['startup_control']['summary']['relar']['median_seconds'] / row['summary']['relar']['median_seconds'] for row in record['workloads'])
        check(fraction == audit['records'][key]['max_control_fractions']['relar'], 'control fraction differs')
        rendered = f"| {key} | {ms(timings['relar']['median_seconds'])} | {ms(timings['rust']['median_seconds'])} | {ms(timings['cpython']['median_seconds'])} | {fraction:.2%} |"
        expect(rendered)
        tables['control'].append(rendered)
    target = {}
    for scenario in SCENARIOS:
        row = medians[('full-thinlto', scenario + '-4096')]
        target[row['label']] = row['relar_over_rust']
        rendered = f"| `{scenario}` | {interval(row['summary']['relar'])} | {interval(row['summary']['rust'])} | {ratio(row['cpython_over_relar'])} | {ratio(row['relar_over_rust'])} | {'yes' if row['relar_over_rust'] < 2 else 'no'} |"
        expect(rendered)
        tables['target'].append(rendered)
    check(target == audit['performance_target']['individual_relar_over_rust'] and all(value < 2 for value in target.values()) == audit['performance_target']['all_four_below_two'], 'report goal differs')
    check(f"**{min(target.values()):.3f}–{max(target.values()):.3f}× Rust's time**" in report, 'headline range differs')
    met = all(value < 2 for value in target.values())
    success = 'All four cases meet the requested **below 2× Rust** target.'
    failure = 'The requested **below 2× Rust in every case** target is not met; the misses remain visible below.'
    check((success in report) == met and (failure in report) == (not met), 'goal sentence differs')
    fractions = {engine: max(record['startup_control']['summary'][engine]['median_seconds'] / row['summary'][engine]['median_seconds'] for record in records.values() for row in record['workloads']) for engine in ENGINES}
    check(fractions == audit['maximum_control_fractions'], 'maximum control fraction differs')
    check(f"maximum empty-control/workload fractions are {fractions['relar']:.2%} for Relar, {fractions['rust']:.2%} for Rust, and {fractions['cpython']:.2%} for CPython" in report, 'control fraction prose differs')
    instruction_ratio = audit['profile']['metrics']['relar-callgrind-128']['instructions'] / audit['profile']['metrics']['rust-callgrind-128']['instructions']
    check(instruction_ratio == audit['profile']['whole_process_instruction_ratio_relar_over_rust']
          and f"instruction ratio is {instruction_ratio:.3f}× Rust" in report, 'instruction ratio prose differs')
    for engine in ('relar', 'rust'):
        cpu = audit['profile']['metrics'][engine + '-callgrind-128']
        heap = audit['profile']['metrics'][engine + '-dhat-128']
        growth = audit['profile']['allocation_growth'][engine]
        rendered = f"| {engine.title()} | {cpu['instructions']:,} | {heap['allocated_bytes']:,} | {heap['allocated_blocks']:,} | {growth['allocated_bytes']['per_additional_query']:g} | {growth['allocated_blocks']['per_additional_query']:g} |"
        expect(rendered)
        tables['cpu'].append(rendered)
    check('| Raw complete measurement record | Uncompressed JSON SHA256 |' in report, 'gzip checksum label is ambiguous')
    for key in records:
        rendered = f"| [{key}]({prefix.name}-{key}.json.gz) | `{audit['records'][key]['record_sha256']}` |"
        expect(rendered)
        tables['raw'].append(rendered)
    pilots, pilot_members = {}, {}
    for number in PILOT_NUMBERS:
        path = getattr(args, f'pilot_{number}_audit')
        pilot = read(path)
        pilot_sha = guard(path)
        check(pilot['status'] == 'passed' and pilot['exploratory'] is True
              and pilot['pilot_audit']['status'] == 'passed'
              and pilot['pilot_audit']['raw_process_records_verified'] == 120
              and pilot['profiles_verified'] == 6 and pilot['raw_profile_counts_independently_reparsed']
              and render['inputs'].get(str(path.resolve(strict=True))) == pilot_sha,
              'historical pilot audit is not bound')
        audit_members = [name for name, entry in members.items()
                         if name.startswith('exploratory/') and entry['sha256'] == pilot_sha]
        check(audit_members, 'historical pilot audit is missing from archive: ' + number)
        historical_manifest = read(Path(pilot['profile_directory']) / 'manifest.json', pilot['profile_manifest_sha256'])
        for name, expected in {
            'summary.json': pilot['profile_summary_sha256'],
            'benchmark-record.json': pilot['benchmark_record_sha256'],
            'timing-status.json': pilot['timing_status_sha256'],
            'preparation.json': pilot['preparation_record_sha256'],
        }.items():
            check(historical_manifest[name] == expected, 'historical pilot/profile binding differs: ' + number)
        profile_roots = []
        for name, entry in members.items():
            if (name.startswith('exploratory/') and name.endswith('/manifest.json')
                    and entry['sha256'] == pilot['profile_manifest_sha256']):
                parent = name.removesuffix('/manifest.json')
                if subtree(parent) == set(historical_manifest) | {'manifest.json'}:
                    for relative, expected in historical_manifest.items():
                        bound(parent + '/' + relative, expected, omitted=True)
                    profile_roots.append(parent)
        check(profile_roots, 'complete historical pilot/profile snapshot is missing: ' + number)
        pilot_members[number] = {'audit_members': audit_members, 'profile_roots': profile_roots}
        pilots[number] = pilot
    branch = read(args.branch_audit)
    branch_sha = guard(args.branch_audit)
    check(branch['status'] == 'passed' and branch['exploratory'] is True
          and branch['source_audit_sha256'] == guard(args.pilot_005_audit)
          and render['inputs'].get(str(args.branch_audit.resolve(strict=True))) == branch_sha,
          'branch audit is not bound')
    check(any(name.startswith('exploratory/') and entry['sha256'] == branch_sha
              for name, entry in members.items()), 'branch audit is missing from archive')
    check(branch['files']['profile.callgrind'] == branch['profile_sha256'], 'branch raw profile binding differs')
    branch_roots = []
    for name, entry in members.items():
        if (name.startswith('exploratory/') and name.endswith('/profile.callgrind')
                and entry['sha256'] == branch['profile_sha256']):
            parent = name.removesuffix('/profile.callgrind')
            if subtree(parent) == set(branch['files']):
                for relative, expected in branch['files'].items():
                    bound(parent + '/' + relative, expected)
                branch_roots.append(parent)
    check(branch_roots, 'complete exploratory branch snapshot is missing')
    baseline = values['historical-baselines/full-thinlto.json']
    bound('historical-baselines/full-thinlto.json', plan['protected_sha256']['baseline:full-thinlto'])
    historical = {row['label']: row for row in baseline['workloads']}
    stages = [('Historical original ThinLTO', [historical[scenario + '-4096']['elir_over_rust'] for scenario in SCENARIOS])]
    for number in PILOT_NUMBERS:
        stages.append(('Pilot ' + number, [pilots[number]['pilot_audit']['ratios_and_medians'][scenario + '-4096']['relar_over_rust'] for scenario in SCENARIOS]))
    stages.append(('Final', [target[scenario + '-4096'] for scenario in SCENARIOS]))
    for stage, ratios in stages:
        rendered = '| ' + stage + ' | ' + ' | '.join(ratio(value) for value in ratios) + ' |'
        expect(rendered)
        tables['trajectory'].append(rendered)
    covered_lines = set()
    def table(header, rows, heading=None):
        if heading is None:
            positions = [i for i, line in enumerate(lines) if line == header]
            check(len(positions) == 1, 'ambiguous report table header: ' + header)
            position = positions[0]
        else:
            check(lines.count(heading) == 1, 'missing report section: ' + heading)
            position = lines.index(heading) + 2
            check(lines[position] == header, 'table is under the wrong linking section')
        check(re.fullmatch(r'\|[-:| ]+\|', lines[position + 1]), 'invalid table separator')
        check(lines[position + 2:position + 2 + len(rows)] == rows and lines[position + 2 + len(rows)] == '', 'numeric table row order/content differs')
        covered_lines.update(range(position, position + 2 + len(rows)))
    table('| Full scan, 4,096 records | Relar ms [p25, p75] | Rust ms [p25, p75] | CPython / Relar | Relar / Rust | <2× |', tables['target'])
    for mode, title in [('normal', 'Ordinary linking'), ('thinlto', 'ThinLTO')]:
        table('| Workload / records | Relar ms | Rust ms | CPython ms | CPython / Relar | Relar / Rust |', tables[mode], '### ' + title)
    table('| Batch / linking | Relar ms | Rust ms | CPython ms | Largest Relar control/workload fraction |', tables['control'])
    table('| Engine | Guest instructions, 128 queries | Allocated bytes, 128 queries | Allocation blocks | Additional bytes / query, 128→256 | Additional blocks / query |', tables['cpu'])
    table('| Stage | Report / Rust | Alert / Rust | Within / Rust | Latest / Rust |', tables['trajectory'])
    table('| Raw complete measurement record | Uncompressed JSON SHA256 |', tables['raw'])
    check(covered_lines == {i for i, line in enumerate(lines) if line.startswith('|')}, 'unverified extra report table rows')
    old_instructions = pilots['003']['metrics']['relar-callgrind-128']['instructions']
    middle_instructions = pilots['005']['metrics']['relar-callgrind-128']['instructions']
    final_instructions = audit['profile']['metrics']['relar-callgrind-128']['instructions']
    instruction_text = f"from {old_instructions:,} in exploratory candidate 003 to {middle_instructions:,} in 005 and {final_instructions:,} in the final build ({(1-final_instructions/old_instructions):+.2%} reduction relative to 003)"
    check(instruction_text in report, 'instruction trajectory prose differs')
    branch_totals = branch['function_self_totals']
    check(f"{branch_totals[1]:,} conditional branches and {branch_totals[2]:,} simulated misses" in report
          and f"{branch['largest_conditional_miss_site']['self_events'][2]:,} misses" in report, 'branch attribution prose differs')
    check(f"**{len(index['jobs'])} full-validation jobs**" in report
          and f"**{index['unit_tests_passed']} Rust unit tests**" in report, 'validation count prose differs')
    counts = Counter()
    for record in records.values():
        for row in [record['startup_control'], *record['workloads']]:
            counts['gates'] += len(row['gates'])
            counts['warmups'] += len(row['warmups']) * len(ENGINES)
            counts['measured'] += len(row['samples']) * len(ENGINES)
    counts['total'] = sum(counts.values())
    check(dict(counts) == audit['executions'], 'raw execution counts differ')
    check(f"**{counts['total']:,} process executions**: {counts['gates']:,} correctness gates, {counts['warmups']:,} warmups and {counts['measured']:,} measured executions" in report, 'execution count prose differs')
    for line, count in Counter(expected_rows).items():
        check(lines.count(line) == count, 'numeric report row multiplicity differs: ' + line)
    closure_path = Path(str(prefix) + '-rename-closure.json')
    closure = read(closure_path)
    check(closure['status'] == 'passed' and closure['kind'] == 'public-relar-rename-closure', 'public rename closure differs')
    check(guard(closure_path) in render['inputs'].values(), 'rename closure is not a renderer input')
    links = re.findall(r'\]\(([^)]+)\)', report)
    for target in links:
        if target.startswith(prefix.name + '-'):
            check((prefix.parent / target).is_file(), 'published evidence link is missing: ' + target)
    for path, expected in list(GUARDS.items()):
        guard(path, expected)
    check(guard(__file__) == script_sha, 'auditor changed')
    result = {
        'schema': 1, 'kind': 'iterator-performance-public-artifact-audit', 'status': 'passed',
        'audited_at_utc': datetime.now(timezone.utc).isoformat(), 'revision': revision,
        'archive_sha256': GUARDS[str(archive_path)], 'manifest_sha256': GUARDS[str(manifest_path)],
        'report_sha256': report_sha, 'render_record_sha256': guard(args.render_record),
        'audit_script_sha256': script_sha, 'members_verified': len(members),
        'uncompressed_bytes_verified': total, 'exclusions_verified': len(excluded),
        'source_files_verified_against_git': source_count, 'validation_jobs': len(index['jobs']),
        'raw_timing_output_files_verified': raw_output_files, 'gzip_records': gzip_records,
        'numeric_table_rows_verified': len(expected_rows), 'performance_target': audit['performance_target'],
        'exploratory_pilot_archive_bindings': pilot_members,
        'exploratory_branch_archive_roots': branch_roots,
        'guarded_inputs': GUARDS,
        'limits': 'Archive, sidecar, source, and report integrity audit. No native workload, archived script, producer module, build, timing, profile, or API operation was executed; detailed semantic/profile validity remains established by the separately retained independent audits.',
    }
    with args.output.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({'status': 'passed', 'audit': str(args.output), 'sha256': guard(args.output), 'members': len(members)}))


if __name__ == '__main__':
    main()
