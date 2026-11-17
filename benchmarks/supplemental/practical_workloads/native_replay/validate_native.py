"""Derive prospective case-level strict pairs from immutable stock and raw native evidence."""
from __future__ import annotations
import argparse
import ast
import hashlib
import json
import os
import posixpath
import re
import shlex
import subprocess
import sys
from pathlib import Path
from collections import Counter

HERE = Path(__file__).resolve().parent
COMPAT = HERE.parents[2] / "compatibility"
sys.path.insert(0, str(COMPAT))
import raw_control as raw
import scorecard

PIN = "99964c6734dbe008d69d5016eb60d25abd31ab0d"
PY_IDENTITY = "import json,sys; print(json.dumps({\"executable\":sys.executable,\"prefix\":sys.prefix},sort_keys=True))"
# This exception authorizes only the two frozen cargo-fixture invocations; it does not grant credit.
FIXTURE_CASES = {"ruff-publisher", "uv-publisher"}
RUN_SYSCALLS = "execve,execveat,open,openat,openat2,creat,io_uring_setup,clone,clone3,fork,vfork,rename,renameat,renameat2,newfstatat,wait4,exit_group"
GATE_SYSCALLS = "execve,execveat,newfstatat,openat,openat2,clone,clone3,fork,vfork,wait4,exit_group"
ACTIVATION_COMMIT = "4f4dafbe58cc097aa6c637b05c8e1c47eb0771ce"
ACTIVATION_BLOB = "9047da99cc905b4726688ed949d9737b467427f5"
ACTIVATION_SHA = "d9c7f0aa6061b104b24dfca242227c7ce753efee84764d7f3c95056f557f2c67"
CORRECTION_VERIFIER_SHA = "798872e2cac0d7fe50b53ff86ad824820f501b5270da3bea9d824e68b4516ea4"
CRLF_PREFIX = "crates/imre-compiler/stdlib/venv/scripts/"
CRLF_PATHS = {CRLF_PREFIX+p for p in ("common/Activate.ps1", "nt/activate.bat", "nt/deactivate.bat")}
CRLF_ATTRS = {"text":"set", "eol":"crlf", "filter":"unspecified", "ident":"unspecified", "working-tree-encoding":"unspecified"}
GENERATED = {"compiler":{"imre-stdlib-sources.rs", "imre-stdlib.zip"}, "runtime":{"unicode_alnum.rs", "unicode_alpha.rs", "unicode_general_categories.rs", "unicode_identifier_continue.rs", "unicode_identifier_start.rs"}}


def need(ok, message):
    if not ok:
        raise ValueError(message)


def sha(path):
    return raw.sha(Path(path).read_bytes())


def load(path):
    return json.loads(Path(path).read_bytes())


def git(repo, *args):
    return subprocess.check_output(["git","-C",str(repo),*args])

