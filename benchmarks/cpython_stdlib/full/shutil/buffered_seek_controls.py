"""Compare buffered seek recovery and the existing nonblocking write fixture on Linux."""
import fcntl
import hashlib
import os
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def compare(tools, work):
    legacy = HERE.parents[3] / 'tests/fixtures/files/nonblocking_write.py'
    reports = {}
    for source in [HERE / 'buffered_partial_case.py', legacy]:
        sub = work / source.stem
        sub.mkdir()
        binary = tools.build(source.read_bytes(), sub) if tools is not None else None

        def invoke(command, directory):
            directory.mkdir()
            reader, writer = os.pipe()
            try:
                if source.name == 'buffered_partial_case.py':
                    fcntl.fcntl(writer, fcntl.F_SETPIPE_SZ, 4096)
                    os.set_blocking(writer, False)
                    command = [*command, str(reader), str(writer)]
                result = subprocess.run(command, cwd=directory, capture_output=True, text=True,
                                        pass_fds=(reader, writer), timeout=30)
            finally:
                os.close(reader)
                os.close(writer)
            assert result.returncode == 0 and not result.stderr, (result.returncode, result.stdout, result.stderr)
            contents = (directory / 'recovered.bin').read_bytes()
            return {'stdout': result.stdout.splitlines(), 'bytes': len(contents),
                    'sha256': hashlib.sha256(contents).hexdigest()}

        expected = invoke([sys.executable, '-B', source], sub / 'reference')
        if source.name == 'buffered_partial_case.py':
            assert expected['stdout'][2:] == ['4096', 'True', 'True', 'True', 'True'], expected
        if binary is not None:
            actual = invoke([binary], sub / 'native')
            assert actual == expected, (source.name, actual, expected)
        reports[source.name] = expected
    return reports
