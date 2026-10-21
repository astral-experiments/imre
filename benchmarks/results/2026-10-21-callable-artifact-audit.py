"""Independently verify callback publication bytes, Git snapshots, and statistics."""
import gzip
import hashlib
import io
import itertools
import json
from pathlib import Path
import statistics
import subprocess
import tarfile

ROOT = Path('/home/dev-user/code/oss/relar')
PREFIX = ROOT / 'benchmarks/results/2026-10-21-callable'
REVISION = '848477f3d06fb1afe6d4bdd93dae4b107c90229a'
ENGINES = ('relar', 'cpython', 'rust')
def sha(data):
    return hashlib.sha256(data).hexdigest()
def load(suffix):
    return Path(str(PREFIX) + suffix).read_bytes()
def check(condition, message):
    if not condition:
        raise RuntimeError(message)
def forbidden(data):
    return data.startswith((b'\x7fELF', b'!<arch>\n', b'!<thin>\n', b'BC\xc0\xde', b'\xde\xc0\x17\x0b', b'MZ', b'\0asm'))

archive_bytes = load('-evidence.tar.gz')
manifest = json.loads(load('-evidence-manifest.json'))
check(manifest['status'] == 'passed' and manifest['revision'] == REVISION, 'manifest status/revision')
check(sha(archive_bytes) == manifest['archive']['sha256'] and len(archive_bytes) == manifest['archive']['bytes'], 'archive identity')
check(archive_bytes[4:8] == b'\0' * 4 and not archive_bytes[3] & 8, 'gzip timestamp/name')
members = {}
with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode='r:gz') as archive:
    check(archive.getnames() == sorted(manifest['members']), 'exact archive membership')
    for member in archive:
        check(member.isfile() and member.name not in members, 'regular unique members')
        check(member.mtime == member.uid == member.gid == 0 and not member.uname and not member.gname, 'canonical tar metadata')
        data = archive.extractfile(member).read()
        check(not forbidden(data), 'native artifact in archive')
        check(manifest['members'][member.name] == {'sha256': sha(data), 'bytes': len(data), 'mode': member.mode}, 'member identity/mode')
        members[member.name] = data
check(len(members) == manifest['member_count'], 'member count')
source_manifest = json.loads(members['provenance/source-manifest.json'])
entries = subprocess.check_output(['git', '-C', str(ROOT), 'ls-tree', '-r', '-z', REVISION]).split(b'\0')
source_entries = {}
for entry in filter(None, entries):
    metadata, path = entry.split(b'\t', 1)
    if path.startswith(b'benchmarks/results/'):
        continue
    mode, kind, oid = metadata.split()
    check(kind == b'blob' and mode in (b'100644', b'100755'), 'Git source kind')
    source_entries[path.decode()] = (int(mode, 8) & 0o777, oid.decode())
check(set(source_entries) == set(source_manifest), 'Git source inventory')
request = ''.join(oid + '\n' for _, oid in source_entries.values()).encode()
output = io.BytesIO(subprocess.check_output(['git', '-C', str(ROOT), 'cat-file', '--batch'], input=request))
for name, (mode, oid) in source_entries.items():
    header = output.readline().decode().split()
    check(header[:2] == [oid, 'blob'], 'Git blob header')
    data = output.read(int(header[2])); check(output.read(1) == b'\n', 'Git batch delimiter')
    check(members['source/' + name] == data, 'exact Git source bytes')
    check(source_manifest[name] == {'sha256': sha(data), 'bytes': len(data), 'mode': mode}, 'exact Git source mode/hash')
