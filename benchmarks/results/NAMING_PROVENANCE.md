# Historical evidence naming

These files present historical evidence using the current Imre name. They do not claim new measurements or new captures. Recorded source revisions, hashes, commands, timestamps, outputs, and performance values keep their historical meaning. The manifest separately identifies the current view and the exact original bytes.

Verify every current view and its inverse:

```sh
python3 benchmarks/results/restore-evidence.py verify
```

Restore one naming layer into a new directory:

```sh
python3 benchmarks/results/restore-evidence.py restore /tmp/imre-original-evidence
```

Restoration also reinstates any preceding provenance manifest, documentation, and utility. Run that restored utility in the restored tree to verify or recover an earlier layer separately. Run capture-specific replay scripts and checksum audits from the restored directory. Their hashes describe the files as they were captured. Original paths and prior support files are encoded in the current manifest.

Text recipes record only replaced occurrences, preserving preexisting current-name text. Tar recipes retain original metadata, headers, and padding plus recursively restored logical payloads. Gzip recipes preserve exact original headers and either reproducible compressed payloads or an encoded original stream. XZ streams use the original compression layout or a stored original stream. Multiblock XZ restoration requires the xz command from XZ Utils. Embedded Git bundles retain their object identities. Every layer checks both byte lengths and SHA-256 identities.
