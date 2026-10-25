# Handler I/O locks

The pinned CPython source creates a reentrant lock from `Handler.__init__` through
the overridable `createLock()`. `NullHandler.createLock` sets its lock to
`None`. Both handlers expose this distinction through their public operations.

`null_handler.py` compares `lock_values.py` with CPython 3.14.5 at O0 and O3.
Its 35 outputs cover recursive acquisition and release, identity before and after lock replacement, early
return and exceptional exit, timeouts, explicit context methods, setting a lock to
`None`, and direct post-fork reinitialization. A separate runtime test exercises
the lock from two OS threads: non-owner release, timeout, and the release that
allows a waiter to acquire it. Neither driver runs a complete upstream test case.

The registration of handlers with the process-wide CPython fork hooks and the
unchanged fork test with its original process and thread fixtures are separate.
A call to `Handler._at_fork_reinit()` is not evidence for that test.
