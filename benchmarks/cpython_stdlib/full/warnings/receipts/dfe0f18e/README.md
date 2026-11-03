# Warnings suite on public revision dfe0f18e

The three complete callback streams cover all 187 registrations from the Linux CPython 3.14.5 inventory in discovery order. Stock passed 181 and skipped six. Both native runs recorded 172 passes, 10 skips, four failures and one error. Of the 172 native passes, 155 qualified in process at both levels with matching stock parent and subtest outcomes. The other 17 passed after invoking host Python.

`parents.tsv` gives the original ID, occurrence, outcome and disposition of every parent. The last three columns give the one-based line numbers for its registration, start, result and stop in the corresponding `intact_fixture.stdout`. The `raw.tar.gz` archive holds those files under `stock/stock/`, `public-dfe0f18e/O0/` and `public-dfe0f18e/O3/`. Alongside them are the process records, build commands and hashes, callback recorder, pinned sources and original inventory.

The native build compiled a launcher that imports the unchanged upstream suite at runtime and calls its standard unittest loader. Each run verified distinct C and pure Python warning providers. The standalone native launchers reported a host Python executable for child interpreter calls. The 19 parents whose methods invoke it require a separate native-child run before they can count. Four parents skipped only on Imre because the upstream methods require CPython. Three in-process parents failed or errored on Imre. The warnings omission count is zero.

Run these commands from this directory to verify the archive and compare the generated ledger.

```bash
set -o pipefail
python3 -I -B verify.py | diff -u parents.tsv -
```
