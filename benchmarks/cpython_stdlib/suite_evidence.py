"""Version 1 of the per-occurrence CPython stdlib native-evidence contract."""
from collections import Counter
import json
import re


SCHEMA = "imre.cpython-stdlib.native-evidence/v1"
ROLES = ("body", "assertions", "operation", "fixture")
LOCATIONS = {"imre", "cpython", "mixed", "none", "unobserved", "unknown"}
OUTCOMES = {"pass", "failure", "error", "skip", "blocked", "excluded", "not-run",
            "expected-failure", "unexpected-success"}


def roles(*, body, assertions, operation, fixture):
    """Identify actual execution; 'none' means there is no such component."""
    result = dict(body=body, assertions=assertions, operation=operation, fixture=fixture)
    if not all(value in LOCATIONS for value in result.values()):
        raise ValueError(f"invalid execution location: {result}")
    return result


def occurrence_keys(ids):
    """Number repeated IDs independently, starting at one, in discovery order."""
    counts = Counter()
    for name in ids:
        counts[name] += 1
        yield name, counts[name]


def reference_cases(reference):
    if not re.fullmatch(r"[0-9a-f]{40}", reference["cpython_commit"]):
        raise ValueError("a reference requires the full CPython commit")
    if not all(_nonempty(reference.get(field)) for field in ("module", "platform")):
        raise ValueError("a reference requires a suite name and platform")
    result = {}
    for name, outcomes in reference["reference"].items():
        if not _nonempty(name):
            raise ValueError("empty upstream ID")
        if not isinstance(outcomes, list):
            outcomes = [outcomes]
        if not outcomes:
            raise ValueError(f"no reference occurrence for {name}")
        for i, outcome in enumerate(outcomes, 1):
            if outcome != "pass" and not (isinstance(outcome, dict) and len(outcome) == 1
                    and next(iter(outcome)) in {"failure", "error", "skipped"}
                    and isinstance(next(iter(outcome.values())), str)):
                raise ValueError(f"invalid reference outcome for {name} occurrence {i}: {outcome}")
            result[name, i] = outcome
    if not result or len(result) != reference["denominator"]:
        raise ValueError("reference denominator does not match its occurrences")
    return result


def _nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def classification(row):
    """Only the first category can count toward fully native compatibility."""
    outcome, ref, loc = row["outcome"], row["reference"], row["roles"]
    exception = row.get("exception", {})
    if outcome in {"failure", "error", "unexpected-success", "expected-failure"}:
        return "failed"
    if outcome == "blocked":
        return "blocked"
    if outcome == "excluded":
        return "reviewed-exclusion"
    if outcome == "skip":
        matched = exception.get("kind") == "upstream-skip" and ref == {"skipped": row["reason"]}
        return "reference-skip" if matched else "unexpected-skip"
    if outcome == "not-run":
        if isinstance(ref, dict):
            return "reference-skip" if "skipped" in ref else "reference-failure"
        return "reference-only"
    if ref != "pass":
        return "reference-disagreement"
    if loc["operation"] == "cpython":
        return "host-operation"
    if loc["operation"] == "none":
        return "no-product-operation"
    if "mixed" in (loc["body"], loc["assertions"], loc["operation"]):
        return "mixed"
    if loc["body"] == "imre" and loc["assertions"] in {"imre", "none"}:
        return "fully-native" if loc["fixture"] in {"imre", "none"} else "native-body-host-fixture"
    return "host-driven-native-operation"


def validate_row(row, reference, level, platform):
    key = row["id"], row["occurrence"]
    if type(key[1]) is not int or key[1] < 1 or type(row["opt_level"]) is not int or row["opt_level"] != level:
        raise ValueError(f"invalid occurrence or optimization level for {key}")
    if row["reference"] != reference:
        raise ValueError(f"reference outcome differs for {key}")
    if not _nonempty(row["fixture_variant"]):
        raise ValueError(f"missing fixture variant for {key}")
    loc = row["roles"]
    if set(loc) != set(ROLES) or any(value not in LOCATIONS for value in loc.values()):
        raise ValueError(f"invalid roles for {key}: {loc}")
    outcome = row["outcome"]
    if outcome not in OUTCOMES:
        raise ValueError(f"invalid outcome for {key}: {outcome}")
    proof = row.get("evidence", {})
    if "opt_level" in proof and (type(proof["opt_level"]) is not int or proof["opt_level"] != level):
        raise ValueError(f"native build level differs from recorded optimization level for {key}")
    if outcome == "pass":
        if any(value in {"unknown", "unobserved"} for value in loc.values()) or loc["body"] == "none":
            raise ValueError(f"passing observation must identify all execution locations for {key}")
        if not all(_nonempty(proof.get(field)) for field in ("operation", "fixture")):
            raise ValueError(f"passing observation needs operation and fixture evidence for {key}")
    if outcome in {"not-run", "excluded"} and any(value != "unobserved" for value in loc.values()):
        raise ValueError(f"unexecuted occurrence cannot claim observed roles for {key}")
    if outcome not in {"pass", "not-run"} and not _nonempty(row.get("reason")):
        raise ValueError(f"missing observed reason for {outcome}: {key}")
    exc = row.get("exception")
    if exc is not None:
        if not isinstance(exc, dict) or not all(_nonempty(exc.get(x)) for x in
                ("kind", "condition", "reason", "source", "configuration")):
            raise ValueError(f"incomplete per-test exception for {key}")
        if (exc.get("platform"), exc.get("fixture_variant")) != (platform, row["fixture_variant"]):
            raise ValueError(f"exception platform or fixture variant differs for {key}")
        if exc["kind"] == "upstream-skip":
            if reference != {"skipped": exc["reason"]}:
                raise ValueError(f"upstream skip reason differs for {key}")
        elif exc["kind"] == "reviewed-exclusion":
            if outcome != "excluded" or not all(_nonempty(exc.get(x)) for x in ("review", "revisit")):
                raise ValueError(f"exclusion needs its review and regression or issue for {key}")
        else:
            raise ValueError(f"invalid exception kind for {key}: {exc['kind']}")
    if outcome == "excluded" and (not exc or exc["kind"] != "reviewed-exclusion"):
        raise ValueError(f"unreviewed exclusion for {key}")
    if outcome == "excluded" and row["reason"] != exc["reason"]:
        raise ValueError(f"exclusion reason differs from its review for {key}")
    if row["classification"] == "reference-skip" and (not exc or exc["kind"] != "upstream-skip"):
        raise ValueError(f"missing per-test condition and source for reference skip {key}")
    if row["classification"] != classification(row):
        raise ValueError(f"classification disagrees with observed execution for {key}")


