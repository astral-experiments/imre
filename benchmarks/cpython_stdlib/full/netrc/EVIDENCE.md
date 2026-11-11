# CPython 3.14.5 netrc family on Linux

Stock CPython and Imre #6989 with `test.test_netrc` declared each ran all 23 originals in upstream order. The same 22 passed at stock, O0 and O3. All three skipped `NetrcTestCase.test_security_on_WASI` with reason `WASI only test`. In the ordinary Imre invocation, `unittest.loader.loadTestsFromName` raised `NotImplementedError: no compiled Python body for <module>` at the import of `test/test_netrc.py` on both tiers, before any original started. [Occurrence ledger](occurrences.tsv)

The earlier 44-family stock campaign had no netrc row. Adding these 22 to both the numerator and denominator brings the fraction of Linux stock passes that also passed at both Imre tiers from 13,039/15,161 to 13,061/15,183. The WASI skip counts toward the 23 originals and registered tests.

## Original sources and collection

The frozen `Lib/test` archive contained 1,639 files, verified against CPython commit `5607950ef232dad16d75c0cf53101d9649d89115`. Stock imported `lib/python3.14/netrc.py` from the installed python-build-standalone distribution. Imre commit `297d3405dfe50c7759feb48d188c2e6cf4a23ddd` registered the same provider bytes as `netrc.py` in its SDK ZIP. [Upstream test](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_netrc.py) · [Registered Imre source](https://github.com/astral-experiments/imre/blob/297d3405dfe50c7759feb48d188c2e6cf4a23ddd/crates/imre-compiler/stdlib/installed-sources.txt#L6) · [Stock release](https://github.com/astral-sh/python-build-standalone/releases/tag/20260602)

No stock run UUID was recorded. The stripped Linux stock ELF has no GNU build-ID note.

| Input | SHA-256 |
| --- | --- |
| Frozen CPython `Lib/test` archive | `1e6c7c0d35b0ddb9150c3a142a1ea52810f5e3cb13fce7ca9bb23f3e59c9a196` |
| `Lib/test/test_netrc.py` | `43982da3abb25bb0bc981196c9c2da5bb6c2fce63719be3bb8804224e9355c65` |
| CPython and Imre `netrc.py` | `3ddc67d7599759d745525af7e00ef1bf2d6bb958129698169b7548f17b81cbe8` |
| Stock release, x86-64 GNU/Linux install-only stripped archive | `982a60fc7f11bbe405ab54afcd4eb145f1134797bbbffac9f5c49df4ec98b13e` |
| Stock `bin/python3.14` | `c27397da5541f67655be462e128ca3d3eb6e0a8eb0d39036896c9042529f4304` |
| Imre compiler | `bdc01961ddc80c4290111f82ea9acc8543d15499755c7a129ad46de64a65e8d6` |
| `imre-stdlib.zip` | `f652b1ec75bb4e8c77699e74d837c6a1488d6051e706cf8eec30a26f5399758e` |

Stage the unchanged `test/` subtree alone in a directory on `PYTHONPATH`. Use the linked #6989 Linux SDK and a separate cache per optimization level. Give each parent a fresh working directory, `HOME` and `TMPDIR` owned by UID/GID 1000, with mode 0700 and umask 077. The runs used `LC_ALL=C.UTF-8`, `TZ=UTC`, `PYTHONHASHSEED=0`, `PYTHONNOUSERSITE=1` and `PYTHONDONTWRITEBYTECODE=1`. The declaration links the upstream module that unittest selects by name. [Module declaration syntax](../../../../docs/subset.md#first-party-module-graphs)

```sh
"$stock" -B -m unittest -v test.test_netrc
# Run each Imre command with level=0 and level=3.
export IMRE_ENCODED_FLAGS="$(printf '%s\037%s' --opt-level "$level")"
"$imre" -B -m unittest -v test.test_netrc
"$imre" -B --include-module test.test_netrc -m unittest -v test.test_netrc
```

## Process receipts

| Intact parent | PID | Raw wait / exit | Original results | stderr bytes | stderr SHA-256 |
| --- | ---: | --- | --- | ---: | --- |
| Stock | 134058 | 0 / 0 | 22 pass, 1 skip | 2434 | `dbffe168c83a7bc21d46a04f799a65a21fc5c6c363255ecf61a8a15064578044` |
| Ordinary O0 | 144488 | 256 / 1 | 0 started | 1256 | `7820ba3d84cbdb4d1bbd55cf74b1c86c695819703822ae98586ab66557ac614e` |
| Ordinary O3 | 145435 | 256 / 1 | 0 started | 1256 | `7820ba3d84cbdb4d1bbd55cf74b1c86c695819703822ae98586ab66557ac614e` |
| Declared O0 | 146495 | 0 / 0 | 22 pass, 1 skip | 2434 | `199526104578b898715ac86bd0597bf866ccfef780e695a88e0dc3f38c0d80ac` |
| Declared O3 | 146564 | 0 / 0 | 22 pass, 1 skip | 2434 | `24e2f80ae51ad954a28ea9d079c209cb7c18c722d1d0ab78b35e8dc8ecae5b95` |

All five parents had empty stdin and stdout (SHA-256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`). Ledger offsets are zero-based byte positions in the corresponding raw stderr. Stock offsets locate the original verbose line. Declared offsets locate its terminal status.

| Declared ELF | GNU build ID | SHA-256 of executed ELF | Relocated PID / raw wait |
| --- | --- | --- | --- |
| O0 | `36ab6781ad0bb6b6` | `3392e7e72fe657bd985a000bff8349320aceea5046d7f6ca90bdc98b0ddd5547` | 147306 / 0 |
| O3 | `90c868ee77137682` | `6ab7d24e6dcefca3958fea5f2280c725c2a39f83ae4ba9ad26d0b00357af8dbb` | 147322 / 0 |

Both relocated ELFs repeated all 23 outcomes. With the frozen test and installed stock trees masked and the SDK ZIP visible, both again matched stock. Hiding the SDK ZIP as well produced one synthetic loader error for missing `netrc` and zero original starts.

## Permission and skip evidence

Stock observer PID 134691 recorded 23 unittest starts and stops, 22 setup and method entries, and 62 parser constructions. The WASI guard skipped before setup. Separate declared-parent syscall traces (PIDs 147312 and 147343) recorded creation of the temporary home and its `.netrc`, and successful chmod calls in the order 0600, 0622, 0600, 0622. The test accepts the owned 0600 file, asserts `NetrcParseError` on the nonanonymous 0622 file, then parses an anonymous login at 0600. Its final assertion after chmod reuses that parsed entry. The file belonged to UID/GID 1000 throughout. Wrong-owner behavior was outside this upstream test. All private fixture trees returned to their prelaunch state. Native per-ID results come from unittest's verbose stream. [Upstream permission assertions](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_netrc.py#L268-L309)

Raw receipts, program images, fixtures and the original roster are indexed by `DELIVERY-MANIFEST.json` in `imre-public-6989-cpython-3.14.5-test-netrc-controls.tar.gz` (SHA-256 `9246c017380aa83e2880fb56249ac2ee781183f8faea6019b16f5f2313957822`). The occurrence ledger projects `report/occurrence-join.json` (SHA-256 `17eff69f1f37e80db24b88c98ec4a45f54a4c4e7be802c53beb44a0f1a18b308`).
