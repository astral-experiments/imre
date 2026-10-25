"""Join complete-method attempts and host-driven probes without dropping an occurrence."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, occurrence_keys, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests


def observe(name, level, body, body_rev, probes):
    state = body["status"]
    proof = {"opt_level": level, "complete_method": {**body, "compiler_commit": body_rev}}
    loc = roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="unobserved")
    if state == "native_body_host_fixture":
        if name.endswith(".test_with_bound_and_unbound_methods"):
            operation = "Native functools.partial constructs and calls both bound and unbound str.join."
        elif name.endswith(".TestSingleDispatch.test_simple_overloads"):
            operation = "Imre creates the dispatcher, registers int, and selects native callbacks for str, int, and list."
        elif name.endswith(".test_get_annotations"):
            operation = ("Imre creates an lru_cache wrapper, reads the original and wrapper annotations, "
                         "and compares both dictionaries with the expected builtin class objects.")
        elif name.endswith(".test_lru_cache_parameters"):
            operation = ("Imre executes both local lru_cache decorators and reads a fresh parameter "
                         "dictionary from each wrapper. The native assertion fixture compares both "
                         "dictionaries with the original expected values.")
        else:
            operation = f"Imre executed the product operations of the complete unchanged {name} method."
        proof.update(operation=operation,
                     fixture=body["fixture_execution"])
        return dict(outcome="pass", roles=roles(body="imre", assertions="imre", operation="imre", fixture="mixed"), evidence=proof)
    if state == "failed":
        return dict(outcome="failure", reason=body["diagnostic"], roles=loc, evidence=proof)
    if state != "blocked":
        raise ValueError(f"unexpected complete-method result: {body}")

    reason = f"{body['stage']}: {body['diagnostic']}"
    for api, rev, records in probes:
        if name not in records:
            continue
        row = records[name]
        state = row["status"]
        counts = {k: v for k, v in row.items() if type(v) is int}
        proof["product_probe"] = {"api": api, "compiler_commit": rev, **counts}
        if bindings := row.get("binding_observations"):
            proof["product_probe"]["binding_observations"] = bindings
        if state == "blocked":
            proof["product_probe"]["diagnostic"] = row["diagnostic"]
            if counts:
                observed_native = any(counts.get(field) for field in ("native_calls", "lookups", "snapshots", "clears", "parameter_reads"))
                observed_host = any(v for k, v in counts.items() if k.startswith("host_"))
                loc = roles(body="cpython", fixture="cpython", assertions="unknown",
                            operation="mixed" if observed_native and observed_host else
                            "imre" if observed_native else "cpython" if observed_host else "unobserved")
            return dict(outcome="blocked", reason=reason, roles=loc, evidence=proof)
        if api == "reduce":
            native = counts.get("native_calls", 0)
            host = counts.get("host_binding", 0)
            detail = f"Imre evaluated {native} folds; {host} calls reached the host argument binder."
            if warnings := counts.get("host_warnings"):
                detail += f" The host emitted {warnings} deprecation warnings for the pure variant."
        else:
            native = sum(counts.get(field, 0) for field in ("lookups", "clears", "snapshots", "parameter_reads"))
            host = sum(v for k, v in counts.items() if k.startswith("host_"))
            detail = ("Imre retained each cache and decided hits, misses, insertion, eviction, counters, "
                      "and clears. CPython invoked the user callback on a miss. Host observations: "
                      + repr({k: v for k, v in counts.items() if k.startswith("host_")}))
            if counts.get("parameter_reads"):
                detail = (f"Imre constructed {counts['parameter_reads']} fresh cache_parameters dictionaries. "
                          "CPython decoded the returned values and ran the original dictionary assertions.")
            if fields := counts.get("native_named_fields"):
                detail += f" Imre read {fields} named fields on the original retained cache snapshots."
            if expected := counts.get("native_expected_constructors"):
                detail += f" Imre constructed {expected} expected _CacheInfo tuples; CPython compared the transported values."
            if bindings:
                detail = ("CPython saved, replaced, looked up and restored its live builtins.len binding. "
                          "Imre chose hits, misses and evictions; the miss callback invoked the original host len. "
                          "The fixture read the native cache counters separately after the unchanged method returned.")
        proof.update(operation=detail, fixture="Original CPython module/class/instance hooks, test method and assertion helpers.")
        loc = roles(body="cpython", assertions="cpython", fixture="cpython",
                    operation="mixed" if native and host else "imre" if native else "cpython")
        if state == "failed":
            return dict(outcome="error", reason=row["diagnostic"], roles=loc, evidence=proof)
        if state not in {"mixed", "host_driven_native_operation"} or not native:
            raise ValueError(f"unaccounted operation: {name}: {row}")
        if (state == "mixed") != bool(host):
            raise ValueError(f"inconsistent host operation accounting: {name}: {row}")
        return dict(outcome="pass", roles=loc, evidence=proof)
    return dict(outcome="blocked", reason=reason, roles=loc, evidence=proof)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("bodies", "cache", "reduce", "result"):
        p.add_argument("--" + arg, type=Path, required=True)
    for arg in ("cache-commit", "reduce-commit"):
        p.add_argument("--" + arg, required=True)
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads((HERE / "reference-linux.json").read_text())
    bodies, cache, reduce = [json.loads(f.read_text()) for f in (args.bodies, args.cache, args.reduce)]
    for source in (bodies, cache, reduce):
        if (source["cpython_commit"], source["denominator"]) != (manifest["commit"], reference["denominator"]):
            raise ValueError("input uses a different suite or pin")
    discovered = [(case.id(), type(case).__name__) for case in cases(load_suite(manifest["module"]))]
    keys = list(occurrence_keys(n for n, _ in discovered))
    # These module-specific operation drivers store distinct IDs. Refuse to
    # attribute their observations to repeated instances without fresh evidence.
    if any(occurrence != 1 for _, occurrence in keys):
        raise ValueError("this suite has new repeated instances; rerun the operation drivers per occurrence")
    reports = []
    for level in (0, 3):
        rows = bodies["optimization_levels"][str(level)]
        if set(rows) != {name for name, _ in keys}:
            raise ValueError("incomplete whole-method attempts")
        probes = [(api, rev, report["optimization_levels"][str(level)]) for api, rev, report
                  in (("cache", args.cache_commit, cache), ("reduce", args.reduce_commit, reduce))]
        obs = {key: observe(key[0], level, rows[key[0]], bodies["compiler_commit"], probes) for key in keys}
        reports.append(build_report(reference, level, discovered, obs))
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
