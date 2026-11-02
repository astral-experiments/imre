# CTextIOWrapperTest registration and outcomes

On Linux, CPython 3.14.5 (5607950ef232dad16d75c0cf53101d9649d89115) registered all 667 cases in the unmodified test.test_io module. All 82 CTextIOWrapperTest originals passed at module ordinals 445–526, occurrence 1. The TSV identifies each original. The archive includes the source hashes, callback and provider evidence, per-mode build and run status, and controls.

On public Imre commit 5472a33cc16130407cc906daf38d3ce50332ea88, the matched compiler and runtime produced native executables at O0 and O3. Both encountered the missing io.BufferedRWPair at test_io.py line 2406 while importing the module. Neither reached registration. Native credits for this class are zero at this revision.

Verify the two files with `sha256sum -c SHA256SUMS`. The packet includes its source verification and replay scripts.

## Reviewed iterator change

Imre PR #3928 was reviewed at 736c6a539c4ff6c3be604df412a3bde8ddafc384. The telling and slot archives contain separately launched CPython, native O0 and native O3 controls. The real-file positive passed and the inverted assertion failed in all three lanes on this head. Cross-chunk raw and logical position pairs were (7, 6), (6, 5) and (8, 6). The finalizer and tempfile families are outside this evidence set.

Two boundary differences remain in the receipts. With an empty saved snapshot after an absolute seek, CPython's release build returns 8 for one six-byte file, whereas Imre returns the mapped byte position 3. An inconsistent custom buffer returning the minimum signed 64-bit offset triggers a checked OverflowError in Imre. Supplemental controls do not add original IDs.
