# Linux CPython 3.14.5 getopt side measurement

This directory records a nine-ID module outside the fixed 25-module stdlib campaign. The driver imports the intact upstream test module, calls unittest.defaultTestLoader.loadTestsFromModule, records discovery order including the doctest added by load_tests, and runs the original suite. The inputs include the actual pygettext tool and six-msgid translation snapshot.

On tested #3764 head b8ddfffb1ff622d7fca2c7ec4ce7936772632108, the native parent plus stock CPython child passed all nine at both -O0 and -O3. This is a host-assisted ninth case. The parent plus actual Imre child passed eight at each level and failed test.test_getopt.TestTranslations.test_translations. The installed layout uses the real shipped getopt.py at an accessible source path. The embedded layout leaves the provider at its production virtual path. The wrong layout changes only --no-location to --imre-invalid-location in a separate copy of the upstream fixture. The original return-code assertion then rejects the invocation. The positive test module and its assertions are unmodified.

## Replay

Use a Linux Imre compiler/runtime pair, its commit, tree and pair.sha256 metadata, Clang 18, and a built CPython 3.14.5 checkout containing 5607950ef232dad16d75c0cf53101d9649d89115. The prepare script reconstructs the input files from that Git object. The three commands below use a fresh work directory. The two native children run serially against each linked parent. The runner captures process-group timeouts, stdout, stderr, exits, artifact hashes and observed child processes in result.json.

```sh
python3 prepare.py /absolute/cpython /absolute/kit
python3 /absolute/kit/replay.py --work /absolute/work --tag stock --cpython /absolute/cpython/python --layouts installed wrong
python3 /absolute/kit/replay.py --work /absolute/work --tag tested --pair /absolute/pair --clang /absolute/clang-18 --cpython /absolute/cpython/python --layouts installed wrong --levels 0 3 --children stock imre
```

For a packaged-only before comparison, run the same native command with --layouts embedded on 35eb40f271c75546bec3e500436ce95b18495c2d. The earlier reporting expression missed CPython's multiline doctest description; results.json includes the ninth status from the retained raw output. A missing process sample is inconclusive. The child configurations use explicit IMRE_PYTHON and compiler flags. The host-assisted route has no independent native translation credit.
