"""Declared native subset used by the live conformance application."""

import json

from codon_platform import CalledProcessError, TimeoutExpired, call


@dataclass
class CompletedProcess:
    returncode: int
    stdout: str
    stderr: str


def run(argv: list[str], cwd, env: dict[str, str], capture_output: bool,
        text: bool, encoding: str, check: bool, timeout: float) -> CompletedProcess:
    if not capture_output or not text or encoding != "utf-8":
        raise ValueError("native benchmark subprocess requires UTF-8 capture")
    request = ('{"argv":' + json.dumps(argv) + ',"cwd":' + json.dumps(str(cwd))
               + ',"env":' + json.dumps(env) + ',"timeout":' + json.dumps(timeout)
               + ',"check":' + json.dumps(check) + '}')
    result = json.loads(call(2, request))
    return CompletedProcess(result["returncode"].as_int(), result["stdout"].as_str(), result["stderr"].as_str())
