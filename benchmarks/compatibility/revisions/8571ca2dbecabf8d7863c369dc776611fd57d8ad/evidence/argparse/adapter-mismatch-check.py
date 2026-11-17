"""Probe the staged argparse candidate against the real raw-control adapter."""
import copy, hashlib, json, pathlib, sys, tempfile
from unittest.mock import patch
D=pathlib.Path(__file__).resolve().parents[6];sys.path.insert(0,str(D/'benchmarks/compatibility'));import scorecard
R='8571ca2dbecabf8d7863c369dc776611fd57d8ad';S=D/'benchmarks/compatibility/revisions'/R;E=S/'evidence/argparse';entry=json.loads((E/'activation-entry.json').read_text());binding=json.loads((E/'raw-control/binding.json').read_text());rows=[json.loads(x) for x in (S/'stdlib/argparse.jsonl').read_text().splitlines()];selected=next(x for x in rows if x['paired']);full=json.loads((E/'stock.json').read_text());full_event=next(x for x in full['originals'] if (x['id'],x['occurrence'])==(selected['id'],selected['occurrence']));full_run=json.loads((E/'run.json').read_text());activation=json.loads((S/'activation.json').read_text());assert len(activation['families'])==6

def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def loc(p):return p.relative_to(D).as_posix()
def check(row,e):
 a=copy.deepcopy(activation);a['families']['argparse']=e
 with patch.object(scorecard,'DATA_ROOT',S),patch.object(scorecard,'COHORT',{**scorecard.CANONICAL_COHORT,'imre_revision':R}),patch.object(scorecard,'snapshot_activation',return_value=a):
  scorecard.validate_record(row,'stdlib',scorecard.families())
def trial(name,row,e):
 try:check(row,e);return {'name':name,'accepted':True}
 except (ValueError,KeyError,TypeError) as ex:return {'name':name,'accepted':False,'reason':str(ex)}
def rebound(tmp,b,e):
 p=tmp/'binding.json';p.write_text(json.dumps(b)+'\n');e['sources']['binding']=loc(p);e['files'][loc(p)]=digest(p);return e
results=[trial('correct finite stock/O0/O3 row',selected,entry)]
wrong=copy.deepcopy(selected);wrong['runs']['stock']['evidence']=loc(E/'stock.stderr')+f'#L{full_event["line"]}';results.append(trial('full-stock evidence substituted without changing finite cohort',wrong,entry))
with tempfile.TemporaryDirectory(dir=E) as tmp:
 tmp=pathlib.Path(tmp);b=copy.deepcopy(binding);e=copy.deepcopy(entry);row=copy.deepcopy(selected);row['runs']['stock'].update(argv=full_run['runs']['stock']['argv'],environment=full_run['runs']['stock']['environment'],evidence=loc(E/'stock.stderr')+f'#L{full_event["line"]}');trows=copy.deepcopy(rows);trows[next(i for i,x in enumerate(trows) if x['id']==row['id'] and x['occurrence']==row['occurrence'])]=row;p=tmp/'receipt.jsonl';p.write_text(''.join(json.dumps(x)+'\n' for x in trows));e['receipt']=loc(p);e['files'][loc(p)]=digest(p);next(x for x in b['originals'] if x['id']==row['id'] and x['occurrence']==row['occurrence'])['launcher_cohort']='full-stock-inventory';e=rebound(tmp,b,e);results.append(trial('rehashed paired row and mapping moved to full-stock inventory cohort',row,e))
with tempfile.TemporaryDirectory(dir=E) as tmp:
 tmp=pathlib.Path(tmp);b=copy.deepcopy(binding);e=copy.deepcopy(entry);r=json.loads((E/'run.finite.json').read_text());r['runs']['stock']['fresh_stock_launcher_sha256']=digest(E/'stock.launcher.py');p=tmp/'run.json';p.write_text(json.dumps(r)+'\n');e['files'][loc(p)]=digest(p);b['cohorts']['finite-metavar']['artifacts'].update(run=loc(p),stock_launcher=loc(E/'stock.launcher.py'));e=rebound(tmp,b,e);results.append(trial('rehashed finite cohort substitutes different full-suite stock launcher archive',selected,e))
out={'adapter':'raw-control-v1','existing_active_families':list(activation['families']),'checks':results,'activation_ready':results[0]['accepted'] and all(not x['accepted'] for x in results[1:]),'blocker':'The adapter accepts rehashed full-stock/finite-native configuration mismatches.' if any(x['accepted'] for x in results[1:]) else None};(E/'adapter-readiness.json').write_text(json.dumps(out,indent=2)+'\n');print(json.dumps(out,indent=2))
