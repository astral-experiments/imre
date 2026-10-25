# Pathlib fixture recheck on the inherited-exception compiler

The [Linux archive job](https://github.com/astral-sh/imre/actions/runs/40228241550/job/121274241666) built commit `d7fc14f2004fd6b3c494c4e46c9eae2b2354066b` from [#971](https://github.com/astral-sh/imre/pull/971). The [receipt](hosted-linux.json) identifies the Git tree, hosted artifact, archive digest and both binary digests. The drivers from `25780479733690b4b03d4f8d7c9c92e8adf6c034` used the complete pinned CPython `Lib/test` tree.

In normal mode, both [package entry checks](package-startup-linux.json) advance from `test/support/__init__.py:133` to the undeclared `procfile` field at line 1155. Reachable-only import and runner still stop at the conditional class declaration at line 1613. None of those checks ran a test.

The [O0/O3 builds](native-pure-fixtures-linux.json) reproduce the package results. Direct registration of the three pure classes still stops at `test_pathlib.py:7`, `import ntpath`. The setup still cannot call `self.id()`, and both native `skipTest` controls preserve the upstream flavour reason. The [180 registered CPython cases](../b60585b/registered-pure-fixtures-linux.json) and their 45 native-body candidates retain their original classifications. Reproduce with the [same commands](../b60585b/README.md#reproduce) and the artifact identified above.
