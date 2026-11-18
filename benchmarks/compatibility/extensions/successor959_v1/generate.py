"""Produce an inactive candidate receipt from exact sealed finite-run events."""
from pathlib import Path
import json
import raw_control


def finite(api, binding, destination):
    roles=binding['roles'];source=binding['source'];originals=json.loads((api.ROOT/roles['originals']).read_bytes());observed=[json.loads(x) for x in (api.ROOT/roles['observed']).read_text().splitlines()];run=json.loads((api.ROOT/roles['run']).read_bytes());assessment=json.loads((api.ROOT/roles['assessment']).read_bytes());inventories={tier:raw_control.event_index(api.ROOT/roles[tier+'_stderr'],api.require) for tier in api.TIERS}
    keys=[(x['id'],x['occurrence']) for x in originals];api.require(len(keys)==len(set(keys))==assessment['observed_originals'] and all(set(index)==set(keys) for index in inventories.values()) and [(x['id'],x['occurrence']) for x in observed]==keys, 'finite sealed original inventory differs')
    rows=[]
    for item in originals:
        key=(item['id'],item['occurrence']);row={'schema':api.SCHEMA,'kind':'stdlib','family':binding['family'],'id':key[0],'occurrence':key[1],'source':source,'imre_revision':assessment['revision'],'platform':dict(api.CANONICAL_COHORT['platform']),'omission_reason':None,'paired':False,'credit_reason':binding['identity_blocker'],'runs':{}}
        api.canonical_original(row,'stdlib')
        for tier in api.TIERS:
            meta=run['runs'][tier];event=inventories[tier][key];row['runs'][tier]={'outcome':event['outcome'],'argv':meta['argv'],'environment':meta['environment'],'evidence':roles[tier+'_stderr']+'#L'+str(event['line'])}
            if tier=='stock':row['runs'][tier].update(version=api.CANONICAL_COHORT['cpython_version'],full_version=assessment['stock_python_version'])
        rows.append(row)
    destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(''.join(json.dumps(row,sort_keys=True,separators=(',',':'))+'\n' for row in rows))
    return rows