check(not output.read(), 'Git batch trailing bytes')
audit_bytes = load('-audit.json'); audit = json.loads(audit_bytes)
report_bytes = load('-benchmarks.md'); report = report_bytes.decode()
check(members['audit.json'] == audit_bytes and members['REPORT.md'] == report_bytes, 'exact report/audit sidecars')
check(audit['status'] == 'passed' and audit['input_guards_passed'] and audit['revision'] == REVISION, 'audit status')
check(audit['packager_sha256'] == sha(members['helpers/package-evidence.py']), 'packager binding')
check(manifest['audit_sha256'] == sha(audit_bytes), 'manifest audit binding')
exclusions = json.loads(members['provenance/exclusions.json'])
check(len(exclusions) == manifest['excluded_artifact_count'], 'exclusion count')
check(all(item['would_be_member'] not in members for item in exclusions), 'excluded artifact present')
counts = dict(gates=0, warmups=0, measured=0)
table_rows = 0
large = {}
for mode in ('normal', 'thinlto'):
    compressed = load('-' + mode + '.json.gz')
    check(compressed[4:8] == b'\0' * 4 and not compressed[3] & 8, 'record gzip metadata')
    raw = gzip.decompress(compressed)
    check(raw == members['timing/' + mode + '.json'] and sha(raw) == audit['timing'][mode]['sha256'], 'exact raw timing record')
    record = json.loads(raw)
    check(record['revision'] == REVISION and record['input_guards_passed'], 'timing revision/status')
    check(record['engine_orders'] == list(map(list, itertools.permutations(ENGINES))), 'engine orders')
    check(record['samples'] == 12 and record['warmups'] == 2, 'round counts')
    for workload in [record['startup_control'], *record['workloads']]:
        check(set(workload['gates']) == set(ENGINES), 'gate engines')
        for phase, count_key in [('warmups', 'warmups'), ('samples', 'measured')]:
            check(len(workload[phase]) == record[phase], 'phase length')
            for index, row in enumerate(workload[phase]):
                check(row['index'] == index and row['order'] == record['engine_orders'][index % 6], 'round position/order')
                for engine in ENGINES:
                    elapsed = row[engine]['elapsed_ns']
                    check(type(elapsed) is int and elapsed > 0, 'elapsed time')
                    for key in ('returncode', 'stdout', 'stderr', 'files', 'directories'):
                        check(row[engine][key] == workload['gates'][engine][key], 'complete result changed between runs')
                    counts[count_key] += 1
        counts['gates'] += 3
        summary = {}
        for engine in ENGINES:
            values = [row[engine]['elapsed_ns'] / 1e9 for row in workload['samples']]
            q = statistics.quantiles(values, n=4, method='inclusive')
            summary[engine] = dict(median_seconds=statistics.median(values), min_seconds=min(values), max_seconds=max(values), p25_seconds=q[0], p75_seconds=q[2])
            check(summary[engine] == workload['summary'][engine], 'raw sample summary')
        for left, right in [('cpython', 'relar'), ('relar', 'rust'), ('cpython', 'rust')]:
            check(workload[left + '_over_' + right] == summary[left]['median_seconds'] / summary[right]['median_seconds'], 'raw sample ratio')
        if workload['records']:
            row = f"| `{workload['label']}` | {summary['relar']['median_seconds']*1000:.3f} | {summary['rust']['median_seconds']*1000:.3f} | {summary['cpython']['median_seconds']*1000:.3f} | {workload['cpython_over_relar']:.3f}× | {workload['relar_over_rust']:.3f}× |"
            check(row in report, 'numeric report table row')
            table_rows += 1
            if workload['records'] == 4096:
                large[mode + '/' + workload['label']] = workload['relar_over_rust']
    control = record['startup_control']['summary']
    fraction = max(control['relar']['median_seconds'] / item['summary']['relar']['median_seconds'] for item in record['workloads'])
    for item in record['workloads']:
        for engine in ENGINES:
            check(item['startup_control_fraction'][engine] == control[engine]['median_seconds'] / item['summary'][engine]['median_seconds'], 'raw control fraction')
    row = f"| {mode} | {control['relar']['median_seconds']*1000:.3f} | {control['rust']['median_seconds']*1000:.3f} | {control['cpython']['median_seconds']*1000:.3f} | {fraction:.2%} |"
    check(row in report, 'control report table row')
    table_rows += 1
full = [large['thinlto/' + mode + '-full-4096'] for mode in ('gross', 'net')]
selective = [large['thinlto/' + mode + '-selective-4096'] for mode in ('gross', 'net')]
check(f"**{min(full):.2f}–{max(full):.2f}× Rust's time**" in report, 'full headline ratio')
check(f"**{min(selective):.2f}–{max(selective):.2f}×**" in report, 'selective headline ratio')
check(all(value > 2 for value in full), 'headline target conclusion')
check(counts == {'gates': 54, 'warmups': 108, 'measured': 648}, 'independent execution counts')
check(audit['execution_counts'] == {'preparation': 732, **counts}, 'audit execution counts')
result = {'status': 'passed', 'scope': 'Independent archive/member/mode/Git-source/sidecar verification and raw-sample statistics/report-table recomputation; application oracle validity is provided by the separately reviewed packaged audit.', 'revision': REVISION, 'archive_sha256': sha(archive_bytes), 'manifest_sha256': sha(load('-evidence-manifest.json')), 'members': len(members), 'source_files': len(source_entries), 'exclusions': len(exclusions), 'numeric_report_rows': table_rows, 'execution_counts': {'preparation': 732, **counts}, 'large_relar_over_rust': large, 'auditor_sha256': sha(Path(__file__).read_bytes())}
Path('/tmp/relar-callable-artifact-audit.json').write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
print(json.dumps(result, indent=2, sort_keys=True))
