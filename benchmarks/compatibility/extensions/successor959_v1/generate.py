"""Generate only inactive finite receipts; counting requires public activation authority."""
import json
from pathlib import Path
import raw_control
from extensions.successor959_v1 import authority


def finite(api,binding,destination):
    roles=binding['roles'];delta=binding['identity']
    def read(role):
        loc=roles[role]
        if '!' in loc:
            arc,member=loc.split('!',1);return authority.archive_members(api,arc,json.dumps(delta['archives'][arc],sort_keys=True))[member]
        return authority.safe_path(api,loc).read_bytes()
    originals=json.loads(read('originals'));observed=[json.loads(x) for x in read('observed').splitlines()];assessment=json.loads(read('assessment'));run=json.loads(read('run'));metas={**run['runs'],'stock':json.loads(read('stock_meta')),'O0':json.loads(read('O0_meta'))};inventories={t:{(x['id'],x['occurrence']):x for x in raw_control.parse(read(t+'_stderr'),False)['originals']} for t in api.TIERS};keys=[(x['id'],x['occurrence']) for x in originals]
    api.require(len(keys)==len(set(keys))==assessment['observed_originals'] and [(x['id'],x['occurrence']) for x in observed]==keys and all(set(i)==set(keys) for i in inventories.values()),'finite original occurrence inventory differs');rows=[]
    for item in originals:
        key=(item['id'],item['occurrence']);row={'schema':api.SCHEMA,'kind':'stdlib','family':binding['family'],'id':key[0],'occurrence':key[1],'source':binding['source'],'imre_revision':assessment['revision'],'platform':dict(api.CANONICAL_COHORT['platform']),'omission_reason':None,'paired':False,'credit_reason':delta['credit_reason'],'runs':{}};api.canonical_original(row,'stdlib')
        for tier in api.TIERS:
            event=inventories[tier][key];meta=metas[tier];row['runs'][tier]={'outcome':event['outcome'],'argv':meta['argv'],'environment':meta['environment'],'evidence':roles[tier+'_stderr']+'#L'+str(event['line'])}
            if tier=='stock':row['runs'][tier].update(version=api.CANONICAL_COHORT['cpython_version'],full_version=assessment['stock_python_version'])
        rows.append(row)
    destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(''.join(json.dumps(x,sort_keys=True,separators=(',',':'))+'\n' for x in rows));return rows
