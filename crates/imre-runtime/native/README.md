The errno registration order, Winsock fallbacks, and module documentation follow
CPython v3.14.5 (commit 5607950ef232dad16d75c0cf53101d9649d89115,
Modules/errnomodule.c). The original license is in PSF-LICENSE. The entries
contain symbolic names only. build.rs compiles a table against the target's
actual errno headers so an absent platform constant is absent in the module.

The builtins module documentation comes from Python/bltinmodule.c in the same
CPython release.

The raw Unicode decomposition mappings use Unicode 16.0.0, the database in
CPython 3.14. Regenerate them with
`python3 scripts/generate_unicode_decomposition.py UnicodeData.txt` and the
[pinned UnicodeData.txt](https://raw.githubusercontent.com/unicode-org/icu/8eca245c7484ac6cc179e3e5f7c1ea7680810f39/icu4c/source/data/unidata/UnicodeData.txt).
The runtime computes the Hangul syllables defined algorithmically by the
standard. The Unicode data license is in UNICODE-LICENSE.

The IDNA Nameprep table comes from CPython 3.14.5's `stringprep` module
and its Unicode 3.2 database. It stores prohibited and bidirectional
character ranges, plus the mappings that differ from Unicode 16 lowercase
and compatibility decomposition. Regenerate it with
`python3.14 scripts/generate_idna_nameprep.py` using that CPython release.
