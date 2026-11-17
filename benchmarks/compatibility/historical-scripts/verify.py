"""Offline join of the archived reported survey to the canonical 84 script IDs."""
from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import re
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
COMPAT = HERE.parent
ARCHIVE_SHA = "fd6b397128a744694bf7a6c0a942bef6245bd3cb164459977a8a8699468a4a18"
SHARDS = {
    "uv-a": "da2bf2d0e35dc9574ce484dc6b26cb30a323f1a458de2088e4639d349a9a7348",
    "uv-b": "9fdde84b72d96312495a02547fa1ecc65d1059be21c06d25fdcf8325509db01d",
    "ruff-a": "7ee9e92dfb43116477fdb235bde114b5cb9e0f24eee5f853f3c8e05f37d567cb",
    "ruff-b": "eac09c76f8d16af9fd54bea7bd97f38f45dea837a4cc1cc7d375262a2f4be2fb",
    "ty": "920578c3c61ec1cbe44fdb543a6558d0914d303d4baaf2dc1e589648db4b08c6",
}
FROZEN_SHA = "b558dfc9780b41ad00ed7911c4b07db8f9e05ca2266b4a9ece957176a8132823"
CHECKOUT = {"uv": "24688aaca1e7fd104437a0909d5db3afdc7dcdb0",
            "ruff": "65aee8892ce379af4ae9c8b5f6e0ab37b25d5ec9",
            "ty": "9b48f40c9f4fbea54387264318b8c0c6f0d6d184"}
TY_COMMITS = dict(zip(("ty-01","ty-02","ty-03","ty-04","ty-05","ty-06"), (
    "d4e4bf90906c7677049bc3a29d1735de7d22052d","7a36fc91318b07a58d0aa27753db4db1d212032a",
    "0445d6497ae22a0c480b23807cd71f47889053d8","1e7a76d73c2e9583f817f2bb219e5665e0351e67",
    "d4e4bf90906c7677049bc3a29d1735de7d22052d","d4e4bf90906c7677049bc3a29d1735de7d22052d")))
NONZERO = frozenset("uv-13 uv-27 ruff-03 ruff-08 ruff-11 ruff-12 ruff-14 ruff-19 prepare pytest ty-02 ty-06".split())
NEW_PASS = frozenset(("uv-27","ruff-12","ty-06"))
UNRESOLVED = {"uv-24": (4,"$VENV/bin/uv","uv-0.12.19"),
              "uv-extra-08": (0,"$WORK/entry-bin/black","black"),
              "uv-extra-09": (0,"$WORK/entry-bin/say-hi","say-hi"),
              "uv-extra-10": (0,"$WORK/entry-bin/flit-editable","flit-editable")}
ROLE = re.compile(r"\$[A-Z][A-Z0-9_]*")


def require(condition, message):
    if not condition: raise ValueError(message)


def checked(path, digest):
    raw = path.read_bytes()
    require(hashlib.sha256(raw).hexdigest() == digest, f"source digest differs: {path.name}")
    return raw


def unique(rows):
    result = {}
    for row in rows:
        require(row["id"] not in result, "duplicate join ID")
        result[row["id"]] = row
    return result


def load():
    raw = checked(HERE/"archived-survey-7724.tsv", ARCHIVE_SHA)
    require(len(raw) == 240585, "archived byte count differs")
    archived = unique(csv.DictReader(io.StringIO(raw.decode()), delimiter="\t"))
    canonical = unique(json.loads(line) for name, digest in SHARDS.items()
                       for line in checked(COMPAT/"scripts"/(name+".jsonl"), digest).splitlines())
    frozen = unique(json.loads(line) for line in checked(COMPAT/"frozen/scripts.jsonl", FROZEN_SHA).splitlines())
    return archived, canonical, frozen


def role_value(role, value):
    if not value.startswith("/") or value == "/": return False
    suffix = {"$PRIVATE_STOCK": "/private-stock/bin/python3.14", "$FIXTURE_PYTHON": "/cpython/bin/python3.14",
              "$UV19": "/uv-0.12.19", "$UV18": "/uv-0.12.18", "$REQ": "/requirements.in",
              "$SOURCE": "/source", "$CACHED_MIRROR": "/cached-mirror", "$FROZEN_INPUT": "/inputs/source",
              "$PACKAGING": "/compatible-packaging", "$PROOT_LIB": "/proot/usr/lib/x86_64-linux-gnu", "$PROOT": "/proot/usr/bin/proot"}
    if role in suffix: return value.endswith(suffix[role])
    if role == "$PYTHON": return value.endswith("/bin/python3.14")
    return role == "$WORK"