def public_commit(repo, commit):
    need(bool(re.fullmatch(r"[0-9a-f]{40}",commit)),"exact public commit")
    need(subprocess.run(["git","-C",str(repo),"merge-base","--is-ancestor",commit,"refs/remotes/origin/main"],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0,"commit is not on fetched public main")

def decode_policy(data):
    obj=json.loads(data)
    need(set(obj)=={"schema","stock_commit","cases","fixture","stdlib"} and obj["schema"]=="practical-workload-fixture-policy-v1" and obj["stock_commit"]==PIN and isinstance(obj["cases"],list) and len(obj["cases"])==2 and set(obj["cases"])==FIXTURE_CASES,"exact frozen fixture policy header/cases")
    fixture=obj["fixture"];stock=HERE.parent/"stock_v1";tails=None
    need(set(fixture)=={"executable_role","executable_sha256","stock_script","script_sha256","argv_tails"} and fixture["executable_role"]=="$PYTHON" and fixture["stock_script"]=="benchmarks/supplemental/practical_workloads/stock_v1/fixtures/cargo_stub.py","exact fixture policy fields/roles")
    for case in sorted(FIXTURE_CASES):
        ref=load(stock/"references"/case/"reference.json");commands=[p["argv"] for p in load(stock/"references"/case/"effects.normalized.json")["processes"]]
        need(fixture["executable_sha256"]==ref["tool"]["executable_sha256"] and (tails is None or tails==commands),"frozen private executable/command scope");tails=commands
    need(fixture["script_sha256"]==sha(stock/"fixtures/cargo_stub.py") and fixture["argv_tails"]==tails,"frozen stub and exact two command tails")
    stdlib=obj["stdlib"]
    need(set(stdlib)=={"implementation","version","root_relative_to_executable_directory","profile","files_sha256"} and stdlib["implementation"]=="CPython" and stdlib["version"]=="3.14.5" and stdlib["profile"]=="private-bytecode-free-cpython-copy" and stdlib["root_relative_to_executable_directory"]=="../lib/python3.14" and isinstance(stdlib["files_sha256"],dict) and len(stdlib["files_sha256"])==25,"exact frozen stdlib policy")
    for path,digest in stdlib["files_sha256"].items():need(isinstance(path,str) and path.endswith(".py") and path==posixpath.normpath(path) and not path.startswith(("/","../")) and isinstance(digest,str) and bool(re.fullmatch(r"[0-9a-f]{64}",digest)),"unsafe stdlib source path/hash")
    return {"stdlib":stdlib["files_sha256"],"stub_sha256":fixture["script_sha256"],"executable_sha256":fixture["executable_sha256"],"argv_tails":tails,"root_relative":stdlib["root_relative_to_executable_directory"]}

def fixture_root(executable, prefix, relative):
    root=Path(posixpath.normpath(str(Path(executable).parent/relative)));need(Path(executable).is_absolute() and Path(prefix).is_absolute() and relative=="../lib/python3.14" and Path(posixpath.normpath(prefix))==Path(executable).parent.parent and root==Path(posixpath.normpath(prefix))/"lib/python3.14","effective private sys.prefix/stdlib root")
    return root

def policy_blob(repo, binding):
    public_commit(repo,binding["commit"]);path=binding["path"]
    need(isinstance(path,str) and path.startswith("benchmarks/") and path==posixpath.normpath(path),"reviewed fixture policy path")
    spec=binding["commit"]+":"+path;blob=git(repo,"rev-parse",spec).decode().strip();data=git(repo,"show",spec)
    need(blob==binding["blob"] and raw.sha(data)==binding["sha256"],"reviewed fixture policy blob/hash")
    return decode_policy(data)

def activation(repo, revision, commit, expected_blob, expected_sha):
    need((commit,expected_blob,expected_sha)==(ACTIVATION_COMMIT,ACTIVATION_BLOB,ACTIVATION_SHA) and bool(re.fullmatch(r"[0-9a-f]{40}",revision)),"independently reviewed public stage squash/activation");public_commit(repo,revision);public_commit(repo,commit);path=f"benchmarks/supplemental/practical_workloads/activations/{revision}.json";spec=commit+":"+path;data=git(repo,"show",spec);blob=git(repo,"rev-parse",spec).decode().strip();need(blob==expected_blob and raw.sha(data)==expected_sha,"public squash activation blob/digest");record=json.loads(data)
    need(record["schema"]=="imre.prospective.activation.v1" and record["stage_status"]=="uncredited" and record["strict_credit"]==0 and record["native_revision"]==revision and bool(re.fullmatch(r"[0-9a-f]{64}",record["artifact_identity_sha256"])),"uncredited external activation revision/identity")
    policy_commit=record["fixture_policy"]["commit"];public_commit(repo,policy_commit);need(policy_commit not in (commit,revision) and all(subprocess.run(["git","-C",str(repo),"merge-base","--is-ancestor",x,commit],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0 for x in (revision,policy_commit)),"activation must follow native and separately frozen policy commits")
    return record,policy_blob(repo,record["fixture_policy"])

def depfile_sources(data, output, cwd, crate):
    text=data.decode().replace("\\\n"," ");first=text.splitlines()[0];left,sep,right=first.partition(": ")
    need(sep and str(output) in shlex.split(left),"depfile does not name exact saved artifact")
    paths=shlex.split(right);checkout=Path(cwd);root=checkout/"crates"/crate;matching=[Path(posixpath.normpath(x)) for x in paths if f"/crates/{crate}/" in x]
    need(checkout.is_absolute() and matching and all(p.is_absolute() and p.is_relative_to(checkout) for p in matching) and any(p.is_relative_to(root) and p.suffix==".rs" for p in matching),"depfile does not bind exact source checkout/crate")
    return paths

def prebuild_provenance(build, log, compiler_dep, runtime_dep):
    cwd=build["cargo_build"]["cwd"]
    for crate in ("imre-compiler","imre-runtime"):
        need(re.search(rf"Compiling {crate}\b.*\({re.escape(cwd)}/crates/{crate}\)",log),"Cargo log lacks exact crate recompilation; use an isolated target after disk review")
    depfile_sources(compiler_dep,build["compiler"],cwd,"imre-compiler");depfile_sources(runtime_dep,build["runtime"],cwd,"imre-runtime")

def tracked_attributes(repo, revision, paths):
    # Ignore system/global attributes and fail closed if local Git metadata could override the tracked rules.
    info=Path(git(repo,"rev-parse","--path-format=absolute","--git-path","info/attributes").decode().strip())
    need(not info.exists(),"local info/attributes would override tracked attributes")
    env={**os.environ,"GIT_ATTR_NOSYSTEM":"1"}
    data=subprocess.check_output(["git","-C",str(repo),"-c","core.attributesFile=/dev/null","check-attr","--source="+revision,"-z",*CRLF_ATTRS,"--",*sorted(paths)],env=env).split(b"\0")
    return {(data[i].decode(),data[i+1].decode()):data[i+2].decode() for i in range(0,len(data)-1,3)}

def stage_relative(repo, build):
    manifest=Path(build["manifest_path"]);parts=manifest.parts;prefix=("benchmarks","supplemental","practical_workloads")
    matches=[i for i in range(len(parts)-2) if parts[i:i+3]==prefix]
    need(manifest.is_absolute() and str(manifest)==posixpath.normpath(str(manifest)) and len(matches)==1 and parts[-2:]==("evidence","build.json"),"confined recorded stage manifest path")
    relative=Path(*parts[matches[0]:-2]);need(len(relative.parts)==4 and relative.name.startswith("native_") and (repo/relative).is_relative_to(repo),"confined stage-relative path")
    return relative.as_posix()

def depfile_audit(evidence, build, ids, repo, revision, activation_commit, deps, log):
    need(revision==build["pin"],"depfile audit requires activated public source")
    stage=stage_relative(repo,build)
    for key,name in (("depfile_audit","depfile_audit.json"),("depfile_correction_proof","depfile_crlf_proof.json")):
        need(ids.get(build[key])==build[key+"_sha256"]==sha(evidence/name),"bound "+key)
    verifier=build["depfile_correction_verifier_sha256"]
    need(verifier==CORRECTION_VERIFIER_SHA==ids.get(build["depfile_correction_verifier"])==raw.sha(git(repo,"show",activation_commit+":"+stage+"/controls/check_depfile_crlf.py")),"reviewed public correction verifier")
    audit=load(evidence/"depfile_audit.json");proof=load(evidence/"depfile_crlf_proof.json");history=audit["audit_history"]
    need(audit["revision"]==proof["revision"]==revision and audit["tree"]==build["tree"] and audit["log_sha256"]==raw.sha(log.encode()) and history["correction_proof_sha256"]==build["depfile_correction_proof_sha256"] and history["correction_verifier_sha256"]==verifier and history["archived_controller_sha256"]==build["audit_controller_sha256"]==ids.get(build["audit_controller"]) and history["correction_timing"]=="retrospective post-capture derivation, not contemporaneous" and proof["derivation"]=="retrospective post-capture source-only verification; not a contemporaneous measurement","retrospective source audit custody")
    tree={}
    for row in git(repo,"ls-tree","-r","-z","--full-tree",revision).split(b"\0"):
        if row:
            meta,path=row.split(b"\t",1);mode,kind,oid=meta.decode().split();tree[path.decode()]=(mode,kind,oid)
    attrs=git(repo,"show",revision+":.gitattributes");rules=[CRLF_PREFIX+"common/Activate.ps1 text eol=crlf",CRLF_PREFIX+"nt/*.bat text eol=crlf"]
    need(proof["tracked_gitattributes"]=={"blob_oid":tree[".gitattributes"][2],"content_sha256":raw.sha(attrs),"rules":rules} and all((rule+"\n").encode() in attrs for rule in rules),"tracked d26 attributes blob/rules")
    observed=tracked_attributes(repo,revision,CRLF_PATHS);transforms=proof["transforms"]
    need(len(transforms)==3 and {x["path"] for x in transforms}==CRLF_PATHS,"exact three reviewed CRLF transforms")
    for row in transforms:
        path=row["path"];blob=git(repo,"show",revision+":"+path);checkout=blob.replace(b"\n",b"\r\n")
        need(tree[path][:2] in (("100644","blob"),("100755","blob")) and b"\r" not in blob and row["canonical_blob_oid"]==tree[path][2] and row["canonical_content_sha256"]==raw.sha(blob) and row["canonical_bytes"]==len(blob) and row["raw_sha256"]==raw.sha(checkout) and row["raw_bytes"]==len(checkout) and row["attributes"]==CRLF_ATTRS and all(observed.get((path,k))==v for k,v in CRLF_ATTRS.items()) and row["relation"]=="raw == canonical.replace(LF, CRLF); canonical == raw.replace(CRLF, LF); canonical contains no CR","deterministic tracked CRLF checkout "+path)
    generated=audit["generated"];need(len(generated)==7 and len({x["path"] for x in generated})==7,"exact seven generated outputs")
    target=Path(build["compiler"]).parents[1];generated_paths={}
    for row in generated:
        crate=row["crate"].removeprefix("imre-");path=Path(row["path"]);out=Path(row["out_dir"])
        need(crate in GENERATED and path.name in GENERATED[crate] and path==out/path.name and out.parent.parent==target/"release/build" and re.fullmatch("imre-"+crate+r"-[0-9a-f]+",out.parent.name) and out.name=="out" and ids.get(str(path))==row["sha256"] and bool(re.fullmatch(r"[0-9a-f]{64}",row["sha256"])) and any("OUT_DIR="+str(out) in line and "imre-"+crate in line for line in log.splitlines()) and row["provenance"]=="exact depfile member; OUT_DIR present in captured verbose Cargo build log; crate explicitly recompiled","generated output/provenance "+str(path))
        generated_paths[str(path)]=crate
    for key,data in zip(("compiler","runtime"),deps):
        paths=depfile_sources(data,build[key],build["cargo_build"]["cwd"],"imre-"+key);checkout=Path(build["cargo_build"]["cwd"]);tracked=[];gen=[];dotdot=0
        need(len(paths)==len(set(paths)) and all(Path(x).is_absolute() for x in paths),"unique absolute depfile inputs")
        for lexical in paths:
            path=Path(posixpath.normpath(lexical));dotdot += ".." in Path(lexical).parts
            if path.is_relative_to(checkout):
                rel=path.relative_to(checkout).as_posix();need(rel in tree and tree[rel][:2] in (("100644","blob"),("100755","blob")),"depfile input outside tracked d26 blobs: "+lexical);tracked.append(rel)
            else:
                need(lexical==str(path) and generated_paths.get(str(path))==key,"depfile input escaped source or unknown generated output: "+lexical);gen.append(str(path))
        transformed=set(tracked)&CRLF_PATHS;expected_raw=776 if key=="compiler" else 599;expected_transforms=3 if key=="compiler" else 0
        need(len(tracked)==len(set(tracked)) and len(tracked)-len(transformed)==expected_raw and len(transformed)==expected_transforms and set(Path(x).name for x in gen)==GENERATED[key] and len(gen)==len(GENERATED[key]),"exact tracked/raw/transformed/generated source inventory")
        need(proof["depfiles"][key]=={"raw_exact":expected_raw,"deterministic_attribute_transforms":expected_transforms,"generated_separately_bound":len(gen),"inputs":len(paths),"raw_depfile_sha256":raw.sha(data)} and audit["depfiles"][key]=={"generated":len(gen),"inputs":len(paths),"lexical_dotdot":dotdot,"raw_sha256":raw.sha(data),"tracked_canonical_git_blobs":len(tracked),"tracked_deterministic_attribute_transforms":expected_transforms,"tracked_raw_exact":expected_raw},"raw checkout proof and depfile inventory")

def public_capture_sources(repo, activation_commit, build):
    public_commit(repo,activation_commit)
    base="benchmarks/supplemental/practical_workloads/native_replay/"
    for key,path in (("producer","produce.py"),("validator","validate_native.py")):
        need(raw.sha(git(repo,"show",activation_commit+":"+base+path))==build[key+"_sha256"],"public activation squash as-used "+key+" source hash")

def capture_checkpoint(evidence, build, ids, record, repo):
    stage=stage_relative(repo,build)
    original=evidence/"capture_checkpoint_7b18276";custody=evidence/"capture_checkpoint_custody.json";base=Path(build["manifest_path"]).parent
    need(sha(custody)==record["retrospective_checkpoint_custody_sha256"]==ids.get(str(base/custody.name)),"reviewed retrospective checkpoint custody")
    obj=load(custody);names={"build.json","depfile_audit.json","artifacts.sha256","activation.json"}
    need(obj["original_checkpoint_commit"]=="7b18276f9c8d7446aebaf5a607bdb647763b429c" and obj["retrospective_proof_sha256"]==build["depfile_correction_proof_sha256"] and obj["retrospective_verifier_sha256"]==build["depfile_correction_verifier_sha256"] and len(obj["original_files"])==4 and {Path(x["archived_path"]).name for x in obj["original_files"]}==names,"checkpoint inventory and retrospective proof")
    for row in obj["original_files"]:
        name=Path(row["archived_path"]).name;data=(original/name).read_bytes();oid=subprocess.check_output(["git","hash-object","--stdin"],input=data,text=False).decode().strip()
        need(row["archived_path"]==stage+"/evidence/capture_checkpoint_7b18276/"+name and raw.sha(data)==row["sha256"]==ids.get(str(base/original.name/name)) and oid==row["original_blob_oid"],"archived original checkpoint file "+name)
    old=load(original/"activation.json");oldbuild=load(original/"build.json");oldids=raw.sha_lines((original/"artifacts.sha256").read_text())
    need(sha(original/"artifacts.sha256")==obj["original_artifact_identity_sha256"]==old["artifact_identity_sha256"] and sha(original/"activation.json")==obj["original_activation_sha256"] and oldbuild["pin"]==build["pin"] and oldbuild["tree"]==build["tree"] and oldids.get(oldbuild["manifest_path"])==sha(original/"build.json") and oldids.get(oldbuild["depfile_audit"])==sha(original/"depfile_audit.json"),"original checkpoint identity/activation/build/audit")

def artifact_ids(evidence, stock, revision, record, activation_commit):
    identity=evidence/"identity"; ids=raw.sha_lines((identity/"artifacts.sha256").read_text()); build=load(evidence/"build.json"); commands=load(identity/"commands.json")
    need(re.fullmatch(r"[0-9a-f]{40}",revision) and sha(identity/"artifacts.sha256")==record["artifact_identity_sha256"],"reviewed native revision/artifact identity")
    need(ids.get(build["manifest_path"])==sha(evidence/"build.json") and ids.get(commands["manifest_path"])==sha(identity/"commands.json"),"build/custody manifest identity")
    cargo=build["cargo_build"];cwd=cargo["cwd"];tree=subprocess.check_output(["git","-C",str(stock.parents[3]),"rev-parse",revision+"^{tree}"],text=True).strip()
    for key,args in (("head",["rev-parse","HEAD"]),("tree",["rev-parse","HEAD^{tree}"]),("status",["status","--porcelain","--untracked-files=all"])):
        row=commands[key]; need(row["argv"]==["git","-C",cwd,*args] and row["exit_code"]==0 and sha(identity/row["stdout"])==row["sha256"],"direct prebuild Git custody command")
    need(commands["prebuild_order"]==["head","tree","status","cargo_build"] and (identity/commands["head"]["stdout"]).read_text()==revision+"\n" and (identity/commands["tree"]["stdout"]).read_text()==tree+"\n" and (identity/commands["status"]["stdout"]).read_bytes()==b"" and build["pin"]==revision and build["tree"]==tree,"direct clean source immediately before build")
    target=str(Path(build["compiler"]).parents[1]);env=cargo["environment"]
    need(cargo["argv"]==[build["cargo"],"build","--offline","--release","--locked","-vv","-p","imre-compiler","-p","imre-runtime","--target-dir",target] and env=={"CARGO_HOME":"/workspace/.toolchains/cargo","RUSTUP_HOME":"/workspace/.toolchains/rustup","PATH":"/usr/bin:/bin","HOME":"/workspace","RUSTC":build["rustc"],"CC":build["cargo_cc_invocation"],"TMPDIR":build["tmpdir"]} and build["cargo_cc_invocation"]=="/usr/bin/cc" and cargo["exit_code"]==0 and cargo["log_sha256"]==sha(evidence.parent/"build.log"),"explicit incremental offline build environment/command")
    for key in ("cargo","rustc","clang"):
        row=commands[key+"_version"];need(row["argv"]==[build[key],"--version"] and row["exit_code"]==0 and sha(identity/row["stdout"])==row["sha256"] and re.search(rf"\b{key}\b",(identity/row["stdout"]).read_text(),re.I),"effective build-tool identity/version")
    ccrow=commands["cargo_cc_effective"];ccver=commands["cargo_cc_version"];need(ccrow["argv"]==["/usr/bin/readlink","-f",build["cargo_cc_invocation"]] and ccrow["exit_code"]==0 and sha(identity/ccrow["stdout"])==ccrow["sha256"] and (identity/ccrow["stdout"]).read_text()==build["cargo_cc"]+"\n" and ccver["argv"]==[build["cargo_cc_invocation"],"--version"] and ccver["exit_code"]==0 and sha(identity/ccver["stdout"])==ccver["sha256"] and re.search(r"\b(?:cc|gcc)\b",(identity/ccver["stdout"]).read_text(),re.I),"effective system Cargo C compiler identity/version")
    deps=[]
    for key in ("compiler","runtime"):
        row=build["depfiles"][key];data=(identity/(key+".d")).read_bytes();need(row["path"]==str(Path(build[key]).with_suffix(".d")) and row["sha256"]==raw.sha(data),"exact archived depfile") ;deps.append(data)
    log=(evidence.parent/"build.log").read_text();prebuild_provenance(build,log,*deps)
    depfile_audit(evidence,build,ids,stock.parents[3],revision,activation_commit,deps,log)
    capture_checkpoint(evidence,build,ids,record,stock.parents[3])
    need(sha(evidence/"controllers/audit_capture.py.txt")==build["audit_controller_sha256"],"as-used archived audit controller")
    pyrow=commands["python_identity"];identity_data=json.loads((identity/pyrow["stdout"]).read_bytes());need(pyrow["argv"]==[build["python_fixture"],"-I","-B","-c",PY_IDENTITY] and pyrow["exit_code"]==0 and sha(identity/pyrow["stdout"])==pyrow["sha256"] and identity_data["executable"]==build["python_fixture"] and identity_data["prefix"]==build["python_prefix"] and str(fixture_root(build["python_fixture"],identity_data["prefix"],"../lib/python3.14"))==build["stdlib_root"],"private effective interpreter identity")
    need(build["fixture_policy"]==record["fixture_policy"],"receipt fixture policy differs from activation")
    for key in ("probe","supervisor"):
        row=commands[key+"_build"];need(row["argv"]==["/usr/bin/cc","-O2","-Wall","-Wextra","-o",build[key],build[key+"_source"]] and row["exit_code"]==0 and sha(identity/row["stdout"])==row["sha256"],"existing control build")
    for key in ("compiler","runtime","cargo","rustc","clang","cargo_cc","python_fixture","supervisor","probe","producer","validator"):
        need(ids.get(build[key])==build[key+"_sha256"],key+" raw identity")
    public_capture_sources(stock.parents[3],activation_commit,build)
    need(sha(HERE/"supervisor.c")==build["supervisor_source_sha256"] and ids.get(build["supervisor_source"])==build["supervisor_source_sha256"] and ids.get(build["probe_source"])==sha(COMPAT/"revisions/8571ca2dbecabf8d7863c369dc776611fd57d8ad/controls/raw_control_probe.c"),"producer and existing supervisor/probe sources")
    need(build["python_fixture_sha256"]==load(stock/"references/ruff-publisher/reference.json")["tool"]["executable_sha256"] and sha(stock/"fixtures/cargo_stub.py")==build["stub_sha256"],"frozen fixture identity")
    return build,ids,build["python_fixture"],build["probe"],build["supervisor"]

def gate_launcher(original, frozen_sha256):
    need(raw.sha(original)==frozen_sha256,"gate positive source differs from frozen bytes")
    marker=b"from __future__ import annotations\n";injection=b"import architecture_control\n"
    try:module=ast.parse(original)
    except SyntaxError as error:raise ValueError("invalid frozen gate source") from error
    futures=[node for node in ast.walk(module) if isinstance(node,ast.ImportFrom) and node.module=="__future__"]
    docstring=ast.get_docstring(module,clean=False) is not None
    if marker in original:
        need(original.count(marker)==1 and len(futures)==1 and module.body[int(docstring)] is futures[0] and futures[0].col_offset==0 and [x.name for x in futures[0].names]==["annotations"] and original.splitlines(keepends=True)[futures[0].lineno-1]==marker,"duplicate or illegal gate future marker")
        return original.replace(marker,marker+injection,1)
    need(not futures and not docstring,"cannot prepend gate import before future import or docstring")
    return injection+original

def gate(o, receipt, source, build, probe):
    g = load(o/"gate.json")
    original = source.read_bytes()
    need((o/"gate.launcher.py.txt").read_bytes() == gate_launcher(original,receipt["source"]["sha256"]) and
         (o/"gate.module.py.txt").read_bytes() == b"status: int = True\n", "gate exact injection/poison")
    positive = receipt["build"]["argv"]
    expected = positive.copy(); expected[-1] = g["image"]
    need(g == receipt["gate"] and g["build_argv"] == expected and g["positive_sha256"] == sha(source) and
         g["injected_sha256"] == sha(o/"gate.launcher.py.txt") and g["module_sha256"] == sha(o/"gate.module.py.txt") and
         g["stdout_sha256"] == sha(o/"gate.stdout") and (o/"gate.stdout").read_bytes() == b"" and
         g["stderr_sha256"] == sha(o/"gate.stderr") and g["trace_sha256"] == sha(o/"gate.trace") and
         g["probe_exit_code"] == 0 and g["compiler_exit_code"] == 1 and g["image_absent_after"] is True, "gate binding")
    poison = str(Path(positive[positive.index("--module-root")+1])/"architecture_control.py")
    stderr = (o/"gate.stderr").read_text()
    need(stderr == f" Compiling {positive[2]} (-O0)\n{poison}:1:15: Imre: expected int, got bool; annotations are enforced contracts\n", "gate diagnostic/location")
    text = (o/"gate.trace").read_text()
    raw.trace_gate(text, expected, probe, poison, stderr, need)
    first, child = raw.execs(text.splitlines(), need)
    clone(text.splitlines(), first[1], child[1], first[0], child[0])
    need(g["argv"][:8] == ["/usr/bin/strace","-f","-qq","-s","512","-e","trace="+GATE_SYSCALLS,"-o"] and g["argv"][9:] == [probe,"gate",g["image"],*expected], "gate traced command/instrumentation")


def clone(lines, parent, child, start, end):
    pattern = rf"^{parent}\s+(?:clone|clone3|fork|vfork)\(.*\) = {child}$"
    i = raw.find(lines, pattern, start+1)
    need(i < end, "child clone must precede exec")
    return i


def descendant_execs(lines, publisher):
    if not publisher:
        return raw.execs(lines, need)
    calls = [(i,l) for i,l in enumerate(lines) if re.search(r"\bexecve(?:at)?\(",l)]
    need(len(calls) == 4, "publisher must execute supervisor, image, and exactly two fixture children")
    first_two = raw.execs(lines[:calls[2][0]], need)
    result = list(first_two)
    for i,line in calls[2:]:
        m = raw.EXEC.fullmatch(line)
        need(m is not None, "publisher extra exec failed/truncated/execveat")
        result.append((i,m[1],json.loads(m[2]),json.loads(m[3])))
    need(len({x[1] for x in result}) == 4, "duplicate publisher PID")
    return result


def open_policy(lines, cwd, execs, py, stub, reviewed, withdrawn_source=None):
    """Bound observed fixture directory enumeration and file opens.

    Symlink/unlink operations and TOCTOU are not traced here; this control does
    not establish adversarial filesystem immutability.
    """
    fixture = {x[1] for x in execs[2:]}
    stdlib = Path(py).parent.parent/"lib/python3.14"
    sources={stdlib/rel for rel in reviewed["stdlib"]}
    approved=sources|{Path(stub)}
    directories={stdlib,stdlib/"site-packages",stdlib/"lib-dynload"}|{parent for source in sources for parent in source.parents if parent.is_relative_to(stdlib)}
    absent_caches={source.parent/"__pycache__"/(source.stem+".cpython-314.pyc") for source in sources}
    violations = []
    for pid in {x[1] for x in execs}:
        rows = [l for l in lines if l.startswith(pid+" ")]
        directory_opens=Counter();resolved_directories=Counter()
        for line in rows:
            need(not re.search(r"\bio_uring_setup\(.*= [0-9]+$",line), "io_uring escape")
            if re.search(r"\bcreat\(",line) and re.search(r"= [0-9]+(?:<[^>]*>)?$",line):
                raise ValueError("successful creat is not covered by the shared resolver")
            m = raw.OPEN.match(line)
            if m:
                name = json.loads(m[1])
                read_only=bool(re.match(r'^, (?:O_RDONLY(?:\|O_CLOEXEC)?\)|\{flags=O_RDONLY(?:\|O_CLOEXEC)?(?:, mode=0)?(?:, resolve=0)?\}, \d+\)) = ',line[m.end(1):]))
                absolute=Path(posixpath.normpath(name)) if name.startswith("/") else None
                if absolute in directories:
                    directory_ok=(pid in fixture and name==str(absolute) and bool(re.match(r'^\d+\s+(?:open\(|openat\(AT_FDCWD, )',line)) and bool(re.fullmatch(r', O_RDONLY\|O_NONBLOCK\|O_CLOEXEC\|O_DIRECTORY\) = [0-9]+',line[m.end(1):])))
                    need(directory_ok,"fixture directory open differs from exact visibility control");directory_opens[absolute]+=1;continue
                sensitive = bool(re.search(r"\.(?:py|pyc|pyo|zip|whl|pyz)$",name,re.I) or "libpython" in name.lower() or absolute is not None and absolute.is_relative_to(stdlib) or absolute is None and name.lower().endswith(".so"))
                if sensitive:
                    need(name.startswith("/"), "unresolved relative Python/archive/dirfd path")
                    path=Path(posixpath.normpath(name)); allowed=pid in fixture and "libpython" not in name.lower() and path in approved
                    need(not allowed or read_only,"approved fixture source/stub opened with unapproved flags")
                    absent_source=pid==execs[1][1] and name==withdrawn_source and bool(re.search(r'= -1 ENOENT\b',line))
                    missing_cache=(pid in fixture and name==str(path) and path in absent_caches and
                        bool(re.match(r'^\d+\s+(?:open\(|openat(?:2)?\(AT_FDCWD, )',line)) and
                        read_only and
                        bool(re.search(r'\) = -1 ENOENT(?: \(No such file or directory\))?$',line)))
                    need(allowed or absent_source or missing_cache,"forbidden descendant Python/archive/libpython probe")
        # Resolve all successful opens per PID with the shared fd-aware resolver.
        for path in scorecard.successful_open_paths("\n".join(rows), cwd):
            name = path.name.lower()
            sensitive = bool(re.search(r"\.(?:py|pyc|pyo|zip|whl|pyz)$",name) or "libpython" in str(path).lower())
            if path in directories:resolved_directories[path]+=1
            if path.is_relative_to(stdlib): sensitive=True
            if not sensitive:
                continue
            allowed = pid in fixture and "libpython" not in str(path).lower() and (path in approved or path in directories)
            if not allowed:
                violations.append(f"{pid}: {path}")
        need(resolved_directories==directory_opens,"resolved directory opens differ from exact authorized raw events")
        if pid in fixture:
            exact = [l for l in rows if (m:=raw.OPEN.match(l)) and json.loads(m[1]) == stub and re.search(r"= [0-9]+$",l)]
            need(len(exact)==2, "fixture must open only its exact stub twice")
    need(not violations, "descendant opened forbidden Python/archive/libpython: "+str(violations))
    return violations


def withdrawal(o, r, py, supervisor, expected_code, publisher, archive, commands, reviewed):
    run = r["run"]; b = r["build"]; text=(o/"trace").read_text(); lines=text.splitlines()
    need(sha(o/"trace")==run["trace_sha256"],"withdrawal trace digest")
    need(run["argv"][:8]==["/usr/bin/strace","-f","-qq","-s","512","-e","trace="+RUN_SYSCALLS,"-o"],"withdrawal trace instrumentation")
    calls=descendant_execs(lines,publisher); first,target=calls[:2]
    roles=run["roles"]; source=str(Path(roles["$WORK"])/r["source"]["path"]); argv=run["argv"]; tail=argv[argv.index("-o")+2:]
    need(first[2:]==(supervisor,tail) and tail[0]==supervisor and tail[1]==source and tail[5]==b["argv"][-1] and
         target[2:]==(b["argv"][-1],run["workload_argv"]), "exact saved image/supervisor argv")
    stash=tail[2]
    pre=raw.stat(lines,first[1],source,False,first[0]+1)
    missing=raw.stat(lines,first[1],stash,True,pre+1)
    mv=raw.find(lines,rf'^{first[1]}\s+rename\({re.escape(json.dumps(source))}, {re.escape(json.dumps(stash))}\) = 0$',missing+1)
    absent=raw.stat(lines,first[1],source,True,mv+1); need(absent<target[0],"source ENOENT must precede image exec")
    clone(lines,first[1],target[1],absent,target[0])
    if archive:
        zip_path=archive; zipstash=tail[4]
        z=raw.find(lines,rf'^{first[1]}\s+rename\({re.escape(json.dumps(zip_path))}, {re.escape(json.dumps(zipstash))}\) = 0$',mv+1)
        ze=raw.stat(lines,first[1],zip_path,True,z+1);need(z<ze<target[0] and tail[3]==zip_path,"ZIP withdrawal/ENOENT order")
    waited=raw.exits(lines,first[1],target[1],expected_code,target[0])
    if archive:
        restored_zip=raw.find(lines,rf'^{first[1]}\s+rename\({re.escape(json.dumps(zipstash))}, {re.escape(json.dumps(zip_path))}\) = 0$',waited+1)
        waited=restored_zip
    restored=raw.find(lines,rf'^{first[1]}\s+rename\({re.escape(json.dumps(stash))}, {re.escape(json.dumps(source))}\) = 0$',waited+1)
    raw.find(lines,rf'^{first[1]}\s+exit_group\({expected_code}\)',restored+1)
    expected_parents={(first[1],target[1])}
    stub=roles["$CORPUS"]+"/fixtures/cargo_stub.py"
    if publisher:
        need(roles["$PYTHON"]==py, "fixture interpreter path")
        need(len(commands)==2,"frozen fixture command count")
        for call,command in zip(calls[2:],commands):
            need(call[2:]==(py,[py,stub,*command]),"fixture path/interpreter/argv")
            clone(lines,target[1],call[1],target[0],call[0])
            raw.exits(lines,target[1],call[1],0,call[0])
            expected_parents.add((target[1],call[1]))
    clones=[]
    for l in lines:
        if re.search(r"\b(?:clone|clone3|fork|vfork)\(",l):
            m=re.fullmatch(r"(\d+)\s+(?:clone|clone3|fork|vfork)\(.*\) = (\d+)",l)
            need(m is not None,"unresolved or failed child creation")
            clones.append((m[1],m[2]))
    need(len(clones)==len(expected_parents) and set(clones)==expected_parents,"full descendant closure")
    all_pids={x[1] for x in calls}
    need(all(not (m:=re.match(r"^(\d+)\s+",l)) or m[1] in all_pids for l in lines),"unaccounted descendant PID")
    open_policy(lines,roles["$WORK"],calls,py,stub,reviewed,source if archive else None)
    for name,args in (("readelf",["readelf","-d",b["argv"][-1]]),("ldd",["ldd",b["argv"][-1]])):
        link=r["linkage"][name];need(link["argv"]==args and link["exit_code"]==0 and not link["timed_out"] and link["stdout_sha256"]==sha(o/(name+".stdout")) and link["stderr_sha256"]==sha(o/(name+".stderr")),"saved image linkage evidence")
    need("libpython" not in (o/"readelf.stdout").read_text().lower() and "libpython" not in (o/"ldd.stdout").read_text().lower(),"native dynamic libpython")
    return expected_code


def tier(o, case, source, refdir, build, ids, py, supervisor, frozen, reviewed):
    r=load(o/"receipt.json"); ref=load(refdir/"reference.json"); b=r["build"];run=r["run"];roles=run["roles"]
    need(r["case"]==case and r["source"]==source and r["reference_sha256"]==sha(refdir/"reference.json") and load(o/"before.json")==load(refdir/"raw/before.json"),"frozen identity/preimage")
    t=r["tier"];need(t==o.name and t in ("O0","O3"),"exact native tier");entry=str(Path(roles["$WORK"])/source["path"]); a=b["argv"];clang=a[a.index("--clang")+1]; image=a[-1]
    expected=[build["compiler"],"build",entry,"--module-root",roles["$WORK"],"--no-cache","--opt-level",t[1],"--runtime",build["runtime"],"--clang",clang]
    archives=[x for x in ref["fixture"]["external_inputs"] if x["path"].lower().endswith((".zip",".whl",".pyz"))]
    need(len(archives)<=1,"unsupported frozen archive inventory")
    archive=archives[0]["path"].replace("$CORPUS",roles["$CORPUS"]) if archives else None
    if archive:
        need(ref["invocation"]["env"].get("PYTHONPATH")==archives[0]["path"] and archives[0]["path"].startswith("$CORPUS/fixtures/") and sha(refdir.parents[1]/archives[0]["path"].removeprefix("$CORPUS/"))==archives[0]["sha256"],"frozen source archive binding")
        expected += ["--extension-path",archive]
    expected+=["-o",image]
    need(a==expected and ids.get(clang)==build["clang_sha256"] and ids.get(image)==b["image_sha256"] and b["exit_code"]==0 and b["image_exists"] is True and
         b["stdout_sha256"]==sha(o/"build.stdout") and b["stderr_sha256"]==sha(o/"build.stderr") and (o/"build.stdout").read_bytes()==b"" and
         f"Compiling {entry} (-{t})" in (o/"build.stderr").read_text() and f"Finished {image} in " in (o/"build.stderr").read_text(),"positive build/image identity")
    need(run["cwd"]==roles["$WORK"] and frozen.normalize(run["environment"],roles)==ref["invocation"]["env"] and frozen.normalize(run["workload_argv"],roles)==ref["invocation"]["argv"][2:] and
         run["umask"]=="0077" and run["source_restored_sha256"]==source["sha256"] and run["bytecode_present_before"] is False,"frozen invocation/withdrawal identity")
    before=load(o/"before.json");after=load(o/"after.json");a0={x["path"]:x for x in before};a1={x["path"]:x for x in after}
    changes={"created":[x for k,x in a1.items() if k not in a0],"deleted":[x for k,x in a0.items() if k not in a1],"modified":[x for k,x in a1.items() if k in a0 and x!=a0[k]]}
    registry=[json.loads(x) for x in (o/"registry.jsonl").read_bytes().splitlines()];procs=[json.loads(x) for x in (o/"processes.jsonl").read_bytes().splitlines()]
    effects=frozen.normalized_effects({"files":changes,"registry":registry,"processes":procs},roles);expected_effects=load(refdir/"effects.normalized.json")
    need(effects==load(o/"effects.normalized.json"),"altered normalized effects")
    publisher=case in FIXTURE_CASES
    if publisher:
        need({x["path"]:x["sha256"] for x in ref["fixture"]["external_inputs"]}=={"$PYTHON":build["python_fixture_sha256"],"$CORPUS/fixtures/cargo_stub.py":build["stub_sha256"]},"frozen fixture exception scope")
        stub=roles["$CORPUS"]+"/fixtures/cargo_stub.py"; witness={py:build["python_fixture_sha256"],stub:build["stub_sha256"],entry:source["sha256"],image:b["image_sha256"]}
        stdlib=fixture_root(py,build["python_prefix"],reviewed["root_relative"]);witness.update({str(stdlib/rel):digest for rel,digest in reviewed["stdlib"].items()})
        need(reviewed["executable_sha256"]==build["python_fixture_sha256"] and reviewed["argv_tails"]==[p["argv"] for p in expected_effects["processes"]] and reviewed["stub_sha256"]==build["stub_sha256"] and all(ids.get(str(stdlib/rel))==digest for rel,digest in reviewed["stdlib"].items()) and ids.get(stub)==build["stub_sha256"] and raw.sha_lines((o/"fixture-before.sha256").read_text())==witness and raw.sha_lines((o/"fixture-after.sha256").read_text())==witness,"exact executed fixture path/bytes before and after")
        need(len(procs)==2,"fixture journal count")
    else:need(not procs,"unsanctioned process journal")
    if archive:need(run["zip_restored_sha256"]==archives[0]["sha256"],"archive restored by frozen hash")
    code=withdrawal(o,r,py,supervisor,run["exit_code"],publisher,archive,[p["argv"] for p in expected_effects["processes"]],reviewed)
    need(run["stdout_sha256"]==sha(o/"stdout") and run["stderr_sha256"]==sha(o/"stderr"),"raw native output binding")
    dimensions={"status":code==ref["observation"]["exit_code"] and not run["timed_out"],"stdout":(o/"stdout").read_bytes()==(refdir/"raw/stdout").read_bytes(),"stderr":(o/"stderr").read_bytes()==(refdir/"raw/stderr").read_bytes(),"mutations":effects["files"]==expected_effects["files"],"http_requests":effects["registry"]==expected_effects["registry"],"cargo_journal":effects["processes"]==expected_effects["processes"]}
    need(dimensions==run["comparison_dimensions"],"raw comparison dimensions rebound incorrectly")
    return dimensions


def validate(evidence, stock, revision, activation_commit, activation_blob, activation_sha):
    repo=stock.parents[3];need(subprocess.run(["git","-C",str(repo),"diff","--quiet",PIN,"--",str(stock.relative_to(repo))]).returncode==0,"immutable stock subtree changed")
    sys.path.insert(0,str(stock));import validate as frozen;frozen.main()
    sources=load(stock/"sources.json");need(set(p.name for p in evidence.iterdir() if p.is_dir() and p.name not in {"identity","controllers","capture_checkpoint_7b18276"})==set(sources),"frozen case inventory")
    record,reviewed=activation(repo,revision,activation_commit,activation_blob,activation_sha);build,ids,py,probe,supervisor=artifact_ids(evidence,stock,revision,record,activation_commit);result={}
    for case,source in sorted(sources.items()):
        try:
            refdir=stock/"references"/case; verdicts={t:tier(evidence/case/t,case,source,refdir,build,ids,py,supervisor,frozen,reviewed) for t in ("O0","O3")}
            failed=[t+"."+k for t,dims in verdicts.items() for k,v in dims.items() if not v]
            if failed: result[case]={"strict_pair":False,"status":"observed_mismatch","reason":", ".join(failed)}
            else:
                gate(evidence/case/"O0",load(evidence/case/"O0/receipt.json"),stock/source["vendored"],build,probe)
                result[case]={"strict_pair":True,"status":"strict_pass"}
        except (ValueError,KeyError,OSError) as error:
            result[case]={"strict_pair":False,"status":"blocked_control","reason":str(error)}
    return {"schema":"imre.prospective.strict.v1","stock_commit":PIN,"native_revision":revision,"activation":{"commit":activation_commit,"blob":activation_blob,"sha256":activation_sha,"artifact_identity_sha256":record["artifact_identity_sha256"]},"cases":result,"strict_pairs":sum(x["strict_pair"] for x in result.values()),"total_cases":len(result)}


def main():
    p=argparse.ArgumentParser();p.add_argument("--evidence",type=Path,required=True);p.add_argument("--stock",type=Path,default=HERE.parent/"stock_v1");p.add_argument("--revision",required=True);p.add_argument("--activation-commit",required=True);p.add_argument("--activation-blob",required=True);p.add_argument("--activation-sha256",required=True);p.add_argument("--score",type=Path);a=p.parse_args()
    result=validate(a.evidence.resolve(),a.stock.resolve(),a.revision,a.activation_commit,a.activation_blob,a.activation_sha256)
    if a.score:need(load(a.score)==result,"supplemental score differs from raw case derivation")
    print(json.dumps(result,indent=2,sort_keys=True))
if __name__=="__main__":main()
