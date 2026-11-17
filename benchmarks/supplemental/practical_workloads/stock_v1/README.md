# Prospective practical workloads: stock reference v1

This is a new supplemental, stock-only reference corpus. It is not a repair, receipt, reference, or source of credit for the frozen historical 84-script cohort. It does not change that cohort, its three independently anchored verified pairs, or any scorecard numerator. These cases acquire prospective authority only through independent review of this stock-only commit before a separate native replay is proposed.

The entry files are unchanged public Git objects:

| Case | Project commit | Path | Git blob | SHA256 | Bytes |
| --- | --- | --- | --- | --- | ---: |
| ruff-publisher | `65aee8892ce379af4ae9c8b5f6e0ab37b25d5ec9` | `scripts/publish-crates.py` | `0ad38297e973be20e7b22a6c70332047ae522629` | `c6f13456d53d1dd1cc155d72d9e63cd999e361e1c983d366e477c8432693a021` | 6,891 |
| uv-publisher | `24688aaca1e7fd104437a0909d5db3afdc7dcdb0` | `scripts/publish-crates.py` | `474dcdbb534b65cd3ab9dce1101a98f618389b4e` | `bc45ffb7097a253601a62a46ac7b356d3005a5b12d5be346eacc5a5df412bddb` | 6,118 |
| uv-rich | `24688aaca1e7fd104437a0909d5db3afdc7dcdb0` | `scripts/uv-run-remote-script-test.py` | `e21e3413afc5fc41942e29ccf7f717af3614ac12` | `a479935e4c75c1b31dcd4805b50464ffa8a37e7240acb7c6457a2e4033ae21b0` | 297 |
| ty-readme | `d4e4bf90906c7677049bc3a29d1735de7d22052d` | `scripts/transform_readme.py` | `4b20727cd3b38ef011b9f779a706d977089c487a` | `d6a13d50218b55e3b73587974e8357b9bc68b7c364d8bb10d04708fec5e2d64f` | 2,641 |

`reference.schema.json` defines one reference per command. Each `references/<case>/reference.json` binds the source, the complete normalized argv/cwd/environment, explicit execution umask `0077`, CPython executable and Linux identities, before-manifest, external input hashes, exit, raw streams, and raw/normalized evidence hashes. `raw/invocation.json` retains the actual invocation and placeholder expansion; `raw/before.json` and `raw/after.json` enumerate the entire writable fixture. Registry and subprocess journals preserve raw request headers, controlled responses, argv, cwd, effective environment, exit and streams. The normalized effects are derived from those journals and the before/after delta. The created Cargo journal uses the normalized journal bytes for its normalized size and digest; its original bytes and raw digest remain in the after-manifest. No ambient shell environment or credentials are passed to the workload. The capture sets `os.umask(0o077)` before creating fixtures or starting subprocesses. Each fresh reference records that effective execution control; its raw capture context records the observed outer umask `0022`, testing that the override works. The prior freeze did not record an umask and is not evidence for that control. `umask-recapture.json` compares the new raw artifacts and normalized effects against immutable prior commit `e88ee56b6ca451904475dafc8cdbf31f3daf9c5e`. Raw temporary paths, loopback ports, and the publisher journals that contain them can differ; program outcomes, fixture preimages, saved output files, and normalized effects match.

The publisher fixture runs only a loopback HTTP server and `fixtures/cargo_stub.py` under the pinned Python. Metadata describes `already@1.0` and `fresh@2.0`; the registry returns 200 for `/api/v1/crates/already/1.0` and 404 otherwise. The stub accepts only the expected metadata and publish argv. It writes a publish marker and journal; it cannot invoke Cargo or publish crates.

The ty README case uses unchanged `README.md` and `dist-workspace.toml` bytes from the same ty commit, with their public Git blobs and SHA256 values bound in `ty-inputs.json` and the reference preimage. CPython exits without writing stdout or stderr and modifies only `README.md`; the complete 5,755-byte output is retained under `references/ty-readme/raw/README.md` (SHA256 `ea831c09eb6dc99d10dc36f8ba3f4002bec3765b04d2d9d7fac4b117bbe43d2f`). This is a new prospective command and does not credit or establish historical authority for ty-05.

The uv greeting uses the existing Cloud cache of Rich 13.7.1. `fixtures/rich-imports.zip` contains only the 49 source modules imported by this command; `rich-members.json` binds every byte to its Git blob at Textualize/rich tag `v13.7.1`, commit `7f580bdcf07a3b269a0e786b6a3aa9c804f393cf`. The upstream license is included. `dependencies.json` pins official wheel filenames, sizes, hashes and public lock-file Git objects for Rich 13.7.1, markdown-it-py 4.3.0, mdurl 0.1.2 and Pygments 2.21.0. Wheel bytes were not downloaded or installed for this stock run: the configured registry was unavailable, and the command imports no transitive package. Those wheel hashes are distribution metadata, not claims that wheel bytes formed the stock preimage. The actual preimage is the archived, hash-verified cached source subset. No public PyPI registry was used for uv.

The freeze sequence is intentionally one-way:

1. Select and independently verify public source objects, fixture bytes and schema.
2. Run only CPython 3.14.5 on Linux to capture new stock observations. The capture utility refuses to overwrite an existing reference.
3. Run the offline source/evidence validator and publish this stock-only commit for independent review and merge.
4. Only after that review/merge may a separate worker build native executables and compare them to this immutable commit. Native observations must be separate; they cannot revise this reference in place.

Capture on a separate copy without `references/` (requires the exact interpreter and a writable `/workspace/scratch`):

```sh
# Run with a different ambient mask to test the explicit override:
umask 0022
/path/to/python3.14 -B capture_stock.py
# Or append a new case without replacing any existing reference:
/path/to/python3.14 -B capture_stock.py --case ty-readme
```

Validate offline without running a workload:

```sh
python3 -B validate.py
```
