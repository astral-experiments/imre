"""Generate Imre's _opcode metadata from the pinned CPython release.

The stack formulas come from pycore_opcode_metadata.h. CPython's
Python/flowgraph.c:get_stack_effects rejects specialized instructions,
checks both stack counts, and treats a non-jump block push separately.

    python3 scripts/generate_opcode_metadata.py /path/to/cpython
"""

import argparse
from collections import defaultdict
from itertools import groupby
import json
from pathlib import Path
import re
import subprocess

PIN = "5607950ef232dad16d75c0cf53101d9649d89115"
DEST = "crates/imre-runtime/src/dynamic/stdlib/opcode/metadata.rs"
FLAGS = ["arg", "const", "name", "jump", "free", "local"]


def definitions(source):
    return {name: int(n) for name, n in re.findall(r"^#define (\w+)\s+(\d+)$", source, re.M)}


def cases(header, direction):
    body = header.split(f"int _PyOpcode_num_{direction}(int opcode, int oparg)  {{", 1)[1]
    body = body.split("default:", 1)[0]
    entries = re.findall(r"case (\w+):\s+return (.+?);", body)
    assert len(entries) == body.count("case ")
    for _, expression in entries:
        if not re.fullmatch(r"[0-9xa-fA-F()+* &>\-]+", expression.replace("oparg", "0")):
            raise ValueError(f"Unhandled CPython stack expression: {expression}")
    return dict(entries)


def pattern(values):
    # Consecutive opcode numbers use an inclusive Rust range.
    groups = groupby(enumerate(sorted(values)), key=lambda pair: pair[1] - pair[0])
    parts = []
    for _, group in groups:
        run = [n for _, n in group]
        parts.append(str(run[0]) if len(run) == 1 else f"{run[0]}..={run[-1]}")
    return " | ".join(parts)


