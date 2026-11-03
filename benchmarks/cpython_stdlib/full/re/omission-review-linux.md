# Evidence for seven Linux `re` omissions

The campaign accepted these seven originals as CPython implementation details. Each stays in the denominator and earns zero passes. The ordinary-profile ledger at review time had 147 of 165 paired originals. Two more passed with provisioned locales. The proposal register preserves the original submission. [Individual decisions](../../../../docs/stdlib-omissions.md#re) · [Proposal register](pending-omissions-linux.tsv)

Each heading gives the registration ordinal. All originals and paired counterparts are occurrence 1. Pinned CPython 3.14.5 and ordinary native O0 and O3 ran the unchanged module under `LC_ALL=C.UTF-8` with `LOCPATH` unset. The processes running the seven proposed originals exited 0, 1 and 1 respectively. The individual callbacks appear under each decision. The tested Imre revision was `606353950adc0b51d450769a83706ddee2522029`. [Pinned module](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py)

For the three DEBUG decisions, `get_debug_out` captures stdout from `re.compile(pat, re.DEBUG)`. A class-level `@cpython_only` skips the original before the comparison on Imre. The documented flag displays information about a compiled expression. Separate public controls verified repeatable diagnostics, cache bypass and matching. The fourth guarded method, `DebugTests.test_debug_flag`, retains its public output and cache requirements. [Capture helper and class guard](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2714-L2724) · [Documented DEBUG flag](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Doc/library/re.rst#L764-L768) · [Public DEBUG controls](https://github.com/astral-experiments/imre/pull/4515)

The callback extract includes each selection, optimization level, owner exit and one-based stdout line. The columns from `EVENT` onward are verbatim. Three compact raw archives preserve the complete streams, argv and build hashes. Their SHA-256 prefixes are `7aea80b5eb82c1c5` for the proposed and deprecated originals, `6a14599a1e760a11` for the seven public counterparts and `a294fc6e5a98952f` for the two group-limit counterparts. The policy snapshot has SHA-256 `e20e4b121d22731626f9215e6b1529b084623f8d2eb34d38ac600264ec507130`. [Recorded callbacks](omission-review-callbacks-linux.tsv)

## 1. Atomic-group disassembly for `(?>ab?)`

`test.test_re.DebugTests.test_atomic_group` · occurrence 1 · class-level `cpython_only`

**Proposed decision.** Omit the exact SRE listing. Retain the public commitments to display compilation diagnostics and to honor atomic grouping.

**Effect on programs.** The expected text exposes the `ATOMIC_GROUP` target at 17 and the nested `REPEAT_ONE` target at 16. A program comparing stdout to that text or reading its addresses will see a difference. Imre displays the backend and compiled pattern. A caller matching an atomic group can rely on its choice remaining committed once the group exits.

**Paired coverage.** `test.test_re.ReTests.test_atomic_grouping`. The paired parent compiles `a(?>bc|b)c`. It rejects `abc` because the group cannot retry `b` after committing to `bc`, and it accepts `abcc`. A separate DEBUG control compiled the original `(?>ab?)`, verified two repeatable nonempty listings and checked its full-match results. Only the guarded original compares the entire CPython instruction listing. [Atomic-group semantics](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Doc/library/re.rst#L398-L410) · [Paired atomic matches](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2567-L2579)

**Unchanged original.** Stock emitted `addSuccess`. O0 and O3 each emitted `addSkip` with `implementation detail specific to cpython`. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2772-L2787)

<details>
<summary>Literal pinned method</summary>

```python
    def test_atomic_group(self):
        self.assertEqual(get_debug_out(r'(?>ab?)'), '''\
ATOMIC_GROUP
  LITERAL 97
  MAX_REPEAT 0 1
    LITERAL 98

 0. INFO 4 0b0 1 2 (to 5)
 5: ATOMIC_GROUP 11 (to 17)
 7.   LITERAL 0x61 ('a')
 9.   REPEAT_ONE 6 0 1 (to 16)
13.     LITERAL 0x62 ('b')
15.     SUCCESS
16:   SUCCESS
17: SUCCESS
''')
```

</details>

## 3. Grouped possessive-repeat disassembly for `(?:ab)?+`

`test.test_re.DebugTests.test_possesive_repeat` · occurrence 1 · class-level `cpython_only`

**Proposed decision.** Omit the exact SRE listing for the two-character operand. Retain compilation diagnostics and the rule that the possessive optional group consumes at most one `ab` without giving it back.

**Effect on programs.** A consumer expecting `POSSESSIVE_REPEAT 7 0 1 (to 13)`, two literal instructions and two numbered `SUCCESS` lines will see Imre’s backend-specific listing. Matching code observes whether the group consumed one complete `ab` and whether the rest of the pattern can match.

**Paired coverage.** `test.test_re.ReTests.test_fullmatch_possessive_quantifiers`, `test.test_re.ReTests.test_findall_possessive_quantifiers`. The paired full-match parent checks the exact `(?:ab)?+` spelling against `ab` and `abc`, accepting only `ab`. The paired `findall` parent obtains `['ab', 'ab', '', '']` on `ababc`. A separate control checks repeated DEBUG output and matching for the literal pattern. Neither credited parent prescribes instruction addresses. [Possessive matching](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Doc/library/re.rst#L162-L177) · [Paired grouped full matches](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2543-L2554) · [Paired grouped findall](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2562-L2565)

**Unchanged original.** Stock emitted `addSuccess`. O0 and O3 each emitted `addSkip` with `implementation detail specific to cpython`. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2801-L2813)

<details>
<summary>Literal pinned method</summary>

```python
    def test_possesive_repeat(self):
        self.assertEqual(get_debug_out(r'(?:ab)?+'), '''\
POSSESSIVE_REPEAT 0 1
  LITERAL 97
  LITERAL 98

 0. INFO 4 0b0 0 2 (to 5)
 5: POSSESSIVE_REPEAT 7 0 1 (to 13)
 9.   LITERAL 0x61 ('a')
11.   LITERAL 0x62 ('b')
13: SUCCESS
14. SUCCESS
''')
```

</details>

## 4. Single-character possessive-repeat disassembly for `a?+`

`test.test_re.DebugTests.test_possesive_repeat_one` · occurrence 1 · class-level `cpython_only`

**Proposed decision.** Omit the exact SRE listing that selects a dedicated instruction for a single-character repeat. Retain compilation diagnostics and possessive optional matching for that character.

**Effect on programs.** A consumer checking for `POSSESSIVE_REPEAT_ONE 6 0 1 (to 12)` or its literal and `SUCCESS` offsets will see a different listing. Matching code can still distinguish a single consumed `a` from an empty match and observe the resulting match sequence.

**Paired coverage.** `test.test_re.ReTests.test_fullmatch_possessive_quantifiers`, `test.test_re.ReTests.test_findall_possessive_quantifiers`. The paired full-match parent accepts `a` with `a?+`, rejects `ab`, and accepts `ab` with `a?+b`. The paired `findall` parent obtains `['a', 'a', '', '']` with `a?+` on `aab`. A separate control checks repeated DEBUG output for `a?+`. These results leave the engine free to choose how it implements a one-character repeat. [Possessive matching](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Doc/library/re.rst#L162-L177) · [Paired single-character full matches](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2529-L2541) · [Paired single-character findall](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2556-L2560)

**Unchanged original.** Stock emitted `addSuccess`. O0 and O3 each emitted `addSkip` with `implementation detail specific to cpython`. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2789-L2799)

<details>
<summary>Literal pinned method</summary>

```python
    def test_possesive_repeat_one(self):
        self.assertEqual(get_debug_out(r'a?+'), '''\
POSSESSIVE_REPEAT 0 1
  LITERAL 97

 0. INFO 4 0b0 0 1 (to 5)
 5: POSSESSIVE_REPEAT_ONE 6 0 1 (to 12)
 9.   LITERAL 0x61 ('a')
11.   SUCCESS
12: SUCCESS
''')
```

</details>

## 7. Private ASCII and Unicode casing helpers

`test.test_re.ImplementationTest.test_case_helpers` · occurrence 1 · method-level `cpython_only`

**Proposed decision.** Omit the four callable `_sre` helper interfaces and their per-code-point results. Retain public `IGNORECASE` and ASCII matching.

**Effect on programs.** A program calling `_sre.ascii_tolower`, `unicode_tolower`, `ascii_iscased` or `unicode_iscased` directly depends on CPython’s private numeric API. A program using `re.I` or `re.I | re.A` observes which subjects match and which text is captured.

**Paired coverage.** `test.test_re.ReTests.test_ignore_case_set`. The paired set test uses U+10400 and U+10428, two code points also in the private loop. Cross-case matches succeed under `re.I` and fail under `re.I | re.A`. It checks regex membership. No public operation exposes every integer and Boolean in the loop or requires the private helper to return `ord('i')` for U+0130. [Mode selection in CPython's compiler](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/re/_compiler.py#L47-L57) · [Unicode and ASCII set matches](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L1149-L1173)

**Unchanged original.** Stock emitted `addSuccess`. O0 and O3 each emitted `addSkip` with `implementation detail specific to cpython`. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2961-L2986)

<details>
<summary>Literal pinned method</summary>

```python
    @cpython_only
    def test_case_helpers(self):
        import _sre
        for i in range(128):
            c = chr(i)
            lo = ord(c.lower())
            self.assertEqual(_sre.ascii_tolower(i), lo)
            self.assertEqual(_sre.unicode_tolower(i), lo)
            iscased = c in string.ascii_letters
            self.assertEqual(_sre.ascii_iscased(i), iscased)
            self.assertEqual(_sre.unicode_iscased(i), iscased)

        for i in list(range(128, 0x1000)) + [0x10400, 0x10428]:
            c = chr(i)
            self.assertEqual(_sre.ascii_tolower(i), i)
            if i != 0x0130:
                self.assertEqual(_sre.unicode_tolower(i), ord(c.lower()))
            iscased = c != c.lower() or c != c.upper()
            self.assertFalse(_sre.ascii_iscased(i))
            self.assertEqual(_sre.unicode_iscased(i),
                             c != c.lower() or c != c.upper())

        self.assertEqual(_sre.ascii_tolower(0x0130), 0x0130)
        self.assertEqual(_sre.unicode_tolower(0x0130), ord('i'))
        self.assertFalse(_sre.ascii_iscased(0x0130))
        self.assertTrue(_sre.unicode_iscased(0x0130))
```

</details>

## 12. Private literal-prefix overlap arrays

`test.test_re.ImplementationTest.test_overlap_table` · occurrence 1 · unguarded

**Proposed decision.** Omit the return lists of `re._compiler._generate_overlap_table`. Retain public search results, including recovery when an apparent literal prefix fails.

**Effect on programs.** Code that calls the helper to inspect a prefix gets a list of prefix lengths on CPython. On the measured native build it fails when reading `re._compiler`. A regex search can find the same match with an engine that never constructs these arrays.

**Paired coverage.** `test.test_re.ExternalTests.test_re_tests`. In the already paired ExternalTests parent, the actual fixture row ('abc', 'ababc', SUCCEED, 'found', 'abc') makes search abandon a partial prefix at the start and return 'abc' later in the subject. The parent compares `group(0)` to the expected text. This row does not prescribe any of the six private arrays, the engine's algorithm or its timing. [Definition of a prefix overlap](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/re/_compiler.py#L421-L440) · [Use in the compiled prefix](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/re/_compiler.py#L574-L582) · [Literal external fixture](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/re_tests.py#L120) · [Public search and comparison](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L3068-L3100) · [Paired external parent](https://github.com/astral-experiments/imre/blob/b5812df0227caa9d3e7d89e47bbdade93fd5ad67/benchmarks/cpython_stdlib/full/re/parent-ledger-linux.md#L131)

**Unchanged original.** Stock emitted `addSuccess`. O0 and O3 each emitted `addError` at the first attribute lookup with `AttributeError: module 're' has no attribute '_compiler'`. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2913-L2920)

<details>
<summary>Literal pinned method</summary>

```python
    def test_overlap_table(self):
        f = re._compiler._generate_overlap_table
        self.assertEqual(f(""), [])
        self.assertEqual(f("a"), [0])
        self.assertEqual(f("abcd"), [0, 0, 0, 0])
        self.assertEqual(f("aaaa"), [0, 1, 2, 3])
        self.assertEqual(f("ababba"), [0, 0, 1, 2, 0, 1])
        self.assertEqual(f("abcabdac"), [0, 0, 0, 1, 2, 0, 1, 0])
```

</details>

## 14. Signs of two private compiler constants

`test.test_re.ImplementationTest.test_signedness` · occurrence 1 · unguarded

**Proposed decision.** Omit the sign checks through `re._compiler.MAXREPEAT` and `re._compiler.MAXGROUPS`. Retain legal counts and user-visible limit errors.

**Effect on programs.** A program reading either private attribute on the measured native build fails when it looks up `re._compiler`. Public users observe whether compilation succeeds, how a group matches, or which exception a count or group reference produces.

**Paired coverage.** `test.test_re.ReTests.test_symbolic_groups`, `test.test_re.ReTests.test_re_groupref_overflow`, `test.test_re.ReTests.test_repeat_minmax_overflow`. The first two parents passed the fresh replay. One matches a 200-group pattern with span (0, 5). The other uses `re._constants.MAXGROUPS` to exercise public invalid-reference diagnostics at positions 3 and 10. The earlier paired repeat-overflow parent checks spans at 65,535 and 65,536 and compilation overflow for larger counts. These cover observable behavior. None reads the signs at the two private attribute paths. [Platform-dependent SRE limits](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Modules/_sre/sre.h#L16-L25) · [200-group match](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L282-L293) · [Template and pattern reference limits](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L702-L707) · [Historically paired repeat-boundary assertions](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2043-L2056)

**Unchanged original.** Stock emitted `addSuccess`. O0 and O3 each emitted `addError` at the first attribute lookup with `AttributeError: module 're' has no attribute '_compiler'`. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2922-L2924)

<details>
<summary>Literal pinned method</summary>

```python
    def test_signedness(self):
        self.assertGreaterEqual(re._compiler.MAXREPEAT, 0)
        self.assertGreaterEqual(re._compiler.MAXGROUPS, 0)
```

</details>

## 15. Private replacement-template token-list validation

`test.test_re.ImplementationTest.test_sre_template_invalid_group_index` · occurrence 1 · method-level `cpython_only`

**Proposed decision.** Omit the private `_sre.template` constructor's two error-message assertions. Retain parsing of public replacement templates, reference diagnostics and `Match.expand` results.

**Effect on programs.** Code passing a hand-built token list through `_sre.template` on CPython can distinguish the two `TypeError` messages. For a public text or bytes replacement, `re.sub` and `Match.expand` parse the caller's template. Their API exposes no slot for an arbitrary tuple or negative integer in CPython's internal token list.

**Paired coverage.** `test.test_re.ReTests.test_expand`, `test.test_re.ReTests.test_sub_template_numeric_escape`. The paired expansion test checks named and numbered captures and verifies that two references to an unmatched group contribute empty strings. The paired replacement test checks invalid numeric references in public `re.sub`, including the recorded error message and position. Neither public test passes malformed token lists to the private constructor. [Public replacement parsing before construction](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/re/__init__.py#L374-L377) · [Private token layout and slot validation](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Modules/_sre/sre.c#L1719-L1767) · [Named, numbered and unmatched groups](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L728-L736) · [Invalid public replacement references](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L219-L231)

**Unchanged original.** Stock emitted `addSuccess`. O0 and O3 each emitted `addSkip` with `implementation detail specific to cpython`. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L3021-L3030)

<details>
<summary>Literal pinned method</summary>

```python
    @cpython_only
    def test_sre_template_invalid_group_index(self):
        # see gh-106524
        import _sre
        with self.assertRaises(TypeError) as cm:
            _sre.template("", ["", -1, ""])
        self.assertIn("invalid template", str(cm.exception))
        with self.assertRaises(TypeError) as cm:
            _sre.template("", ["", (), ""])
        self.assertIn("an integer is required", str(cm.exception))
```

</details>

## Remaining ordinary-profile work

`ImplementationTest.test_deprecated_modules` remains an unguarded compatibility gap. Stock passed all three import subtests and the parent. O0 and O3 each errored on all three imports before reaching the warning or export assertions. The one-parent processes exited 0, 1 and 1 for stock, O0 and O3. The original checks the warning text and caller filename, entries in `sys.modules`, module metadata and symbols including `SubPattern` and `_compile_info`. CPython’s three deprecated shims re-export the non-dunder contents of its regex parser, compiler and constants. At inspected public Imre `b5812df0`, the regex modules bundled in the compiler registry are `re` and its partial `re._constants`. A faithful import needs usable parser and compiler interfaces behind those exports. [Original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2934-L2959) · [Parser shim](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/sre_parse.py#L1-L7) · [Compiler shim](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/sre_compile.py#L1-L7) · [Constants shim](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/sre_constants.py#L1-L7) · [Native registry](https://github.com/astral-experiments/imre/blob/b5812df0227caa9d3e7d89e47bbdade93fd5ad67/crates/imre-compiler/src/modules.rs#L182-L186)

Of the other ten residual originals, five have CPython guards (`test_debug_flag`, `test_dealloc`, `test_disallow_instantiation`, `test_immutable` and `test_repeat_minmax_overflow_maxrepeat`). The public DEBUG and scanner controls have separate results. `test_locale_caching` and `test_locale_compiled` require provisioned locale data, and `test_memory_leaks` skips on ordinary stock and native builds because it requires debug fault injection. `test_look_behind_overflow` retains the recorded native NFA construction errors. `test_regression_gh94675` retains the native multiprocessing skip. The stress and process workloads remain held under the previous approval decisions. [Previous intact run](parent-ledger-linux.md) · [Scanner controls](https://github.com/astral-experiments/imre/pull/4520) · [Provisioned locale tests](https://github.com/astral-experiments/imre/pull/4529)

One historical paired counterpart needs a qualification. The bounded `ReTests.test_repeat_minmax` replay passed stock but failed its `x{2,1}` subtest at both native levels on `60635395`. Imre reported `error parsing pattern 0`. CPython expects `min repeat greater than max repeat` at position 2. That public diagnostic lies outside the 18 residual keys. Its historical cumulative credit and this failed replay are recorded separately. [Exact assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L771-L772)
