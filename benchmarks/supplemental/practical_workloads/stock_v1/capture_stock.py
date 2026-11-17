"""Freeze fresh stock-only CPython observations; refuse to overwrite a reference."""
from __future__ import annotations
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import sysconfig
import tempfile
import threading

ROOT = Path(__file__).resolve().parent
SOURCES = json.loads((ROOT / "sources.json").read_text())

def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()

def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")

def inventory(root: Path) -> list[dict]:
    result = []
    for p in sorted(root.rglob("*")):
        item = {"path": p.relative_to(root).as_posix(), "mode": oct(p.stat().st_mode & 0o777)}
        if p.is_dir(): item["kind"] = "directory"
        elif p.is_file():
            data = p.read_bytes(); item.update(kind="file", bytes=len(data), sha256=digest(data))
        else: raise ValueError(f"unsupported fixture entry: {p}")
        result.append(item)
    return result

def normalize(value, roles):
    if isinstance(value, str):
        for role, actual in sorted(roles.items(), key=lambda kv: -len(kv[1])):
            value = value.replace(actual, role)
        return value
    if isinstance(value, list): return [normalize(x, roles) for x in value]
    if isinstance(value, dict): return {k: normalize(v, roles) for k, v in value.items()}
    return value

def normalized_effects(effects, roles):
    result = normalize(effects, roles)
    journal = "".join(json.dumps(p, sort_keys=True) + "\n" for p in result["processes"]).encode()
    for item in result["files"]["created"]:
        if item["path"] == "cargo.log": item.update(bytes=len(journal), sha256=digest(journal))
    return result

def file_fact(path: Path, role: str) -> dict:
    data=path.read_bytes(); return {"path": role, "bytes": len(data), "sha256": digest(data)}

class Registry(http.server.BaseHTTPRequestHandler):
    events: list[dict] = []
    def do_GET(self):
        status = 200 if self.path == "/api/v1/crates/already/1.0" else 404
        body = b"{}"; headers = [["Content-Length", "2"]]
        self.events.append({"request_line": self.requestline, "method": "GET", "target": self.path,
                            "headers": list(map(list, self.headers.raw_items())), "body_hex": "",
                            "response": {"status": status, "headers": headers, "body_hex": body.hex()}})
        self.send_response_only(status)
        for key, value in headers: self.send_header(key, value)
        self.end_headers(); self.wfile.write(body)
    def log_message(self, *args): pass

