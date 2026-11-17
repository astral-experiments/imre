"""Derive prospective case-level strict pairs from immutable stock and raw native evidence."""
from __future__ import annotations
import argparse
import hashlib
import json
import posixpath
import re
import shlex
import subprocess
import sys
from pathlib import Path

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
    need(bool(re.fullmatch(r"[0-9a-f]{40}",revision)),"exact native revision");public_commit(repo,revision);public_commit(repo,commit);path=f"benchmarks/supplemental/practical_workloads/activations/{revision}.json";spec=commit+":"+path;data=git(repo,"show",spec);blob=git(repo,"rev-parse",spec).decode().strip();need(blob==expected_blob and raw.sha(data)==expected_sha,"public squash activation blob/digest");record=json.loads(data)
    need(record["native_revision"]==revision and bool(re.fullmatch(r"[0-9a-f]{64}",record["artifact_identity_sha256"])),"external activation revision/identity")
    policy_commit=record["fixture_policy"]["commit"];public_commit(repo,policy_commit);need(policy_commit not in (commit,revision) and all(subprocess.run(["git","-C",str(repo),"merge-base","--is-ancestor",x,commit],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0 for x in (revision,policy_commit)),"activation must follow native and separately frozen policy commits")
    return record,policy_blob(repo,record["fixture_policy"])

def depfile_sources(data, output, cwd, crate):
    text=data.decode().replace("\\\n"," ");first=text.splitlines()[0];left,sep,right=first.partition(": ")
    need(sep and str(output) in shlex.split(left),"depfile does not name exact saved artifact")
    paths=shlex.split(right);root=Path(cwd)/"crates"/crate;matching=[Path(x) for x in paths if f"/crates/{crate}/" in x]
    need(matching and all(p.is_absolute() and p.is_relative_to(root) and ".." not in p.parts for p in matching) and any(p.suffix==".rs" for p in matching),"depfile does not bind exact source checkout/crate")

def prebuild_provenance(build, log, compiler_dep, runtime_dep):
    cwd=build["cargo_build"]["cwd"]
    for crate in ("imre-compiler","imre-runtime"):
        need(re.search(rf"Compiling {crate}\b.*\({re.escape(cwd)}/crates/{crate}\)",log),"Cargo log lacks exact crate recompilation; use an isolated target after disk review")
    depfile_sources(compiler_dep,build["compiler"],cwd,"imre-compiler");depfile_sources(runtime_dep,build["runtime"],cwd,"imre-runtime")

def artifact_ids(evidence, stock, revision, record):
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
    prebuild_provenance(build,(evidence.parent/"build.log").read_text(),*deps)
    pyrow=commands["python_identity"];identity_data=json.loads((identity/pyrow["stdout"]).read_bytes());need(pyrow["argv"]==[build["python_fixture"],"-I","-c",PY_IDENTITY] and pyrow["exit_code"]==0 and sha(identity/pyrow["stdout"])==pyrow["sha256"] and identity_data["executable"]==build["python_fixture"] and identity_data["prefix"]==build["python_prefix"] and str(fixture_root(build["python_fixture"],identity_data["prefix"],"../lib/python3.14"))==build["stdlib_root"],"private effective interpreter identity")
    need(build["fixture_policy"]==record["fixture_policy"],"receipt fixture policy differs from activation")
    for key in ("probe","supervisor"):
        row=commands[key+"_build"];need(row["argv"]==["/usr/bin/cc","-O2","-Wall","-Wextra","-o",build[key],build[key+"_source"]] and row["exit_code"]==0 and sha(identity/row["stdout"])==row["sha256"],"existing control build")
    for key in ("compiler","runtime","cargo","rustc","clang","cargo_cc","python_fixture","supervisor","probe","producer","validator"):
        need(ids.get(build[key])==build[key+"_sha256"],key+" raw identity")
    need(sha(HERE/"validate_native.py")==build["validator_sha256"] and sha(HERE/"produce.py")==build["producer_sha256"] and sha(HERE/"supervisor.c")==build["supervisor_source_sha256"] and ids.get(build["supervisor_source"])==build["supervisor_source_sha256"] and ids.get(build["probe_source"])==sha(COMPAT/"revisions/8571ca2dbecabf8d7863c369dc776611fd57d8ad/controls/raw_control_probe.c"),"producer and existing supervisor/probe sources")
    need(build["python_fixture_sha256"]==load(stock/"references/ruff-publisher/reference.json")["tool"]["executable_sha256"] and sha(stock/"fixtures/cargo_stub.py")==build["stub_sha256"],"frozen fixture identity")
    return build,ids,build["python_fixture"],build["probe"],build["supervisor"]

def gate(o, receipt, source, build, probe):
    g = load(o/"gate.json")
    original = source.read_bytes()
    marker = b"from __future__ import annotations\n"
    need(original.count(marker) == 1 and (o/"gate.launcher.py.txt").read_bytes() == original.replace(marker,marker+b"import architecture_control\n",1) and
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
    fixture = {x[1] for x in execs[2:]}
    stdlib = Path(py).parent.parent/"lib/python3.14"
    approved={stdlib/rel for rel in reviewed["stdlib"]}|{Path(stub)}
    violations = []
    for pid in {x[1] for x in execs}:
        rows = [l for l in lines if l.startswith(pid+" ")]
        for line in rows:
            need(not re.search(r"\bio_uring_setup\(.*= [0-9]+$",line), "io_uring escape")
            if re.search(r"\bcreat\(",line) and re.search(r"= [0-9]+(?:<[^>]*>)?$",line):
                raise ValueError("successful creat is not covered by the shared resolver")
            m = raw.OPEN.match(line)
            if m:
                name = json.loads(m[1])
                sensitive = bool(re.search(r"\.(?:py|pyc|pyo|zip|whl|pyz)$",name,re.I) or "libpython" in name.lower())
                if sensitive:
                    need(name.startswith("/"), "unresolved relative Python/archive/dirfd path")
                    path=Path(posixpath.normpath(name)); allowed=pid in fixture and "libpython" not in name.lower() and path in approved
                    absent_source=pid==execs[1][1] and name==withdrawn_source and bool(re.search(r'= -1 ENOENT\b',line))
                    need(allowed or absent_source,"forbidden descendant Python/archive/libpython probe")
        # Resolve all successful opens per PID with the shared fd-aware resolver.
        for path in scorecard.successful_open_paths("\n".join(rows), cwd):
            name = path.name.lower()
            sensitive = bool(re.search(r"\.(?:py|pyc|pyo|zip|whl|pyz)$",name) or "libpython" in str(path).lower())
            if path.is_relative_to(stdlib): sensitive=True
            if not sensitive:
                continue
            allowed = pid in fixture and "libpython" not in str(path).lower() and path in approved
            if not allowed:
                violations.append(f"{pid}: {path}")
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
    sources=load(stock/"sources.json");need(set(p.name for p in evidence.iterdir() if p.is_dir() and p.name!="identity")==set(sources),"frozen case inventory")
    record,reviewed=activation(repo,revision,activation_commit,activation_blob,activation_sha);build,ids,py,probe,supervisor=artifact_ids(evidence,stock,revision,record);result={}
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
    return {"schema":"imre.prospective.strict.v1","stock_commit":PIN,"native_revision":revision,"cases":result,"strict_pairs":sum(x["strict_pair"] for x in result.values()),"total_cases":len(result)}


def main():
    p=argparse.ArgumentParser();p.add_argument("--evidence",type=Path,required=True);p.add_argument("--stock",type=Path,default=HERE.parent/"stock_v1");p.add_argument("--revision",required=True);p.add_argument("--activation-commit",required=True);p.add_argument("--activation-blob",required=True);p.add_argument("--activation-sha256",required=True);p.add_argument("--score",type=Path);a=p.parse_args()
    result=validate(a.evidence.resolve(),a.stock.resolve(),a.revision,a.activation_commit,a.activation_blob,a.activation_sha256)
    if a.score:need(load(a.score)==result,"supplemental score differs from raw case derivation")
    print(json.dumps(result,indent=2,sort_keys=True))
if __name__=="__main__":main()
