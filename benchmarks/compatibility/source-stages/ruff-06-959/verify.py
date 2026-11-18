"""Focused offline validation for the inactive Ruff-06 selective source stage."""
from pathlib import Path
import hashlib
import json
import re
import subprocess

HERE = Path(__file__).resolve().parent
SOURCE = HERE / 'source'
EVIDENCE = HERE / 'evidence'

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def load(path):
    return json.loads(path.read_text())

def check(condition, message):
    if not condition:
        raise SystemExit(message)

def main():
    custody = load(SOURCE / 'custody.json')
    binding = load(HERE / 'historical-binding.json')
    stage = load(HERE / 'stage.json')
    check(digest(SOURCE / 'custody.json') == 'b45e69d48ff1c36d006291b6060551a369824af4cebf2b40dacb0cf3d42d0d84', 'custody hash')
    check(len([p for p in SOURCE.rglob('*') if p.is_file()]) == 12, 'selective source footprint')
    before = load(SOURCE / 'stock/before.json')
    check(before == {f['path']: f['sha256'] for f in custody['fixture']}, 'physical83 before manifest')
    for f in custody['fixture']:
        path = SOURCE / 'fixture' / f['path']
        check(path.stat().st_size == f['bytes'] and digest(path) == f['sha256'], f"fixture {f['path']}")
    for name, f in custody['raw_stock'].items():
        path = SOURCE / name
        check(path.stat().st_size == f['bytes'] and digest(path) == f['sha256'], f'raw stock {name}')
    authority = subprocess.check_output(['git', '-C', str(HERE), 'show', binding['authority_commit'] + ':' + binding['authority_path']])
    record = next(line for line in authority.splitlines() if json.loads(line).get('id') == 'ruff-06')
    original = json.loads(record)
    check(hashlib.sha256(record).hexdigest() == binding['authority_record_sha256'] and original['original_argv'] == binding['original_argv'] and original['source'] == binding['source'], 'authority Git record binding')
    entry = (SOURCE/'fixture/scripts/check-scripts.py').read_bytes()
    blob = hashlib.sha1(b'blob ' + str(len(entry)).encode() + b'\0' + entry).hexdigest()
    check(blob == binding['source']['git_blob'], 'Ruff public Git blob')
    wait = load(SOURCE/'stock/cli.wait.json')
    check(wait['returncode'] == 0 and wait['raw_wait'] == 0 and wait['pid'] > 0 and wait['stdout_sha256'] == digest(SOURCE/'stock/cli.stdout') and wait['stderr_sha256'] == digest(SOURCE/'stock/cli.stderr'), 'later raw PID wait and streams')
    check(binding['authority_commit'] == '880648b620f57905a495eb51fbb4600755ee0c54' and binding['source']['commit'] == '65aee8892ce379af4ae9c8b5f6e0ab37b25d5ec9', 'historical authority')
    check(binding['source']['sha256'] == before['scripts/check-scripts.py'] and binding['original_argv'][1:] == ['-BS', '/dev/shm/imre-frozen-4894/stock/O0/ruff-06/ruff-work/script_list/scripts/check-scripts.py', '--write-ty-args-file', 'scripts.txt'], 'original source and argv')
    check(binding['later_stock_is_historical'] is False and binding['later_stock_observation_date'] is None and binding['archive_preexistence_claim'] is False, 'later stock/time labels')
    check(stage['activation'] is False and stage['credit'] == 0 and stage['stock_observation_kind'] == 'LATER', 'inactive stage')
    expected = b'tools/rotate.py\nscripts/check-scripts.py\n'
    withdrawn = b'tools/rotate.py\n'
    ref = load(SOURCE / 'stock/effects.json')
    check(ref == {'scripts.txt': [None, hashlib.sha256(expected).hexdigest()]}, 'later full effect')
    for tag in ('stock','cli-O0','cli-O3','retained-O0','retained-O3','withdraw-O0','withdraw-O3'):
        run = load(EVIDENCE / (tag + '.json'))
        check(run['exit_code'] == 0 and not run['timed_out'] and run['matches_later_streams'], f'{tag} exit/streams')
        check(digest(EVIDENCE / run['stdout_file']) == run['stdout_sha256'] and digest(EVIDENCE / run['stderr_file']) == run['stderr_sha256'], f'{tag} raw streams')
        expected_before = dict(before)
        if tag.startswith('withdraw'):
            del expected_before['scripts/check-scripts.py']
        check(run['before'] == expected_before, f'{tag} exact fixture before')
        check(run['observed_root_order'] == ['tools','crates','ignored','.gitignore','scripts'], f'{tag} traversal')
        postimage = (EVIDENCE / (tag + '.scripts.txt')).read_bytes()
        check(postimage == (withdrawn if tag.startswith('withdraw') else expected), f'{tag} postimage bytes')
        check(run['effects'] == {'scripts.txt': [None, hashlib.sha256(postimage).hexdigest()]}, f'{tag} complete effects')
        check(run['matches_later_effect'] == (not tag.startswith('withdraw')), f'{tag} effect divergence')
        if tag.startswith(('retained','withdraw')):
            tracepath = EVIDENCE / run['trace_file']; trace = tracepath.read_text(); lines=trace.splitlines()
            check(digest(tracepath) == run['trace_sha256'], f'{tag} trace digest')
            execs=[l for l in lines if re.search(r'\bexecve(?:at)?\(',l)]
            check(len(execs)==1 and 'ruff06-O' in execs[0] and not any(re.search(r'\b(?:clone3?|fork|vfork)\(',l) for l in lines), f'{tag} child execution')
            opened=[]
            for line in lines:
                m=re.search(r'\bopen(?:at2?)?\([^\"]*\"([^\"]+)\"[^\n]*',line)
                if m and re.search(r'\.(?:py|pyc|pyo|zip|whl|egg|pyz|a|tar(?:\.[^/]+)?)(?:$|/)',m.group(1)):
                    path=m.group(1);opened.append(path)
                    check(path.endswith(('/tools/rotate.py','/scripts/check-scripts.py')) and 'O_RDONLY' in line, f'{tag} unapproved source/archive open: {line}')
            check(any(p.endswith('/tools/rotate.py') for p in opened), f'{tag} data helper read')
            check(any(p.endswith('/scripts/check-scripts.py') for p in opened) == tag.startswith('retained'), f'{tag} entry data read')
            if tag.startswith('withdraw'):
                check(run['preserved_sha256']==binding['source']['sha256']==run['restored_sha256'] and run['effects_from_original_preimage']['scripts/check-scripts.py']==[binding['source']['sha256'],None], f'{tag} withdrawal custody')
    poison=load(EVIDENCE/'poison.json')
    check(len(poison)==4 and all(x['exit_code']==1 and not x['image_exists'] and 'expected int, got bool' in x['diagnostic'] for x in poison.values()), 'direct/imported controls')
    for path, expected_hash in stage['evidence_sha256'].items():
        check(digest(HERE/path)==expected_hash, f'evidence hash {path}')
    print('Ruff-06 selective source and bounded controls verified; inactive, zero credit')

if __name__ == '__main__':
    main()