def generate(source):
    ids = definitions(source("Include/opcode_ids.h"))
    header = source("Include/internal/pycore_opcode_metadata.h")
    utils = source("Include/internal/pycore_opcode_utils.h")
    block_macro = utils.split("#define IS_BLOCK_PUSH_OPCODE(opcode)", 1)[1].split("\n\n", 1)[0]
    blocks = {ids[name] for name in re.findall(r"\(opcode\) == (\w+)", block_macro)}
    assert blocks

    start, body = re.search(r"_PyOpcode_opcode_metadata\[(\d+)\] = \{(.*?)\n\};", header, re.S).groups()
    flags = [0] * int(start)
    for line in body.strip().splitlines():
        name, expr = re.fullmatch(r"\s*\[(\w+)\] = \{ true, [\w-]+, ([\w |]+) \},", line).groups()
        bits = set(expr.split(" | "))
        flags[ids[name]] = 128 | (64 if ids[name] in blocks else 0)
        for i, flag in enumerate(FLAGS):
            if f"HAS_{flag.upper()}_FLAG" in bits:
                flags[ids[name]] |= 1 << i

    def number(token):
        return int(token) if token.isdecimal() else ids[token]

    deopt = header.split("const uint8_t _PyOpcode_Deopt[256] = {", 1)[1].split("};", 1)[0]
    deopt = {number(n): number(base) for n, base in re.findall(r"\[(\w+)\] = (\w+),", deopt)}
    assert len(deopt) == 256
    popped = cases(header, "popped")
    pushed = cases(header, "pushed")
    assert popped.keys() == pushed.keys()
    max_real_opcode = definitions(utils)["MAX_REAL_OPCODE"]
    effects = defaultdict(list)
    for name in popped:
        op = ids[name]
        if op > max_real_opcode or deopt[op] == op:
            effects[popped[name], pushed[name]].append(op)

    lines = [
        f"// Generated from CPython {PIN} by scripts/generate_opcode_metadata.py.",
        "// CPython license: licenses/CPython.txt.",
        "",
        "// Bits 0–5 represent the six HAS_* flags, bit 6 block pushes, and bit 7 valid entries.",
        "#[rustfmt::skip]",
        f"const FLAGS: [u8; {len(flags)}] = [",
    ]
    for offset in range(0, len(flags), 16):
        lines.append("    " + " ".join(f"{n:3}," for n in flags[offset:offset + 16]))
    lines += ["];", "", "pub(super) fn predicate(name: &str, opcode: i32) -> bool {",
              "    let mask = match name {"]
    for i, key in enumerate([*["has_" + f for f in FLAGS], "has_exc", "is_valid"]):
        lines.append(f'        "{key}" => {1 << i},')
    lines += ["        _ => return false,", "    };",
              "    FLAGS.get(opcode as usize).is_some_and(|flags| flags & mask != 0)", "}", "",
              "pub(super) fn stack_effect(opcode: i32, oparg: i32, jump: i32) -> Option<i32> {",
              "    // CPython flowgraph.c:get_stack_effects accepts base and pseudo instructions.",
              "    // Evaluate the C stack-count expressions with wrapping signed 32-bit results.",
              "    let oparg = i64::from(oparg);",
              "    let (popped, pushed): (i64, i64) = match opcode {"]
    for (pop, push), opcodes in sorted(effects.items(), key=lambda group: min(group[1])):
        lines.append(f"        {pattern(opcodes)} => ({pop}, {push}),")
    lines += ["        _ => return None,", "    };",
              "    let (popped, pushed) = (popped as i32, pushed as i32);",
              "    if popped < 0 || pushed < 0 {", "        return None;", "    }",
              f"    if jump == 0 && matches!(opcode, {pattern(blocks)}) {{",
              "        return Some(0);", "    }", "    let effect = pushed - popped;",
              "    (effect != i32::MAX).then_some(effect)", "}", ""]

    def table(name, typ, values):
        lines.append(f"pub(super) const {name}: &[{typ}] = &[")
        lines.extend(f"    {value}," for value in values)
        lines.extend(["];", ""])

    nb = definitions(source("Include/opcode.h"))
    module = source("Modules/_opcode.c")
    operators = re.findall(r'^    ADD_NB_OP\((NB_\w+), ("[^"]+")\);$', module, re.M)
    operators.sort(key=lambda pair: nb[pair[0]])
    assert [nb[key] for key, _ in operators] == list(range(len(operators)))
    table("NB_OPS", "(&str, &str)", [f'("{key}", {symbol})' for key, symbol in operators])

    defs = definitions(source("Include/internal/pycore_intrinsics.h"))
    intrinsics = source("Python/intrinsics.c")
    for name, kind in [("INTRINSIC1", "Unary"), ("INTRINSIC2", "Binary")]:
        body = intrinsics.split(f"_PyIntrinsics_{kind}Functions[] = {{", 1)[1].split("};", 1)[0]
        names = re.findall(r"INTRINSIC_FUNC_ENTRY\((\w+),", body)
        names.sort(key=defs.__getitem__)
        assert [defs[k] for k in names] == list(range(defs[f"MAX_INTRINSIC_{name[-1]}"] + 1))
        table(name, "&str", map(json.dumps, names))

    defs = definitions(source("Include/internal/pycore_ceval.h"))
    body = source("Python/ceval.c").split("_Py_SpecialMethods[] = {", 1)[1].split("};", 1)[0]
    specials = re.findall(r"\[(SPECIAL_\w+)\] = \{\s*\.name = &_Py_ID\((\w+)\),", body)
    specials.sort(key=lambda pair: defs[pair[0]])
    assert [defs[k] for k, _ in specials] == list(range(defs["SPECIAL_MAX"] + 1))
    table("SPECIAL_METHODS", "&str", [json.dumps(name) for _, name in specials])
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cpython", type=Path)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / DEST)
    args = parser.parse_args()

    def source(path):
        return subprocess.check_output(["git", "-C", args.cpython, "show", f"{PIN}:{path}"], text=True)

    args.output.write_text(generate(source))
    subprocess.run(["rustfmt", "--edition", "2024", str(args.output)], check=True)


if __name__ == "__main__":
    main()
