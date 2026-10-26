"""Exercise the public handler and server over loopback on Linux and macOS."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

SUITE = Path(__file__).resolve().parents[1] / 'benchmarks/cpython_stdlib/full/http'
sys.path.insert(0, str(SUITE))
from server_cases import public_contracts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--clang', default='clang')
    parser.add_argument('--opt-levels', nargs='+', default=['0', '3'])
    args = parser.parse_args()
    source = SUITE / 'server_contract.py'
    if sys.version_info[:3] == (3, 14, 5):
        public_contracts([sys.executable, '-B', source])
    with tempfile.TemporaryDirectory() as tmp:
        binary = Path(tmp) / 'handler'
        for level in args.opt_levels:
            subprocess.run([args.compiler.resolve(), 'build', source, '--no-cache', '--runtime', args.runtime.resolve(),
                            '--clang', args.clang, '--opt-level', level, '-o', binary], check=True, timeout=180)
            public_contracts([binary])
            print(f'O{level}: delayed binding, HTTPStatus keys, HTTP/0.9 mapping and HTTP/1.1 subclass dispatch passed')


if __name__ == '__main__':
    main()
