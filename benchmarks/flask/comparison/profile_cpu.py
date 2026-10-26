"""Profile 900 verified mixed requests using the single Werkzeug thread's CPU clock."""
import argparse
import cProfile
import json
import os
from pathlib import Path
import pstats
import runpy
import signal
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compare import cases, gate


def run_server(args):
    profiler = cProfile.Profile(timer=time.thread_time_ns, timeunit=1e-9)
    work, site, launcher = map(Path, args)

    def start(signum, frame):
        profiler.enable()
        (work / "profiling").touch()

    def finish(signum, frame):
        profiler.disable()
        profiler.dump_stats(work / "cpu.pstats")
        raise SystemExit(0)

    signal.signal(signal.SIGUSR1, start)
    signal.signal(signal.SIGUSR2, finish)
    ready = work / "port"
    if launcher == site / "serve.py":
        sys.path.insert(0, str(site))
        sys.argv = [str(launcher), str(ready)]
    else:
        sys.argv = [str(launcher), str(site), str(ready)]
    runpy.run_path(str(launcher), run_name="__main__")


def wait_for(file, process):
    limit = time.monotonic() + 15
    while not file.exists():
        if process.poll() is not None or time.monotonic() > limit:
            raise RuntimeError(f"server did not write {file}")
        time.sleep(0.01)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("wheelhouse", "client", "hosted-launcher", "output-dir"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--server-cpu", type=int, default=4)
    parser.add_argument("--client-cpus", default="5,6")
    args = parser.parse_args()
    args.output_dir.mkdir(exist_ok=False, parents=True)
    with tempfile.TemporaryDirectory() as temp:
        site = Path(temp) / "site"
        site.mkdir()
        wheels = gate.stage(args.wheelhouse, site)
        fixtures = Path(temp) / "cases.json"
        fixtures.write_text(json.dumps(cases()))
        report = {"wheels": wheels, "app_sha256": gate.digest(site / "tiny_flask_app.py"),
                  "requests": 900, "concurrency": 8, "clock": "time.thread_time_ns", "servers": {}}
        for name, launcher in (("cpython", site / "serve.py"), ("hosted_llvm", args.hosted_launcher.resolve())):
            work = args.output_dir.resolve() / name
            work.mkdir()
            command = ["taskset", "-c", str(args.server_cpu), sys.executable, "-ESB",
                       Path(__file__).resolve(), "--server", work, site, launcher]
            with (work / "stderr").open("w") as stderr, subprocess.Popen(command, stderr=stderr) as process:
                try:
                    wait_for(work / "port", process)
                    client = ["taskset", "-c", args.client_cpus, args.client.resolve(), "-cases", fixtures,
                              "-addr", "127.0.0.1:" + (work / "port").read_text(), "-profile", "mixed", "-c", "8"]
                    env = {**os.environ, "GOMAXPROCS": str(len(args.client_cpus.split(",")))}
                    subprocess.run([*client, "-n", "90"], env=env, check=True, stdout=subprocess.DEVNULL)
                    process.send_signal(signal.SIGUSR1)
                    wait_for(work / "profiling", process)
                    subprocess.run([*client, "-n", "900"], env=env, check=True, stdout=subprocess.DEVNULL)
                    process.send_signal(signal.SIGUSR2)
                    process.wait(timeout=5)
                    if process.returncode != 0:
                        raise RuntimeError(f"{name} profiling server exited with {process.returncode}")
                finally:
                    if process.poll() is None:
                        process.kill()
            stats = pstats.Stats(str(work / "cpu.pstats"))
            funcs = []
            for (file, line, function), (_, calls, self_time, cumulative, _) in stats.stats.items():
                if file.startswith(str(site)):
                    file = str(Path(file).relative_to(site))
                funcs.append(dict(file=file, line=line, function=function, calls=calls,
                                  self_seconds=self_time, cumulative_seconds=cumulative))
            report["servers"][name] = {"thread_cpu_seconds": stats.total_tt, "functions": funcs}
        (args.output_dir / "profile.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    if sys.argv[1:2] == ["--server"]:
        run_server(sys.argv[2:])
    else:
        main()
