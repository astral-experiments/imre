"""Offline validation of prospective observations against immutable stock_v1."""
from pathlib import Path
import argparse, ast, gzip, hashlib, json, re, subprocess, sys
PIN = '99964c6734dbe008d69d5016eb60d25abd31ab0d'

def sha(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()

def load(p):
    return json.loads(Path(p).read_bytes())

def need(c, m):
    if not c:
        raise ValueError(m)

def successful_execs(trace):
    lines = trace.splitlines()
    out = []
    for line in lines:
        m = re.match('^(\\d+)\\s+execve\\("([^"]+)",\\s*(\\[[^\\]]*\\]),', line)
        if 'execveat(' in line:
            raise ValueError('unexpected execveat attempt: ' + line[:180])
        if not m:
            if 'execve(' in line:
                raise ValueError('unparsed exec attempt: ' + line[:180])
            continue
        pid = int(m.group(1))
        ok = line.rstrip().endswith('= 0') or ('<unfinished' in line and any((re.match('^' + str(pid) + '\\s+<\\.\\.\\. execve resumed>\\).*?= 0$', x) for x in lines)))
        need(ok, 'failed or unresolved exec attempt: ' + line[:180])
        out.append((pid, m.group(2), ast.literal_eval(m.group(3))))
    return out

def main():
    p = argparse.ArgumentParser()
    p.add_argument('--evidence', type=Path, default=Path(__file__).parent / 'evidence')
    p.add_argument('--stock', type=Path, default=Path(__file__).resolve().parent.parent / 'stock_v1')
    p.add_argument('--partial', action='store_true')
    args = p.parse_args()
    E = args.evidence
    S = args.stock.resolve()
    repo = Path(subprocess.check_output(['git', '-C', str(S), 'rev-parse', '--show-toplevel'], text=True).strip())
    stock_rel = S.relative_to(repo)
    need(subprocess.run(['git', '-C', str(repo), 'diff', '--quiet', PIN, '--', str(stock_rel)]).returncode == 0, 'stock corpus differs from immutable commit')
    sys.path.insert(0, str(S))
    import validate as frozen
    frozen.main()
    sources = load(S / 'sources.json')
    build = load(E / 'build.json')
    need(build['pin'] == PIN, 'wrong pin')
    need(build['python_fixture_sha256'] == load(S / 'references/ruff-publisher/reference.json')['tool']['executable_sha256'], 'fixture Python hash')
    need(build['stub_sha256'] == sha(S / 'fixtures/cargo_stub.py'), 'stub hash')
    count = 0
    all_exact = True
    findings = {}
    for case, src in sources.items():
        refdir = S / 'references' / case
        ref = load(refdir / 'reference.json')
        case_results = {}
        for tier in ('O0', 'O3'):
            o = E / case / tier
            rp = o / 'receipt.json'
            if not rp.exists():
                need(args.partial, 'missing receipt ' + case + '/' + tier)
                continue
            r = load(rp)
            count += 1
            need(r['case'] == case and r['tier'] == tier and (r['source'] == src) and (r['reference_sha256'] == sha(refdir / 'reference.json')), 'identity/reference mismatch')
            need(load(o / 'before.json') == load(refdir / 'raw/before.json') and r['preimage_exact'], 'preimage differs')
            b = r['build']
            need(b['stdout_sha256'] == sha(o / 'build.stdout') and b['stderr_sha256'] == sha(o / 'build.stderr'), 'build evidence hash')
            need('--no-cache' in b['argv'] and b['argv'][b['argv'].index('--opt-level') + 1] == tier[1], 'unmatched build flags')
            need(b['argv'][0] == build['compiler'] and b['argv'][b['argv'].index('--runtime') + 1] == build['runtime'], 'toolchain path mismatch')
            if tier == 'O0':
                g = load(o / 'gate.json')
                need(g == r['gate'] and sha(o / 'gate.trace') == g['trace_sha256'] and (sha(o / 'gate.launcher.py.txt') == g['injected_sha256']) and ((o / 'gate.module.py.txt').read_bytes() == b'status: int = True\n'), 'gate raw evidence')
                need(g['positive_sha256'] == src['sha256'], 'gate binds wrong source')
                need(g['passed'] == (g['exit_code'] not in (0, None) and g['expected_int_got_bool'] and (not g['image_exists'])), 'dishonest gate status')
                all_exact &= g['passed']
            if not r['run']:
                need(r['status'] in ('build_blocker', 'build_timeout') and b['exit_code'] != 0 and (not r['paired']), 'invalid build blocker')
                case_results[tier] = r['status']
                all_exact = False
                continue
            run = r['run']
            need(b['exit_code'] == 0 and b['image_exists'] and b['image_sha256'], 'run without image')
            roles = run['roles']
            need(frozen.normalize(run['environment'], roles) == ref['invocation']['env'], 'environment drift')
            need(frozen.normalize(run['workload_argv'], roles) == ref['invocation']['argv'][2:], 'application argv drift')
            need(run['umask'] == '0077' and run['source_restored_sha256'] == src['sha256'] and (run['bytecode_present_before'] is False), 'withdrawal controls')
            before = load(o / 'before.json')
            after = load(o / 'after.json')
            a = {x['path']: x for x in before}
            z = {x['path']: x for x in after}
            changes = {'created': [x for k, x in z.items() if k not in a], 'deleted': [x for k, x in a.items() if k not in z], 'modified': [x for k, x in z.items() if k in a and x != a[k]]}
            registry = [json.loads(l) for l in (o / 'registry.jsonl').read_bytes().splitlines()]
            procs = [json.loads(l) for l in (o / 'processes.jsonl').read_bytes().splitlines()]
            effects = frozen.normalized_effects({'files': changes, 'registry': registry, 'processes': procs}, roles)
            need(effects == load(o / 'effects.normalized.json'), 'normalized effects not derived from raw')
            expected = load(refdir / 'effects.normalized.json')
            comp = {'exit_code': run['exit_code'] == ref['observation']['exit_code'] and (not run['timed_out']), 'stdout': (o / 'stdout').read_bytes() == (refdir / 'raw/stdout').read_bytes(), 'stderr': (o / 'stderr').read_bytes() == (refdir / 'raw/stderr').read_bytes(), 'effects': effects == expected}
            need(comp == run['comparisons'], 'reported comparison contradicts raw')
            dimensions = {'status': comp['exit_code'], 'stdout': comp['stdout'], 'stderr': comp['stderr'], 'mutations': effects['files'] == expected['files'], 'http_requests': effects['registry'] == expected['registry'], 'cargo_journal': effects['processes'] == expected['processes']}
            need('comparison_dimensions' not in run or run['comparison_dimensions'] == dimensions, 'dimension verdict contradicts raw')
            raw = gzip.decompress((o / 'trace.gz').read_bytes())
            need(hashlib.sha256(raw).hexdigest() == run['trace_sha256'] and sha(o / 'trace.gz') == run['trace_gzip_sha256'], 'trace digest')
            text = raw.decode(errors='replace')
            execs = successful_execs(text)
            need(len(execs) >= 2 and execs[0][1] == run['argv'][run['argv'].index('-o') + 2] and (execs[1][1] == b['argv'][-1]) and (execs[1][2] == run['workload_argv']), 'supervisor/image activation')
            imagepid = execs[1][0]
            need('libpython' not in (o / 'readelf.stdout').read_text(errors='replace').lower() and 'libpython' not in (o / 'ldd.stdout').read_text(errors='replace').lower() and (not any((l.startswith(str(imagepid) + ' ') and 'libpython' in l.lower() for l in text.splitlines()))), 'native libpython')
            source = str(Path(roles['$WORK']) / src['path'])
            need(any(('rename("' + source + '"' in l and l.endswith('= 0') for l in text.splitlines())) and any(('", "' + source + '") = 0' in l for l in text.splitlines())) and ('WIFEXITED' in text), 'missing withdrawal/restore/wait')
            source_attempt = any((l.startswith(str(imagepid) + ' ') and re.search('\\b(open|openat|openat2|creat)\\(', l) and (source in l or '.pyc"' in l or '.pyo"' in l) for l in text.splitlines()))
            all_exact &= not source_attempt
            if case == 'uv-rich':
                zipfile = roles['$CORPUS'] + '/fixtures/rich-imports.zip'
                need(run['zip_restored_sha256'] == sha(S / 'fixtures/rich-imports.zip') and any(('rename("' + zipfile + '"' in l and l.endswith('= 0') for l in text.splitlines())), 'Rich source archive not withdrawn')
                archive_attempt = any((l.startswith(str(imagepid) + ' ') and 'open' in l and (zipfile in l) for l in text.splitlines()))
                all_exact &= not archive_attempt
            if case.endswith('publisher'):
                py = roles['$PYTHON']
                stub = roles['$CORPUS'] + '/fixtures/cargo_stub.py'
                allowed = [x['argv'] for x in expected['processes']]
                need(len(execs) == 4 and [(x[1], x[2]) for x in execs[2:]] == [(py, [py, stub, *x]) for x in allowed], 'unsanctioned publisher exec/argv')
                need([x['argv'] for x in procs] == allowed and len(procs) == 2 and run['sanctioned_fixture_children_recorded'], 'fixture journal mismatch')
                need(all((re.search('^' + str(imagepid) + '\\s+wait4\\(' + str(x[0]) + ',', text, re.M) for x in execs[2:])), 'fixture child wait missing')
                need(all((any((l.startswith(str(x[0]) + ' ') and 'open' in l and (stub in l) for l in text.splitlines())) for x in execs[2:])), 'fixture child did not open sanctioned script')
                need(not any((l.startswith(str(x[0]) + ' ') and 'open' in l and (source in l) for x in execs[2:] for l in text.splitlines())), 'fixture child touched workload source')
            else:
                need(len(execs) == 2 and (not procs), 'unexpected child')
            exact = all(comp.values())
            need(r['status'] == ('observed_exact' if exact else 'observed_mismatch'), 'incorrect status')
            need(not r['paired'], 'premature credit')
            case_results[tier] = 'exact' if exact else 'mismatch'
            all_exact &= exact
        findings[case] = case_results
    if not args.partial:
        need(count == 8, 'expected 8 receipts')
    print('validated', count, 'prospective native observations; exact_all=', all_exact, 'credited=0', json.dumps(findings, sort_keys=True))
if __name__ == '__main__':
    main()
