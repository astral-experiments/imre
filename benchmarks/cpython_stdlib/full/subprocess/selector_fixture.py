"""Transfer the original selector class and trace only the compiled parent."""

from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import selectors
import shlex


def verify_binding(module, no_poll):
    expected = selectors.SelectSelector if no_poll else selectors.PollSelector
    if module._PopenSelector is not expected:
        raise AssertionError(f"original fixture did not install {expected}")


def selected_entry(source):
    """Set the binding at the shared runner's invocation, outside the method."""
    invocation = b"\ncase = NativeCase()\n"
    if source.count(invocation) != 1:
        raise AssertionError("cannot locate the native fixture invocation")
    before, after = source.split(invocation)
    imports = b"import selectors\n"
    if b"import subprocess\n" not in before:
        imports += b"import subprocess\n"
    return (
        imports
        + before
        + b"\nsubprocess._PopenSelector = selectors.SelectSelector\n"
        + invocation
        + after
    )


class ParentTrace:
    """Each invocation checks its own execve; strace does not follow children."""

    def __init__(self, directory):
        self.directory = directory
        self.executables = {}

    def wrap(self, binary):
        binary = Path(binary).resolve()
        key = str(len(self.executables))
        self.executables[key] = binary
        wrapper = self.directory / f"trace-parent-{key}"
        prefix = self.directory / f"syscall-{key}"
        command = shlex.join(
            [
                "/usr/bin/strace",
                "-qq",
                "-e",
                "signal=none",
                "-e",
                "trace=execve,poll,ppoll,select,pselect6",
                "-o",
            ]
        )
        wrapper.write_text(
            "#!/bin/sh\nexec "
            + command
            + " "
            + shlex.quote(str(prefix))
            + ".$$ -- "
            + shlex.quote(str(binary))
            + ' "$@"\n'
        )
        wrapper.chmod(0o755)
        return wrapper

    def observations(self):
        rows = []
        for key, exe in self.executables.items():
            files = sorted(self.directory.glob(f"syscall-{key}.*"))
            if not files:
                raise AssertionError(f"no trace for {exe}")
            with exe.open("rb") as binary:
                digest = hashlib.file_digest(binary, "sha256").hexdigest()
            for file in files:
                trace = file.read_text()
                target = "execve(" + json.dumps(str(exe)) + ","
                if not any(
                    line.startswith(target) and line.endswith("= 0")
                    for line in trace.splitlines()
                ):
                    raise AssertionError(f"no successful native execve in {file}")
                calls = Counter(
                    re.findall(r"^(poll|ppoll|select|pselect6)\(", trace, re.M)
                )
                rows.append(
                    {"binary_sha256": digest, "native_parent_syscalls": dict(calls)}
                )
        if not rows:
            raise AssertionError("no native parent was executed")
        return rows
