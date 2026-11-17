"""Verify raw depfile source bytes and the three tracked d26 CRLF transforms."""
from __future__ import annotations
import hashlib, json, shlex, subprocess, sys
from pathlib import Path

REV = 'd26f19448c70c00039c305f105a85c215f820577'
PREFIX = 'crates/imre-compiler/stdlib/venv/scripts/'
SPECIAL = {PREFIX+'common/Activate.ps1', PREFIX+'nt/activate.bat', PREFIX+'nt/deactivate.bat'}
ATTRS = {'text':'set','eol':'crlf','filter':'unspecified','working-tree-encoding':'unspecified','ident':'unspecified'}

def git(source, *args):
    return subprocess.check_output(['git','-C',str(source),*args])

def sha(data): return hashlib.sha256(data).hexdigest()
def oid(data): return hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()

def verify(source, evidence):
    source=source.resolve(strict=True)
    assert git(source,'rev-parse','HEAD').decode().strip()==REV
    tree={}
    for row in git(source,'ls-tree','-r','-z','--full-tree',REV).split(b'\0'):
        if row:
            meta,path=row.split(b'\t',1); mode,kind,blob=meta.decode().split(); tree[path.decode()]=(mode,kind,blob)
    attrs=git(source,'show',REV+':.gitattributes'); assert b'crates/imre-compiler/stdlib/venv/scripts/common/Activate.ps1 text eol=crlf\n' in attrs and b'crates/imre-compiler/stdlib/venv/scripts/nt/*.bat text eol=crlf\n' in attrs
    result={'derivation':'retrospective post-capture source-only verification; not a contemporaneous measurement','revision':REV,'tracked_gitattributes':{'blob_oid':tree['.gitattributes'][2],'content_sha256':sha(attrs),'rules':['crates/imre-compiler/stdlib/venv/scripts/common/Activate.ps1 text eol=crlf','crates/imre-compiler/stdlib/venv/scripts/nt/*.bat text eol=crlf']},'depfiles':{},'transforms':[]}
    generated_audit=json.loads((evidence/'depfile_audit.json').read_bytes())['generated']
    allowed_generated={row['path']:row for row in generated_audit}
    for key in ('compiler','runtime'):
        data=(evidence/'identity'/(key+'.d')).read_bytes(); lexical=shlex.split(data.decode().splitlines()[0].partition(': ')[2]); exact=0; transformed=0; generated=0
        for item in lexical:
            path=Path(item).resolve(strict=True)
            if not path.is_relative_to(source):
                row=allowed_generated[str(path)]; assert Path(item)==path and row['crate']=='imre-'+key and row['sha256']==sha(path.read_bytes()); generated+=1; continue
            rel=path.relative_to(source).as_posix(); mode,kind,blob=tree[rel]; assert mode in ('100644','100755') and kind=='blob'; raw=path.read_bytes()
            if oid(raw)==blob: exact+=1; continue
            assert key=='compiler' and rel in SPECIAL
            canonical=git(source,'show',REV+':'+rel); assert oid(canonical)==blob and b'\r' not in canonical and raw==canonical.replace(b'\n',b'\r\n') and raw.replace(b'\r\n',b'\n')==canonical
            fields=git(source,'check-attr','--source='+REV,'-z',*ATTRS,'--',rel).split(b'\0'); observed={fields[n+1].decode():fields[n+2].decode() for n in range(0,len(fields)-1,3)}; assert observed==ATTRS
            result['transforms'].append({'path':rel,'raw_sha256':sha(raw),'raw_bytes':len(raw),'canonical_blob_oid':blob,'canonical_content_sha256':sha(canonical),'canonical_bytes':len(canonical),'attributes':observed,'relation':'raw == canonical.replace(LF, CRLF); canonical == raw.replace(CRLF, LF); canonical contains no CR'})
            transformed+=1
        result['depfiles'][key]={'raw_exact':exact,'deterministic_attribute_transforms':transformed,'generated_separately_bound':generated,'inputs':len(lexical),'raw_depfile_sha256':sha(data)}
    assert result['depfiles']['compiler']['raw_exact']==776 and result['depfiles']['compiler']['deterministic_attribute_transforms']==3 and result['depfiles']['compiler']['generated_separately_bound']==2
    assert result['depfiles']['runtime']['raw_exact']==599 and result['depfiles']['runtime']['deterministic_attribute_transforms']==0 and result['depfiles']['runtime']['generated_separately_bound']==5
    assert {x['path'] for x in result['transforms']}==SPECIAL
    result['transforms'].sort(key=lambda x:x['path']); return result

if __name__=='__main__':
    print(json.dumps(verify(Path(sys.argv[1]),Path(sys.argv[2])),indent=2,sort_keys=True))
