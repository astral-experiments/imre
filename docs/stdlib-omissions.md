# Standard library test omissions

The compatibility table keeps every original test in its upstream denominator. Each accepted omission identifies one occurrence and the behavior Imre can reasonably leave unsupported. Paired passes and observed skips keep their original meanings.

To add a decision, cite the assertion in the pinned CPython source, explain its impact on Python programs, and identify the public behavior that remains in scope. A failure, timeout or skip is evidence for review. The decision is made per original test.

## Accepted omissions

### `collections`

These six OrderedDict originals passed on stock and skipped at both native levels in the unchanged 295-case file. [Tested Imre revision](https://github.com/astral-experiments/imre/commit/b5af3ae9c19865f07fc3910132390a7ff1f32828)

The unmodified namedtuple docstring-sharing test passed on stock and took its upstream CPython-only skip at native O0 and O3. Its companion test for field values and independent edits passed in all three runs.

The deque size original passed on stock and reported the upstream CPython-specific skip on both native runs. [Tested Imre revision](https://github.com/astral-experiments/imre/commit/2a0ad34695006d693080ed35d3dadf8836a73427)

| Original CPython test | Occurrence | Decision and reason | Related paired original |
| --- | ---: | --- | --- |
| `test.test_collections.TestNamedTuple.test_field_doc_reuse` | 1 | The test checks that independently generated namedtuple classes share the same field docstring objects. CPython interns those strings to save memory. Imre preserves their values and allows each class to edit its field documentation independently. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_collections.py#L435-L442) | `TestNamedTuple.test_field_doc` |
| `test.test_deque.TestBasic.test_sizeof` | 1 | The test fixes the exact byte counts of CPython’s C deque header, 16 free-block slots and 64-pointer blocks, including the allocation boundary after 42 blocks. Imre can use a different layout while preserving deque behavior. Bounded eviction and ordering remain in scope. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_deque.py#L747-L759) | `TestBasic.test_maxlen` |
| `test.test_ordered_dict.CPythonOrderedDictSubclassTests.test_ordered_dict_items_result_gc` | 1 | The subclass inherits a test of the C iterator's recycled tuple. Subclass item iteration preserves the key and value without prescribing when the collector tracks the result. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L733-L742) | `CPythonOrderedDictSubclassTests.test_views` |
| `test.test_ordered_dict.CPythonOrderedDictSubclassTests.test_sizeof_exact` | 1 | This subclass inherits a byte-count formula for CPython's C object, table entries, nodes and iterators. A subclass can report a different footprint and still implement the sizing protocol. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L887-L917) | `CPythonOrderedDictSubclassTests.test_sizeof` |
| `test.test_ordered_dict.CPythonOrderedDictTests.test_ordered_dict_items_result_gc` | 1 | The assertion inspects whether the collector tracks an item tuple after CPython recycles it. An iterator can return the same key and value using a fresh tuple. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L733-L742) | `CPythonOrderedDictTests.test_views` |
| `test.test_ordered_dict.CPythonOrderedDictTests.test_sizeof_exact` | 1 | The expected bytes come from CPython's C structs, pointer widths, hash-table allocation and node sizes. Imre can report the size of its own representation. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L887-L917) | `CPythonOrderedDictTests.test_sizeof` |
| `test.test_ordered_dict.PurePythonOrderedDictSubclassTests.test_ordered_dict_items_result_gc` | 1 | The pure-Python subclass checks the collector's tracking bit after its first item. A different tuple lifetime strategy can satisfy the subclass iteration contract. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L733-L742) | `PurePythonOrderedDictSubclassTests.test_views` |
| `test.test_ordered_dict.PurePythonOrderedDictTests.test_ordered_dict_items_result_gc` | 1 | The pure-Python provider runs the same tuple-tracking assertion. The required item values are observable regardless of the collector's tracking policy. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L733-L742) | `PurePythonOrderedDictTests.test_views` |

### `io`

These two originals come from the SizeofTest mixin on CBufferedWriterTest. Stock passed both. The ordinary native runs reported the upstream CPython-specific skip at both optimization levels. [Tested Imre revision](https://github.com/astral-experiments/imre/commit/daf27ebdd244b9654a3183ab822fef2be8cf4c80)

| Original CPython test | Occurrence | Decision and reason | Related paired original |
| --- | ---: | --- | --- |
| `test.test_io.CBufferedWriterTest.test_buffer_freeing` | 1 | The test subtracts 4,096 bytes from the open writer’s reported size and expects that exact size after close. Imre must close the stream and release its resources. Its allocator can report a different change in `sys.getsizeof()`. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L1460-L1466) | `CBufferedWriterTest.test_context_manager` |
| `test.test_io.CBufferedWriterTest.test_sizeof` | 1 | The assertion requires a writer with an 8,192-byte buffer to report exactly 4,096 bytes more than one with a 4,096-byte buffer. Imre supports the requested buffer capacity and can account for object storage differently. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L1449-L1457) | `CBufferedWriterTest.test_constructor` |

### `pathlib`

