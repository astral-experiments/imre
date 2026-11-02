# OrderedDict on Linux

The pinned CPython 3.14.5 loader registered 295 OrderedDict tests. The native runs below use Imre's builtin dict or a fresh import of its native-backed `_collections.OrderedDict`. Each result uses the original fixture class and an unchanged method body. The archive includes the source selections and their hashes.

| New paired registrations | Count |
| --- | ---: |
| Builtin dict `test_abc` | 1 |
| Accelerated base and subclass `test_abc` | 2 |
| Accelerated base and subclass `test_repr` | 2 |
| Accelerated base and subclass mapping `test_read` | 2 |
| Accelerated base and subclass iterator pickling | 2 |
| Accelerated base and subclass mutations during equality | 8 |

The 17 additions give a union of 245 OrderedDict keys across revisions. The supplied historical ledger held 228 OrderedDict keys in the 528-key collections union. A further 92 registrations from that ledger passed at O0 and O3 on public builds.

The unchanged module on public Imre revision `97052322` stops before discovery at the import of `random.shuffle`. All 13 class selections and the 228-case regression batch reproduced that result. The archive also includes source-selected results, original fixture identity checks, raw streams, executable hashes, failing controls and teardown.

[Registration index](registrations.json) · [Seventeen-addition checkpoint](public-seventeen.tar.gz) · [Earlier seven-addition checkpoint](public-seven.tar.gz)

The SHA-256 of the seventeen-addition archive is `63fcdbaacbe903295eb5ea1bd067b2061c1576ae1f039e54432e1e8f0133c9a6`.