def build_report(reference, level, discovered, observations, *, exceptions=None):
    """Join explicit observations to the unchanged reference; never infer roles from labels.

    discovered contains (ID, fixture variant) in loader order. Observations use
    (ID, occurrence) keys. Exceptions use (ID, occurrence, optimization level).
    Missing observations remain in the denominator with all roles unobserved.
    """
    if type(level) is not int or level not in (0, 3):
        raise ValueError("native evidence requires O0 or O3")
    refs = reference_cases(reference)
    discovered = list(discovered)
    keys = list(occurrence_keys(name for name, _ in discovered))
    if set(keys) != refs.keys():
        raise ValueError("discovery and reference occurrences differ")
    exceptions = {} if exceptions is None else exceptions
    if set(observations) - set(keys) or set(exceptions) - {(*key, level) for key in keys}:
        raise ValueError("observation or exception names an undiscovered occurrence or wrong optimization level")
    rows = []
    for key, (_, variant) in zip(keys, discovered, strict=True):
        obs = observations.get(key, {"outcome": "not-run", "roles": dict.fromkeys(ROLES, "unobserved")})
        if set(obs) - {"outcome", "roles", "reason", "evidence"}:
            raise ValueError(f"unexpected observation fields for {key}: {obs.keys()}")
        row = {"id": key[0], "occurrence": key[1], "opt_level": level, "fixture_variant": variant,
               "reference": refs[key], **obs}
        if exc := exceptions.get((*key, level)):
            row["exception"] = exc
        row["classification"] = classification(row)
        validate_row(row, refs[key], level, reference["platform"])
        rows.append(row)
    return {"schema": SCHEMA, "suite": reference["module"], "cpython_commit": reference["cpython_commit"],
            "platform": reference["platform"], "opt_level": level, "denominator": len(rows),
            "distinct_ids": len(reference["reference"]),
            "counts": dict(sorted(Counter(row["classification"] for row in rows).items())),
            "records": rows}


def validate_report(report, reference):
    """Validate persisted evidence before a coordinator or another owner consumes it."""
    refs = reference_cases(reference)
    level = report["opt_level"]
    if (report["schema"] != SCHEMA or type(level) is not int or level not in (0, 3)
            or report["suite"] != reference["module"] or report["platform"] != reference["platform"]
            or report["cpython_commit"] != reference["cpython_commit"] or report["denominator"] != len(refs)
            or report["distinct_ids"] != len(reference["reference"])):
        raise ValueError("report header does not match this pinned reference")
    seen = set()
    for row in report["records"]:
        key = row["id"], row["occurrence"]
        if key in seen or key not in refs:
            raise ValueError(f"duplicate or unknown evidence key: {key}")
        seen.add(key)
        validate_row(row, refs[key], level, report["platform"])
    if seen != refs.keys() or report["counts"] != dict(Counter(r["classification"] for r in report["records"])):
        raise ValueError("coverage counts or occurrence denominator disagree with the evidence records")


def validate_pair(reports, reference):
    if sorted(report["opt_level"] for report in reports) != [0, 3]:
        raise ValueError("need exactly one full O0 and one full O3 report")
    for report in reports:
        validate_report(report, reference)


def write_reports(path, reports, reference):
    """Write ordinary JSON with one row per line for readable, focused evidence diffs."""
    validate_pair(reports, reference)
    chunks = []
    for report in reports:
        header = {key: value for key, value in report.items() if key != "records"}
        prefix = json.dumps(header, indent=2).removesuffix("\n}")
        records = ",\n".join("    " + json.dumps(row) for row in report["records"])
        chunks.append(f'{prefix},\n  "records": [\n{records}\n  ]\n}}')
    path.write_text("[\n" + ",\n".join(chunks) + "\n]\n")


if __name__ == "__main__":
    import argparse
    from pathlib import Path
    parser = argparse.ArgumentParser(description="Validate both native-evidence runs against the pinned reference.")
    parser.add_argument("reference", type=Path)
    parser.add_argument("reports", type=Path)
    args = parser.parse_args()
    reports = json.loads(args.reports.read_text())
    validate_pair(reports, json.loads(args.reference.read_text()))
    for report in reports:
        print(f"{report['suite']} -O{report['opt_level']}: {report['denominator']} occurrences, {report['counts']}")