def freeze(case: str) -> None:
    source=SOURCES[case]; out=ROOT/"references"/case; out.mkdir(parents=True, exist_ok=False); raw=out/"raw"; raw.mkdir()
    with tempfile.TemporaryDirectory(prefix="stock-reference-", dir="/workspace/scratch") as temp:
        work=Path(temp); (work/"scripts").mkdir(mode=0o755); (work/"home").mkdir(mode=0o755)
        entry=work/source["path"]; shutil.copyfile(ROOT/source["vendored"],entry); entry.chmod(0o644)
        project_inputs=json.loads((ROOT/"ty-inputs.json").read_text()) if case=="ty-readme" else []
        for item in project_inputs:
            target=work/item["path"]; shutil.copyfile(ROOT/item["vendored"],target); target.chmod(0o644)
        roles={"$WORK": str(work), "$CORPUS": str(ROOT), "$PYTHON": sys.executable}
        env={"HOME":str(work/"home"),"LANG":"C.UTF-8","LC_ALL":"C.UTF-8","PATH":"/usr/bin",
             "PYTHONDONTWRITEBYTECODE":"1","PYTHONHASHSEED":"0","PYTHONNOUSERSITE":"1","TZ":"UTC"}
        argv=[sys.executable,"-B",source["path"]]
        external=[file_fact(Path(sys.executable),"$PYTHON")]
        server=None; thread=None; Registry.events=[]
        if case.endswith("publisher"):
            server=http.server.ThreadingHTTPServer(("127.0.0.1",0),Registry)
            thread=threading.Thread(target=server.serve_forever,daemon=True); thread.start()
            authority=f"127.0.0.1:{server.server_port}"; origin="http://"+authority
            roles.update({"$REGISTRY_ORIGIN":origin,"$REGISTRY_AUTHORITY":authority})
            env.update({"CARGO_LOG":str(work/"cargo.log"),"NO_PROXY":"127.0.0.1"})
            stub=ROOT/"fixtures/cargo_stub.py"; external.append(file_fact(stub,"$CORPUS/fixtures/cargo_stub.py"))
            argv += ["--api-url",origin+"/api/v1","--cargo",sys.executable,str(stub),"--no-verify"]
        elif case=="uv-rich":
            bundle=ROOT/"fixtures/rich-imports.zip"; external.append(file_fact(bundle,"$CORPUS/fixtures/rich-imports.zip"))
            env.update({"PYTHONPATH":str(bundle),"TERM":"dumb","NO_COLOR":"1","COLUMNS":"80"}); argv.append("Ruff")
        before=inventory(work)
        try: result=subprocess.run(argv,cwd=work,env=env,capture_output=True,timeout=30)
        finally:
            if server: server.shutdown(); server.server_close(); thread.join()
        after=inventory(work)
        (raw/"stdout").write_bytes(result.stdout); (raw/"stderr").write_bytes(result.stderr)
        write_json(raw/"wait.json",{"exit_code":result.returncode,"timed_out":False})
        write_json(raw/"before.json",before); write_json(raw/"after.json",after)
        registry="".join(json.dumps(x,sort_keys=True)+"\n" for x in Registry.events).encode(); (raw/"registry.jsonl").write_bytes(registry)
        processes=(work/"cargo.log").read_bytes() if (work/"cargo.log").exists() else b""; (raw/"processes.jsonl").write_bytes(processes)
        if (work/"published.txt").exists(): shutil.copyfile(work/"published.txt",raw/"published.txt")
        if case=="ty-readme": shutil.copyfile(work/"README.md",raw/"README.md")
        original={x["path"]:x for x in before}; final={x["path"]:x for x in after}
        changes={"created":[x for p,x in final.items() if p not in original],"deleted":[x for p,x in original.items() if p not in final],"modified":[x for p,x in final.items() if p in original and x!=original[p]]}
        effects={"registry":Registry.events,"processes":[json.loads(x) for x in processes.splitlines()],"files":changes}
        write_json(out/"effects.normalized.json",normalized_effects(effects,roles))
        invocation={"argv":argv,"cwd":str(work),"env":dict(sorted(env.items()))}; write_json(raw/"invocation.json",{"roles":roles,"invocation":invocation})
        evidence={}
        for p in sorted(raw.iterdir()): evidence["raw/"+p.name]=file_fact(p,"raw/"+p.name)
        evidence["effects.normalized.json"]=file_fact(out/"effects.normalized.json","effects.normalized.json")
        tool={"implementation":platform.python_implementation(),"version":platform.python_version(),"version_verbose":sys.version,
              "executable_sha256":digest(Path(sys.executable).read_bytes()),"soabi":sysconfig.get_config_var("SOABI"),
              "system":platform.system(),"machine":platform.machine(),"kernel":platform.release(),"libc":list(platform.libc_ver())}
        ref={"schema":"prospective-stock-reference-v1","case":case,"scope":"supplemental-stock-only","source":source,
             "invocation":normalize(invocation,roles),"tool":tool,"fixture":{"before":"raw/before.json","external_inputs":external},
             "observation":{"exit_code":result.returncode,"stdout":"raw/stdout","stderr":"raw/stderr","effects":"effects.normalized.json"},"evidence":evidence}
        if project_inputs: ref["fixture"]["project_inputs"]=project_inputs
        write_json(out/"reference.json",ref)
        print(case,result.returncode,len(result.stdout),len(result.stderr),len(Registry.events),flush=True)

def main():
    if sys.version_info[:3] != (3,14,5) or platform.python_implementation()!="CPython" or platform.system()!="Linux":
        raise SystemExit("capture requires CPython 3.14.5 on Linux")
    parser=argparse.ArgumentParser(); parser.add_argument("--case",choices=SOURCES,action="append"); args=parser.parse_args()
    for case in args.case or SOURCES: freeze(case)
if __name__=="__main__":main()