def uv_mapping(id, original, normalized):
    require(len(original) == len(normalized), f"{id}: argv length differs")
    mappings = []; unresolved = False
    bindings = {}
    for index,(old,new) in enumerate(zip(original,normalized)):
        if old == new: continue
        special = UNRESOLVED.get(id)
        if special and index == special[0]:
            require(new == special[1] and Path(old).name == special[2]
                    and (id == "uv-24" or old.startswith("/tmp/uvdisc/") and "/bin/" in old),
                    f"{id}: unresolved mapping changed")
            mappings.append({"index":index,"unresolved_original":old,"canonical":new})
            unresolved = True; continue
        matches = list(ROLE.finditer(new))
        require(len(matches) == 1, f"{id}: unmapped argv token {index}")
        match = matches[0];role=match.group();prefix=new[:match.start()];suffix=new[match.end():]
        require(old.startswith(prefix) and old.endswith(suffix) and len(old) >= len(prefix)+len(suffix),
                f"{id}: argv structure differs at {index}")
        value=old[len(prefix):len(old)-len(suffix) if suffix else None]
        require(role_value(role,value) and (role not in bindings or bindings[role] == value),
                f"{id}: invalid role mapping {role}")
        bindings[role]=value;mappings.append({"index":index,"role":role,"archive_value":value,"canonical":new})
    require(unresolved == (id in UNRESOLVED), f"{id}: unresolved mapping missing")
    return ("unresolved" if unresolved else "structural_roles"),mappings


def derive(archived, canonical, frozen):
    require(len(canonical)==84 and len(archived)==85 and set(archived)-set(canonical)=={"ecosystem-console"}
            and set(canonical) <= set(archived), "84/85 join count or extra ID differs")
    require(len(frozen)==7 and set(frozen)<=set(canonical), "frozen/scripts anchor count differs")
    rows=[];historic=Counter();latest=Counter();old_nonpass=[];new_pass=[];nonzero=set()
    for id,c in canonical.items():
        a=archived[id];project=a["project"];src=c["source"]
        require(project in CHECKOUT and src["repo"] == f"https://github.com/astral-sh/{project}", f"{id}: source repo differs")
        require(a["entry"]==src["path"] and re.fullmatch(r"[0-9a-f]{64}",a["source_sha256"]) and a["source_sha256"]==src["sha256"],
                f"{id}: source path or SHA differs")
        require(a["git_revision"]==CHECKOUT[project] and src["commit"]==(TY_COMMITS[id] if project=="ty" else CHECKOUT[project]),
                f"{id}: checkout/source commit differs")
        original=json.loads(a["original_argv"]);require(isinstance(original,list) and all(isinstance(x,str) for x in original), f"{id}: invalid argv")
        if project=="uv": relation,mapping=uv_mapping(id,original,c["normalized_invocation"]["argv"])
        else:
            require(original==c.get("original_argv",c.get("historical_argv")), f"{id}: historical argv differs")
            relation,mapping="exact_historical_vector",[]
        require(re.fullmatch(r"[0-9]+",a["stock_O0"]) is not None, f"{id}: invalid reported stock exit")
        exit_code=int(a["stock_O0"]);old=exit_code==0;now=c["runs"]["stock"]["outcome"]
        require(now in {"pass","fail","error","skip","timeout","not_run","omitted"}, f"{id}: invalid canonical stock verdict")
        if not old:nonzero.add(id)
        if old:historic[project]+=1
        if now=="pass":latest[project]+=1
        if old and now!="pass":old_nonpass.append(id)
        if not old and now=="pass":new_pass.append(id)
        rows.append({"id":id,"project":project,"path":a["entry"],"source_sha256":a["source_sha256"],
                     "archived_checkout":a["git_revision"],"canonical_file_commit":src["commit"],
                     "commit_relation":"different_checkout_same_recorded_bytes" if project=="ty" else "exact",
                     "argv_relation":relation,"role_mapping":mapping,"archived_reported_stock_exit":exit_code,
                     "canonical_stock_outcome":now,"independent_frozen_entry":id in frozen})
    require(nonzero==NONZERO and historic==Counter(uv=41,ruff=27,ty=4), "archived stock verdict differs")
    require(sum(latest.values())==52 and len(old_nonpass)==23 and set(new_pass)==NEW_PASS, "canonical stock transition differs")
    require(Counter(r["argv_relation"] for r in rows)==Counter(exact_historical_vector=41,structural_roles=39,unresolved=4),
            "argv relation counts differ")
    return rows


def encoded(rows):
    return "".join(json.dumps(row,sort_keys=True,separators=(",",":"))+"\n" for row in rows)


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--check",action="store_true");args=parser.parse_args()
    rows=derive(*load());output=HERE/"join.jsonl";text=encoded(rows)
    if args.check:require(output.read_text()==text,"derived join differs")
    else:output.write_text(text)
    print("84 joined; reported historical stock 72; canonical stock 52; seven independent frozen entries; zero credit")


if __name__=="__main__":main()
