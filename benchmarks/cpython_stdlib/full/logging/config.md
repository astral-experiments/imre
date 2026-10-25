# Logger configuration

`native_config.py` runs four complete CPython 3.14.5 methods under the shared
unittest fixture runner. The two `ConfigDictTest` methods pass their original
dictionary to the Imre executable as JSON; the two `ConfigFileTest` methods
pass their real filename. Imre opens the file, validates configuration and
raises the exception. CPython runs the body, assertions and fixture hooks.
The runner also checks the precise native exception type and message: the
two original dictionary tests accept any exception.

`config_values.py` separately compares a single process against CPython,
including repeated dictionary and INI configuration, mutable configuration
passed by reference, custom levels, parent inheritance, propagation, and
disabling and re-enabling an existing logger. This comparison adds no
upstream test IDs.

This first implementation accepts logger and root settings and UTF-8 or
Latin-1 INI paths. Constructing configured handlers, filters, formatters and
custom factories remains open. These are module implementation gaps, not
transport or platform exclusions.