Stock passed the original. Both native runs reported its upstream CPython-specific skip and exited successfully. [Pinned startup helper](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/support/import_helper.py#L316-L335) · [Tested Imre revision](https://github.com/astral-experiments/imre/commit/dd3180cce8c6b613d4c14aea25ef3b633b85635b)

| Original CPython test | Occurrence | Decision and reason | Related paired original |
| --- | ---: | --- | --- |
| `test.test_pathlib.test_pathlib.LazyImportTest.test_lazy_import` | 1 | A fresh interpreter started with `-S` must keep shutil out of `sys.modules` before and after importing pathlib. That assertion fixes CPython’s startup import order. Imre may load shutil earlier while providing the same path and file operations. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_pathlib/test_pathlib.py#L86-L87) | `PathSubclassTest.test_copy_error_handling` |

### `tempfile`

Stock passed both originals. The ordinary and boxed native runs reported the upstream CPython-specific skip at both optimization levels. The related cleanup originals passed in all four native runs. [Tested Imre revision](https://github.com/astral-experiments/imre/commit/0d7959f15485a6cda6da1f73c4a8ef873d506d6a)

| Original CPython test | Occurrence | Decision and reason | Related paired original |
| --- | ---: | --- | --- |
| `test.test_tempfile.TestTemporaryDirectory.test_del_on_collection` | 1 | This test deletes the last reference and immediately requires the directory to be gone. Its source explicitly relies on CPython reference counting. Imre supports finalization when collection runs. The paired warning test requests collection and verifies that the directory disappears. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_tempfile.py#L1846-L1857) | `TestTemporaryDirectory.test_warnings_on_cleanup` |
| `test.test_tempfile.TestTemporaryDirectory.test_del_on_collection_ignore_errors` | 1 | This test deletes the last reference while a file is open and immediately checks the directory contents and platform-specific unlink result. The paired explicit-cleanup test exercises those same open-file and error-handling rules. Only the timing of finalization after deleting the reference is omitted. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_tempfile.py#L1859-L1878) | `TestTemporaryDirectory.test_explicit_cleanup_ignore_errors` |

## Reviewed cases still in scope

### `collections`

Both originals passed on stock and skipped at both native levels in the unchanged OrderedDict file. Each exercises a failure that a Python program can observe.

| Original CPython test | Occurrence | Decision and reason | Related paired original |
| --- | ---: | --- | --- |
| `test.test_ordered_dict.CPythonOrderedDictSubclassTests.test_weakref_list_is_not_traversed` | 1 | A subclass, a self-referential list and a weak reference go out of scope before collection. A crash would affect ordinary subclass users. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L954-L971) | `CPythonOrderedDictSubclassTests.test_reference_loop` |
| `test.test_ordered_dict.CPythonOrderedDictTests.test_weakref_list_is_not_traversed` | 1 | The body creates a reference cycle containing a weak reference and calls the public collector. A crash in that sequence is observable in an ordinary program. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ordered_dict.py#L954-L971) | `CPythonOrderedDictTests.test_reference_loop` |

### `io`

Each original below is occurrence 1 in the pinned I/O inventory. A private fixture detail can expose a stream behavior that applications rely on. These seven decisions add no omissions.

| Original CPython test | Occurrence | Reason to keep it in scope |
| --- | ---: | --- |
| `test.test_io.StatefulIncrementalDecoderTest.test_decoder` | 1 | The test decoder buffers incomplete input until the next decode call and flushes it at end of input. The text-stream tests depend on this fixture behaving consistently across calls. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L2815-L2824) |
| `test.test_io.CTextIOWrapperTest.test_illegal_decoder` | 1 | A decoder can return bytes where a text stream requires strings, or supply invalid state. Reading must raise a catchable TypeError. The private codec flag lets the fixture reach these public read operations. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L3747-L3785) |
| `test.test_io.CTextIOWrapperTest.test_illegal_encoder` | 1 | Writing text must raise TypeError if the codec returns an object the binary stream cannot accept. The test changes a private flag to reach the write operation. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L3739-L3745) |
| `test.test_io.CTextIOWrapperTest.test_seek_and_tell` | 1 | A file opened with a registered stateful encoding must return the same text after seeking to a position saved by tell(). The test varies the buffer boundary and treats the saved position as opaque. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L3278-L3323) |
| `test.test_io.PyTextIOWrapperTest.test_illegal_decoder` | 1 | A decoder can return bytes where a text stream requires strings, or supply invalid state. Reading must raise a catchable TypeError. The private codec flag lets the fixture reach these public read operations. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L3747-L3785) |
| `test.test_io.PyTextIOWrapperTest.test_illegal_encoder` | 1 | Writing text must raise TypeError if the codec returns an object the binary stream cannot accept. The test changes a private flag to reach the write operation. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L3739-L3745) |
| `test.test_io.PyTextIOWrapperTest.test_seek_and_tell` | 1 | A file opened with a registered stateful encoding must return the same text after seeking to a position saved by tell(). The test varies the buffer boundary and treats the saved position as opaque. [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_io.py#L3278-L3323) |

### `os`

Stock passed the unchanged original. The native observations below come from separate filesystem controls. The unchanged native original has yet to run.

| Original CPython test | Occurrence | Decision and reason | Related paired original |
| --- | ---: | --- | --- |
| `test.test_os.FileTests.test_rename` | 1 | The original requires an invalid destination to raise `TypeError` and checks that CPython’s reference count stays constant. Separate boxed controls verified the exception and the source file’s contents. We retain the mixed original while its ordinary compiled error path remains unverified. [Upstream assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_os.py#L215-L220) | `TestPosixDirFd.test_rename_dir_fd` |
