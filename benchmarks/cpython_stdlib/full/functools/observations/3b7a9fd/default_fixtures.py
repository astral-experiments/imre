"""Try the pinned, complete primary or companion suite with native default fixtures."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import sysconfig
import tarfile
import tempfile

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path)
    parser.add_argument("--companion", action="store_true")
    args = parser.parse_args()
    receipt=args.snapshot.resolve()
    suite=receipt/"suite/full/functools"
    sys.path.insert(0,str(suite.parents[1]))
    from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
    from suite_evidence import build_report, occurrence_keys, validate_pair
    from suite_native import NativeDefaultMethod, Toolchain
    assert sys.implementation.name=="cpython" and sys.version_info[:3]==(3,14,5)
    if args.companion:
        assert sysconfig.get_config_var("Py_GIL_DISABLED") and not sys._is_gil_enabled()
    hashes=json.loads((receipt/"binary-hashes.json").read_bytes())
    for file,digest in hashes.items():
        with (receipt/file).open("rb") as stream:
            assert hashlib.file_digest(stream,"sha256").hexdigest()==digest
    suffix="-free-threading" if args.companion else ""
    manifest=json.loads((suite/("FREE_THREADING.json" if args.companion else "UPSTREAM.json")).read_bytes())
    verify_files(suite/"upstream",manifest["files"])
    verify_interpreter_tests(manifest)
    reference=json.loads((suite/f"reference{suffix}-linux.json").read_bytes())
    name=manifest["module"]; source="Lib/"+name.replace(".","/")+".py"
    discovered=list(cases(load_suite(name)))
    keys=list(occurrence_keys(c.id() for c in discovered))
    assert len(keys)==reference["denominator"]==(4 if args.companion else 325)
    assert set(c.id() for c in discovered)==set(reference["reference"])
    variants=[(case.id(),type(case).__name__) for case in discovered]
    git=Path("/home/dev-user/.cache/imre-cpython-suite-v3.14.5.git")
    result={"compiler_commit":receipt.name,"binary_sha256":hashes,"cpython_commit":manifest["commit"],"modes":{}}
    with tempfile.TemporaryDirectory(prefix="functools-native-default-") as tmp:
        root=Path(tmp)/"Lib"
        with tempfile.TemporaryFile() as archive:
            subprocess.run(["git","-C",git,"archive","--format=tar",manifest["commit"],"--","Lib/test"],stdout=archive,check=True)
            archive.seek(0)
            with tarfile.open(fileobj=archive) as tar:
                tar.extractall(tmp,filter="data")
        for reach in (False,True):
            mode="reachable" if reach else "ordinary"
            pair=[]
            for level in (0,3):
                tools=Toolchain(receipt/"imre",receipt/"libimre_runtime.a",Path("/usr/bin/clang-18"),level,reach,root)
                observations={}
                for key,case in zip(keys,discovered,strict=True):
                    with tempfile.TemporaryDirectory(dir=root,prefix="case_") as scratch:
                        op=("unobserved","Native module operation requires review if the original method completes.")
                        entry=NativeDefaultMethod(tools,Path(scratch),suite/"upstream"/source,manifest["files"][source],op)
                        row=entry.run(case)
                        if "reason" in row:
                            row["reason"]=row["reason"].replace(scratch+"/","<case>/").replace(str(root)+"/","Lib/")
                        if row["outcome"]!="blocked":
                            print(mode,level,case.id(),row["outcome"],flush=True)
                        observations[key]=row
                report=build_report(reference,level,variants,observations)
                print(mode,level,report["counts"],Counter(r["evidence"].get("stage") for r in report["records"]),flush=True)
                pair.append(report)
            validate_pair(pair,reference)
            result["modes"][mode]=pair
    (receipt/f"default-fixtures{suffix}-linux.json").write_text(json.dumps(result,indent=2)+"\n")

if __name__=="__main__":
    main()
