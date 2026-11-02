# OrderedDict on Linux

The pinned CPython 3.14.5 loader registered 295 OrderedDict tests. The native runs below use Imre's builtin dict or a fresh import of its native-backed `_collections.OrderedDict`. Each result uses the original fixture class and an unchanged method body. The archive includes the source selections and their hashes.

| New paired registrations | Count |
| --- | ---: |
| Builtin dict `test_abc` | 1 |
| Accelerated base and subclass `test_abc` | 2 |
| Accelerated base and subclass `test_repr` | 2 |
| Accelerated base and subclass mapping `test_read` | 2 |

Eight previously credited builtin-dict methods and 30 previously credited mapping and LRU methods also passed at O0, O3 and on the reference. The supplied historical ledger had 228 OrderedDict keys in its 528-key collections union. The seven additions give a union of 235 OrderedDict keys across revisions.

The unmodified module on the initial public Imre revision `2a1d0281` stops before discovery. The archive records that run separately from the source-selected public passes, and includes the 228-case regression batch, reproducible native and reference source, raw streams, controls and teardown.

[Registration index](registrations.json) · [Seven-addition checkpoint](public-seven.tar.gz)

The SHA-256 of the archive is `ed5e3330e1e4835f4be3ecfb3e89eae568792735472c9af6b4f96df1af7797cb`.
