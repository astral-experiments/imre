# gettext

Click 8.4.2 imports `gettext` and `ngettext` at `click/core.py` lines 18 and 19. On CPython 3.14.5, constructing Click's `Path` objects while Flask starts translates `file` three times and `path` twice. The ten HTTP requests in the Flask gate make no calls to either function. A separate positive control uses Click's complete `_format_possibilities` with translated singular and plural suggestions.

The pinned Linux suite has 73 occurrences. Eighteen unchanged plural-rule tests run their complete bodies, assertions and default `TestCase` fixtures in Imre at O0 and O3. Six more run the original domain functions, test bodies and assertions in Imre while CPython sets up the catalog files, working directory, environment and test instance. Five separate controls translate the expected text using domain functions in place of an installed alias or a bound `GNUTranslations` method. Their ledger entries record the missing original bindings. The remaining rows retain the compiler or fixture diagnostic, or the reference result when no native body ran.

`originals.py` verifies the byte identity of the discovered test file and exercises every passing row of the Linux ledger. Run it from the repository root with the pinned CPython interpreter and matching Imre compiler and runtime archive.

```sh
$CPYTHON -B benchmarks/cpython_stdlib/full/gettext/originals.py \
    --compiler "$IMRE" --runtime "$IMRE_RUNTIME" --clang "$CLANG"
```

The original file, including its embedded GNU `.mo` catalogs, and its PSF license are in [upstream](upstream/). The application fixture checks locale aliases, fallback languages, three plural forms, contextual lookup, malformed expressions, short-circuiting and source-text fallback. The Click calls use integer plural counts. [Application fixture](../../../../tests/fixtures/text/gettext_catalog.py) · [Occurrence roles and diagnostics](evidence-linux.json)
