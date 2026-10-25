"""Compare the warning site, filters, ordering, and returned OS errors on private fds."""

import os
from pathlib import Path
import re
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def warnings(stderr):
    rows = []
    for line in stderr.splitlines():
        if line.startswith('  '):  # CPython's optional source excerpt from linecache
            continue
        match = re.fullmatch(r'(.+):(\d+): (\w+Warning): (.*)', line)
        if not match:
            raise AssertionError(f'unexpected stderr: {line!r}')
        filename, lineno, category, message = match.groups()
        rows.append([Path(filename).name, int(lineno), category, message])
    return rows


def compare(tools, work):
    source = HERE / 'descriptor_warning_case.py'
    native = tools.build(source.read_bytes(), work) if tools is not None else None
    private = work / 'owned-file'
    private.touch()
    cases = [(a, op, False) for a in ('always', 'default', 'ignore', 'error', 'parent', 'line')
             for op in ('disk', 'own', 'directory', 'both')]
    cases += [(a, op, False) for a in ('always', 'error') for op in ('bad_uid', 'bad_group', 'no_ids')]
    cases += [(a, op, True) for a in ('ignore', 'error') for op in ('disk', 'own')]
    rows = []
    for action, operation, closed in cases:
        def invoke(command):
            with private.open('rb') as stdin:
                proc = subprocess.run([*command, action, operation, str(private)], cwd=work, stdin=stdin,
                                      capture_output=True, text=True, check=True, timeout=20,
                                      preexec_fn=(lambda: os.close(0)) if closed else None)
            return {'stdout': proc.stdout.splitlines(), 'warnings': warnings(proc.stderr)}
        expected = invoke([sys.executable, '-B', source])
        if native is not None:
            actual = invoke([native])
            if actual != expected:
                raise AssertionError((action, operation, closed, expected, actual))
        rows.append({'action': action, 'operation': operation, 'closed': closed, **expected})
    return rows
