"""Prospective shared-core controls over authentic completed and partial raw captures."""
import hashlib
import importlib.util
import json
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(ROOT / 'benchmarks/compatibility'))
import scorecard


def sha(data):
    return hashlib.sha256(data).hexdigest()


def sealed(name, seal):
    archive = HERE / 'evidence' / name
    manifest = json.loads((HERE / 'evidence' / seal).read_bytes())
    assert sha(archive.read_bytes()) == manifest['archive_sha256']
    with tarfile.open(archive) as t:
        members = {x.name: t.extractfile(x).read() for x in t.getmembers() if x.isfile()}
    assert set(members) == set(manifest['files'])
    assert all(len(b) == manifest['files'][n]['bytes'] and sha(b) == manifest['files'][n]['sha256'] for n, b in members.items())
    return members


class Completed(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.policy = json.loads((HERE / 'policy.json').read_bytes())
        cls.tmp = tempfile.TemporaryDirectory(prefix='completed-unittest-')
        root = pathlib.Path(cls.tmp.name)
        rel = cls.policy['files'][0]['path']
        p = root / rel
        p.parent.mkdir(parents=True)
        p.write_bytes(subprocess.check_output(['git', 'show', '--no-textconv', cls.policy['base_commit'] + ':' + rel], cwd=ROOT))
        assert subprocess.check_output(['git', 'rev-parse', cls.policy['base_commit'] + ':' + rel], cwd=ROOT, text=True).strip() == cls.policy['files'][0]['old_git_blob']
        assert sha(p.read_bytes()) == cls.policy['files'][0]['old_sha256']
        policy_patch = ROOT / cls.policy['patch']
        assert sha(policy_patch.read_bytes()) == cls.policy['sha256']
        subprocess.run(['git', 'apply', str(policy_patch)], cwd=root, check=True)
        assert sha(p.read_bytes()) == cls.policy['files'][0]['post_sha256']
        spec = importlib.util.spec_from_file_location('prospective_raw_control', p)
        cls.api = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.api)
        cls.members = sealed('dataclasses-959.tar.gz', 'COMPLETE.json')
        cls.recapture = sealed('identity-effects-v1.tar.gz', 'identity-effects-v1.COMPLETE.json')
        cls.canonical = [k[1:] for k in scorecard.canonical_originals('stdlib') if k[0] == 'dataclasses']
        assert len(cls.canonical) == 276

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def raw(self, tier):
        return self.members['raw/' + tier + '.stderr']

    def trace(self, tier):
        return self.members['raw/' + tier + '.trace'].decode()

    def pid(self, tier, trace=None):
        return self.api.exec_sequence((trace or self.trace(tier)).splitlines(), scorecard.require, 1 if tier == 'stock' else 2)[-1][1]

    def check(self, tier, raw=None, trace=None, exit_code=None, stdout=b'', canonical='default'):
        trace = self.trace(tier) if trace is None else trace
        return self.api.completed_unittest(self.raw(tier) if raw is None else raw, (0 if tier == 'stock' else 1) if exit_code is None else exit_code, trace, self.pid(tier, trace), scorecard.require, stdout=stdout, canonical=self.canonical if canonical == 'default' else canonical)

    def test_real_completed_nonzero_and_stock(self):
        for tier, passes in (('stock', 275), ('O0', 273), ('O3', 273)):
            with self.subTest(tier=tier):
                index = self.check(tier)
                self.assertEqual(list(index), self.canonical)
                self.assertEqual(sum(x['outcome'] == 'pass' for x in index.values()), passes)

    def test_partial_even_with_rebound_stream_and_exit(self):
        raw = b'\n'.join(self.raw('O0').splitlines()[:-1]) + b'\n'
        trace = '\n'.join(x for x in self.trace('O0').splitlines() if not ('write(2<' in x and 'summary' in x)) + '\n'
        with self.assertRaisesRegex(ValueError, 'summary'):
            self.check('O0', raw, trace)

    def test_canonical_inventory_is_mandatory_and_ordered(self):
        for canonical in (None, [], self.canonical[:-1], list(reversed(self.canonical)), self.canonical + [self.canonical[0]]):
            with self.subTest(canonical_size=len(canonical) if canonical is not None else None), self.assertRaises(ValueError):
                self.check('O0', canonical=canonical)

    def test_exact_single_final_summary_and_protocol_only(self):
        lines = self.raw('O0').splitlines(keepends=True)
        cases = [b'noise\n' + b''.join(lines), b''.join(lines) + b'noise\n', b''.join(lines[:277] + [lines[-1]] + lines[277:]), b''.join(lines) + lines[-1], b''.join(lines) + lines[1]]
        for raw in cases:
            with self.subTest(bytes=len(raw)), self.assertRaisesRegex(ValueError, 'non-event|summary'):
                self.check('O0', raw=raw)

    def test_stdout_and_stderr_must_equal_target_writes(self):
        pid = self.pid('O0')
        trace = self.trace('O0') + f'{pid} write(1<pipe:[123]>, "extra", 5) = 5\n'
        with self.assertRaisesRegex(ValueError, 'traced target writes'):
            self.check('O0', trace=trace)
        with self.assertRaisesRegex(ValueError, 'traced target writes'):
            self.check('O0', stdout=b'extra')

    def test_inconsistent_count_exit_and_truncation(self):
        with self.assertRaises(ValueError):
            self.check('O0', self.raw('O0').replace(b'"count": 276', b'"count": 275', 1))
        with self.assertRaisesRegex(ValueError, 'expected exit'):
            self.check('O0', exit_code=0)
        with self.assertRaisesRegex(ValueError, 'traced child exit'):
            self.check('O0', trace=self.trace('O0').replace('exit_group(1)', 'exit_group(0)', 1))

    def test_fabricated_pass_rebound_stderr_rejected(self):
        lines = self.raw('O0').decode().splitlines()
        found = [i for i, l in enumerate(lines) if '"event": "result"' in l]
        error = next(i for i in found if '"outcome": "error"' in lines[i])
        good = next(i for i in found if '"outcome": "pass"' in lines[i])
        prefix = 'IMRE_UNITTEST '
        a, b = (json.loads(lines[i][len(prefix):]) for i in (error, good))
        a.update(outcome='pass', details=[])
        b.update(outcome='error', details=['invented'])
        lines[error], lines[good] = (prefix + json.dumps(row, ensure_ascii=True) for row in (a, b))
        with self.assertRaisesRegex(ValueError, 'traced target writes'):
            self.check('O0', ('\n'.join(lines) + '\n').encode())

    def test_missing_imported_control_rejected(self):
        meta = json.loads(self.members['raw/gate.O0.imported.json'])
        target = meta['target_argv']; cmd = target[3:]; probe = target[0]
        poison = str(pathlib.Path(cmd[2]).parent / 'dataclasses_gate_poison.py')
        trace = self.members['raw/gate.O0.imported.trace'].decode(); diag = self.members['raw/gate.O0.imported.stderr'].decode()
        self.api.trace_gate(trace, cmd, probe, poison, diag, scorecard.require, expected_exit=1)
        bad = '\n'.join(l for l in trace.splitlines() if not (any(op in l for op in ('open(', 'openat(', 'openat2(')) and poison in l)) + '\n'
        with self.assertRaisesRegex(ValueError, 'imported poison'):
            self.api.trace_gate(bad, cmd, probe, poison, diag, scorecard.require, expected_exit=1)

    def test_nonzero_withdrawal_and_exact_failed_python_probe(self):
        trace = self.trace('O0'); meta = json.loads(self.members['raw/O0.json']); cmd = meta['target_argv']; tree, stash, leaf, image = cmd[2:]; cwd = meta['cwd']
        positive = str(pathlib.Path(tree).parent.parent.parent / 'launch/launcher.py'); archive = str(pathlib.Path(positive).parent / 'stock.source.py')
        probe = {'pid':'target','operation':'statx','path':'/workspace/scratch/imre-95908-transferred-config-v1/product/imre-toolchain/bin/python3','errno':'ENOENT','count':1}
        def verify(t): self.api.trace_withdraw(scorecard, t, image, cmd[0], tree, stash, leaf, cwd, scorecard.require, (positive, archive), True, expected_exit=1, python_probe=probe)
        verify(trace)
        line = next(l for l in trace.splitlines() if 'statx(' in l and 'bin/python3"' in l)
        for altered in (trace.replace(line, line + '\n' + line), trace.replace(line, line.replace('= -1 ENOENT (No such file or directory)', '= 0'))):
            with self.assertRaisesRegex(ValueError, 'Python|CPython'):
                verify(altered)

    def test_wrapped_policy_without_probe_still_checks_statx(self):
        text = '1 execve("/probe", [], []) = 0\n2 statx(AT_FDCWD</tmp>, "/bin/python3", AT_STATX_SYNC_AS_STAT, STATX_ALL, 0x123) = -1 ENOENT (No such file or directory)\n'
        with patch.object(self.api, 'wrapped_withdrawal', return_value=((0,'1'),(1,'2'),1)):
            with self.assertRaisesRegex(ValueError, 'CPython'):
                self.api.trace_withdraw(scorecard, text, '/image', '/probe', '/tree', '/stash', '/tree/source.py', '/tmp', scorecard.require, chain_policy={'schema':'wrapped'})

    def test_recaptured_held_live_identity_effects_and_complete_runs(self):
        for tier in ('stock','O0','O3'):
            with self.subTest(tier=tier):
                get = lambda n: self.recapture[tier+'/'+n]
                m = json.loads(get('result.json')); trace = get('run.trace').decode()
                self.assertEqual(m['held_fd_pre'],m['held_fd_post']); fd=m['held_fd_pre']; self.assertEqual(fd['fstat'],m['path_post_fstat']); self.assertEqual(fd['fstat'],m['proc_exe_sample']['fstat']); self.assertEqual(str(m['proc_exe_sample']['pid']),self.pid(tier,trace)); self.assertEqual(get('effects.before.json'),get('effects.after.json')); self.assertEqual(get('stdout'),self.members['raw/'+tier+'.stdout']); self.assertEqual(get('stderr'),self.raw(tier)); self.assertEqual(sha(get('run.trace')),m['trace_sha256']); self.assertEqual(sha(self.recapture['capture.py']),m['controller_sha256']); self.check(tier,get('stderr'),trace,m['exit_code'],get('stdout'))
        self.assertEqual(self.recapture['before.json'],self.recapture['after.json']); self.assertEqual(self.recapture['before.json'],self.recapture['post-gates.json'])

    def test_live_gate_compiler_identity_and_no_image(self):
        bracket=json.loads(self.recapture['gates/bracket.json']); positive=self.members['launch/launcher.py']
        self.assertEqual(bracket['held_before'],bracket['held_after']); self.assertEqual(bracket['launcher_sha256'],sha(positive))
        negative={'direct':positive.replace(b'import unittest\n',b'import unittest\nstatus: int = True\n',1),'imported':positive.splitlines(keepends=True)[0]+b'import dataclasses_gate_poison\n'+b''.join(positive.splitlines(keepends=True)[1:])}
        for tier in ('O0','O3'):
            build=json.loads(self.members['raw/'+tier+'.build.json'])
            for mode in ('direct','imported'):
                with self.subTest(tier=tier,mode=mode):
                    prefix='gates/gate.'+tier+'.'+mode; m=json.loads(self.recapture[prefix+'.json']); trace=self.recapture[prefix+'.trace'].decode(); first,child=self.api.execs(trace.splitlines(),scorecard.require); sample=m['live_compiler_sample']; diag=self.recapture[prefix+'.stderr'].decode()
                    self.assertEqual(m['held_before'],m['held_after']); self.assertEqual(m['held_before'],bracket['held_before']); self.assertEqual(sample['pid'],int(child[1])); self.assertEqual(sample['fstat'],m['held_before']['cli']['fstat_before_read']); self.assertEqual(sample['target'],child[2]); self.assertLess(m['start_utc_ns'],sample['sample_utc_ns']); self.assertLess(sample['sample_utc_ns'],m['finish_utc_ns'])
                    for key in ('cli','runtime','clang'):
                        self.assertEqual(m['held_before'][key]['sha256'],build['held_before'][key]['sha256']); self.assertEqual(m['held_before'][key]['fstat_before_read'],build['held_before'][key]['fstat_before_read'])
                    self.assertEqual(self.api.exec_environment(trace.splitlines()[child[0]],scorecard.require),build['environment']); self.assertEqual(self.api.child_exit_code(trace.splitlines(),child[1],scorecard.require),1); self.assertEqual(scorecard.comparable_build(child[3]),scorecard.comparable_build(build['target_argv'])); self.assertEqual(m['negative_launcher_sha256'],sha(negative[mode])); self.assertTrue(m['image_absent_before'] and m['image_absent_after']); self.assertEqual(sha(self.recapture['gate_capture.py']),m['controller_sha256']); self.assertEqual(sha(self.members['controller.py']),m['frozen_controller_sha256']); self.assertEqual(sha(self.recapture[prefix+'.trace']),m['trace_sha256'])
                    if mode=='direct':self.api.trace_direct_gate(trace,child[3],first[2],diag,scorecard.require,expected_exit=1)
                    else:self.api.trace_gate(trace,child[3],first[2],str(pathlib.Path(child[3][2]).parent/'dataclasses_gate_poison.py'),diag,scorecard.require,expected_exit=1)

    def test_stage_zero_credit_closure_and_all_original_outcomes(self):
        m=json.loads((HERE/'manifest.json').read_bytes()); actual={p.relative_to(ROOT).as_posix():sha(p.read_bytes()) for p in HERE.rglob('*') if p.is_file() and p.name!='manifest.json'}
        self.assertEqual(m['files'],actual); self.assertEqual(m['credit'],0)
        rows=[json.loads(line) for line in (HERE/'templates/dataclasses.jsonl').read_bytes().splitlines()]
        self.assertEqual([(r['id'],r['occurrence']) for r in rows],self.canonical); self.assertTrue(all(r['paired'] is False for r in rows)); self.assertEqual(sum(all(r['runs'][t]['outcome']=='pass' for t in scorecard.TIERS) for r in rows),273)
        for t in scorecard.TIERS:
            parsed=self.check(t)
            self.assertEqual([r['runs'][t]['outcome'] for r in rows],[v['outcome'] for v in parsed.values()])

if __name__ == '__main__':
    unittest.main()
