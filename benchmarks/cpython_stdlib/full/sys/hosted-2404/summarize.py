"""Validate earlier positive observations and preserve every measured failure."""
from collections import Counter
import json
from pathlib import Path
import sys

folder=Path(__file__).resolve().parent
load=lambda name:json.loads((folder/name).read_bytes())
pair=load('pair.json')
src=pair['source']
expected=load('known-passing.json')
checks=load('controls.json')
registered=load('registered-oserror-'+src+'.json')
assert registered['pair']==pair==checks['pair']
result={'pair':pair,'application_replays':0,'optimizations':{}}
errors=[]
for opt in ('0','3'):
    level='O'+opt
    click=checks['groups']['click_textio']['native'][opt]
    outcomes={k:v['matches'] for k,v in click['cases'].items()}
    assert len(outcomes)==7 and click['boxed_entry'] and set(outcomes)==set(checks['groups']['click_textio']['cpython'])
    original_failures={k:click['cases'][k]['run'] for k,v in outcomes.items() if not v}
    if set(original_failures)-{'drop_keeps_original_buffer_open'}: errors.append([level,'Click regression',list(original_failures)])
    callback=checks['groups']['registered_callback']['native'][opt]['cases']['']['matches']
    rows={};neg={}
    for label in ('upstream-io','upstream-io-lifetime','upstream-io-chunk-v2'):
        data=load(f'{label}-{src}.json')
        assert data['pair']==pair and data['pin']=='5607950ef232dad16d75c0cf53101d9649d89115'
        assert data['blob']=='57b42fbb65fe107b1573a2a6e0848de942ff8367' and data['license_blob']=='20cf39097c68baa17cc566b64e76d34ebf034044'
        assert data['cpython_outcome']=='all selected passed'
        ids=data['full_test_io_ids']; wrappers=data['full_CTextIOWrapperTest_ids']
        assert len(ids)==667 and len(wrappers)==82
        neg[label]=data['levels'][level]['negative_control']['outcome']
        for row in data['levels'][level]['rows']:
            assert row['id'] not in rows and not row['fully_native'] and row['roles']['fixture']=='cpython'
            if row['outcome']=='passed':
                assert row['roles']=={'fixture':'cpython','body':'native','assertions':'native','operation':'native'}
            rows[row['id']]={k:row[k] for k in ('id','occurrence','optimization','outcome','roles','body_sha256')}
            if row['outcome']!='passed': rows[row['id']]['diagnostic']=row.get('run',row['build'])['stderr'].splitlines()[-1:]
    assert len(rows)==27
    for name in expected[level]:
        if rows[name]['outcome']!='passed':errors.append([level,'previously passing original',name])
    natives=registered['levels'][opt]
    for name,item in natives['cases'].items():
        if not item['match']:errors.append([level,'registered source',name,item['native']])
    if not callback:errors.append([level,'registered_error_handler'])
    if set(neg.values())!={'passed'}:errors.append([level,'negative controls',neg])
    result['optimizations'][level]={'click':{'total':7,'matches':sum(outcomes.values()),'failures':original_failures},'callback':callback,'registered':{k:v['match'] for k,v in natives['cases'].items()},'test_io':{'complete_suite_count':len(ids),'wrapper_class_count':len(wrappers),'selected':len(rows),'fully_native':0,'counts':dict(Counter(r['outcome'] for r in rows.values())),'per_occurrence':list(rows.values()),'negative_controls':neg}}
result['unexpected_failures']=errors+checks.get('unexpected_failures',[])
(folder/'qualification.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({k:{'Click':v['click']['matches'],'test_io':v['test_io']['counts'],'registered':sum(v['registered'].values()),'callback':v['callback']} for k,v in result['optimizations'].items()}))
if result['unexpected_failures']:
    print(result['unexpected_failures'])
    sys.exit(1)
