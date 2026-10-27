"""Compare complete programs in both compiler modes; ensure unsupported paths stay explicit."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap

HERE = Path(__file__).resolve().parent
CASES = ("bindings", "tasks", "cancel", "old_values", "representations", "first_use", "error_order", "shared_frame")
BLOCKED = {
    "thread": ("""
        import threading
        from contextvars import Context
        t = threading.Thread(context=Context()); t.start(); t.join()
    """, "unsupported sealed standard-library export threading.Thread"),
    "task_context": ("""
        import asyncio
        from contextvars import Context
        async def child() -> int:
            return 3
        async def main() -> None:
            async with asyncio.TaskGroup() as group:
                group.create_task(child(), context=Context())
        asyncio.run(main())
    """, "keyword"),
    "callback_keywords": ("""
        from contextvars import Context
        def plus(x: int) -> int:
            return x + 1
        print(Context().run(plus, x=4))
    """, "complete positional callback signature"),
    "mapping_equality": ("""
        from contextvars import Context
        print(Context() == Context())
    """, "Context mapping equality"),
    "nested_call": ("""
        from contextvars import Context
        def only(x: int) -> int:
            return x
        try:
            Context().get(7, only())
        except TypeError as exc:
            print(str(exc))
    """, "missing"),
    "retains_token": ("""
        from contextvars import ContextVar, Token
        owner: ContextVar[int] = ContextVar('owner')
        retained: ContextVar[Token[int]] = ContextVar('retained')
        retained.set(owner.set(1))
    """, "acyclic data layout"),
    "default_hook": ("""
        from contextvars import ContextVar
        class Display:
            def __init__(self) -> None:
                pass
            def __repr__(self) -> str:
                return "custom"
        v = ContextVar("name", default=Display())
        print("custom" in repr(v))
    """, "default with native repr"),
}


def run(argv):
    return subprocess.run(argv, capture_output=True, timeout=30)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang"):
        p.add_argument("--" + flag, type=Path, required=True)
    args = p.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        p.error("use CPython 3.14.5")
    with tempfile.TemporaryDirectory() as d:
        directory = Path(d)
        entries = [(name, HERE / (name + ".py"), None) for name in CASES]
        for name, (body, diagnostic) in BLOCKED.items():
            entry = directory / (name + ".py")
            entry.write_text(textwrap.dedent(body))
            entries.append((name, entry, diagnostic))
        for name, entry, diagnostic in entries:
            reference = run([sys.executable, "-IB", entry])
            if reference.returncode or reference.stderr:
                raise AssertionError((name, reference))
            for level in (0, 3):
                for reachable in (False, True):
                    exe = directory / "checked"
                    cmd = [args.imre.resolve(), "build", entry, "-o", exe, "--no-cache",
                           "--opt-level", str(level), "--runtime", args.runtime.resolve(),
                           "--clang", args.clang.resolve()]
                    if reachable:
                        cmd.append("--reachable-only")
                    built = run(cmd)
                    if diagnostic is not None:
                        expected = diagnostic[int(reachable)] if isinstance(diagnostic, tuple) else diagnostic
                        if not built.returncode or expected not in built.stderr.decode():
                            raise AssertionError((name, level, reachable, built.stderr))
                    else:
                        if built.returncode:
                            raise AssertionError((name, level, reachable, built.stderr))
                        actual = run([exe])
                        if (actual.returncode, actual.stdout, actual.stderr) != (
                            reference.returncode, reference.stdout, reference.stderr
                        ):
                            raise AssertionError((name, level, reachable, actual, reference))
            print(name, "explicit diagnostic" if diagnostic else "four native matches", flush=True)


if __name__ == "__main__":
    main()
