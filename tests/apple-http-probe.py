import concurrent.futures
from pathlib import Path
import socket
import subprocess
import sys
import time


def measure(label, operation):
    then = time.monotonic()
    try:
        value = operation()
    except Exception as exc:
        value = repr(exc)
    print(f'{label} after {time.monotonic()-then:.1f}s: {value!r}', flush=True)
    return value


def probe():
    source = Path('benchmarks/cpython_stdlib/full/http/server_contract.py')
    with concurrent.futures.ThreadPoolExecutor() as pool:
        reference = pool.submit(measure, 'CPython getfqdn', lambda: socket.getfqdn('127.0.0.1'))
        pool.submit(measure, 'CPython gethostbyaddr', lambda: socket.gethostbyaddr('127.0.0.1'))
        pool.submit(measure, 'Darwin C host lookup', lambda: subprocess.run([sys.argv[2]], capture_output=True, text=True))
        name = reference.result()
        for label, command in [('native', [sys.argv[1]]), ('reference', [sys.executable, '-B', source])]:
            pool.submit(measure, label, lambda cmd=command: subprocess.run([*cmd, name, 'normal', '1'], capture_output=True, text=True, timeout=125))


probe()
