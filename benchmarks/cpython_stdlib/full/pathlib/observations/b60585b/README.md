# Registered pure-path fixtures on Linux

CPython 3.14.5 discovers 1,369 occurrences in the unchanged `test.test_pathlib` package (978 passes, 391 skips). The primary evidence contains 136 complete native bodies with a host fixture at both O0 and O3. Of those, 45 belong to three classes with the same original `PurePathTest.setUp`. This probe ran the three complete classes under CPython, including methods outside those 45.

| Original class | Registered | CPython pass | CPython skip | Native body with host fixture |
| --- | ---: | ---: | ---: | ---: |
| PurePathTest | 60 | 44 | 16 require Windows flavour | 15 |
| PurePosixPathTest | 60 | 44 | 16 require Windows flavour | 15 |
| PureWindowsPathTest | 60 | 52 | 8 require POSIX flavour | 15 |

Every registered result and skip reason matched the pinned reference. A second discovery exercised the actual setup hooks independently: the POSIX classes received `posixpath`, `/`, and `None`; the Windows class received `ntpath`, `\\`, and `/`. Each skip matched the upstream decorators' live method-name sets. The [per-occurrence report](registered-pure-fixtures-linux.json) identifies the 11 candidate method names shared by all three classes, each registered ID, its parser or skip reason, and its staging result. The shared runner rejects the 15 direct-class candidates because they have a custom `setUp`, and the 30 inherited candidates because it requires a direct subclass of `TestCase`.

[Native build probes](native-pure-fixtures-linux.json) used a compiler built from `b60585b750b8ba4deafe7f2fd25de930041862a8`. At O0 and O3 the untouched package runner stops in `test.support` (line 133 in normal mode, line 1613 in reachable-only mode). Direct registration of the three classes through their real module stops at `import ntpath` (line 7), in both modes. The [shared startup checker](package-startup-linux.json) independently reports the package import and runner boundaries.

The first expression of the unchanged setup, `self.id().split('.')[-1]`, fails with `unknown class method` in the isolated source probe. A separately registered control fails on `self.id()` alone. Independent probes reject the original class factory field and the class and instance parser attributes. Both flavour-specific `skipTest` controls run at each level, preserve their reason, and suppress the body. These controls are outside the CPython denominator. No original case reached native fixture registration here; the primary counts have not changed. Concrete `PathTest` and `PosixPathTest` also need their filesystem setup and teardown.

## Reproduce

Use the test-enabled CPython at the pinned release, a Git repository containing that release, and the matching compiler, runtime and SHA-256 manifest in `$ARTIFACTS/source.json` (the format appears under `source` in the native report). Run from the repository root:

```sh
dir=benchmarks/cpython_stdlib/full/pathlib
"$PYTHON" -B -X warn_default_encoding "$dir/pure_fixture_probe.py" --result registered.json
"$PYTHON" -B "$dir/pure_fixture_native.py" --artifacts "$ARTIFACTS" --clang "$CLANG" --cpython-git "$CPYTHON_GIT" --result boundaries.json
"$PYTHON" -B benchmarks/cpython_stdlib/full/check_suite_startup.py --compiler "$ARTIFACTS/imre" --cpython-git "$CPYTHON_GIT" test.test_pathlib
```

The first command optionally takes `--candidate-manifest` to check the coordinator's selection. The last command exits nonzero when suite startup is blocked.

The [recheck on the inherited-exception compiler](../d7fc14f/README.md) uses these same probes and a verified hosted Linux archive.
