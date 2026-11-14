//! Loads a closed graph of first-party modules and orders their initialization.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use ruff_db::parsed::ParsedModuleRef;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::analysis::{CheckedDiagnostic, FallbackCalls, Session};
use crate::frontend::phase::Diagnostic;
use crate::ir::{
    Builtin, CopyOperation, FnmatchOperation, PosixPathOperation, SysEncoding, UnicodeProperty,
    WrapperAttributes,
};
use crate::watch::FileSystem;

mod boxed_exports;
mod declarations;
pub(crate) mod doctest;
mod empty_keywords;
mod entry_profile;
#[cfg(test)]
mod module_entry_tests;
mod namedtuple_fields;
mod ordered_imports;
mod os;
pub(crate) use os::credential_arity;
mod reachability;
pub(crate) use reachability::{definition_effect, http_command_key, inert_literal};
mod type_aliases;

pub(crate) type ModuleId = usize;

pub(crate) fn valid_module_operand(name: &str) -> bool {
    name.split('.').all(|part| {
        !part.is_empty() && Path::new(part).file_name().is_some_and(|file| file == part)
    })
}

/// Return the package used to resolve imports originating in this module.
pub(crate) fn import_package(module: &str, is_package: bool) -> &str {
    if is_package {
        module
    } else {
        module.rsplit_once('.').map_or("", |(parent, _)| parent)
    }
}

/// Resolve an import against its containing package, counting the leading dots in `level`.
pub(crate) fn resolve_import(package: &str, level: u32, name: &str) -> Option<String> {
    if level == 0 {
        return Some(name.to_owned());
    }
    if package.is_empty() {
        return None;
    }
    let mut base = package;
    for _ in 1..level {
        base = base.rsplit_once('.')?.0;
    }
    Some(if name.is_empty() {
        base.to_owned()
    } else {
        format!("{base}.{name}")
    })
}

/// Mode constants and operations from the target's POSIX stat provider.
fn stat_mode_source() -> Option<&'static str> {
    macro_rules! modes {
        ($platform:literal) => {
            concat!(
                include_str!($platform),
                "\n",
                include_str!("../stdlib/stat/common.py")
            )
        };
    }
    match std::env::consts::OS {
        "linux" => Some(modes!("../stdlib/stat/linux.py")),
        "macos" => Some(modes!("../stdlib/stat/darwin.py")),
        _ => None,
    }
}

/// Borrow a packaged input only when it has the fixed registration's physical origin.
macro_rules! bundled_source {
    ($path:tt, from $package:tt) => {
        $crate::zip_entry::packaged_fixed!(($path $package), Some((
            $path,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../benchmarks/cpython_stdlib/full/", $package, "/upstream/Lib/", $path
            )),
        )))
    };
    ($path:tt) => {
        $crate::zip_entry::packaged_fixed!(($path), Some((
            $path,
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/stdlib/", $path)),
        )))
    };
}
pub(crate) use bundled_source;

/// Module paths using the same single-byte encoding provider.
pub(crate) fn charmap_provider_path(name: &str, boxed: bool) -> Option<&'static str> {
    Some(match name {
        "encodings.cp1251" => "encodings/cp1251.py",
        "encodings.cp1252" => "encodings/cp1252.py",
        "encodings.cp424" => "encodings/cp424.py",
        "encodings.cp437" => "encodings/cp437.py",
        // Keep these codec modules on the object importer. Exposing their source
        // to the checked graph would send literal imports through boxed evaluation.
        "encodings.iso8859_2" if boxed => "encodings/iso8859_2.py",
        "encodings.koi8_r" if boxed => "encodings/koi8_r.py",
        "encodings.iso8859_15" => "encodings/iso8859_15.py",
        "encodings.koi8_u" => "encodings/koi8_u.py",
        _ => return None,
    })
}

fn bundled_package(name: &str) -> bool {
    bundled_runtime_source(name).is_some_and(|(file, _)| file.ends_with("/__init__.py"))
}

fn sealed_package_child(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(parent, _)| {
        IntrinsicModule::named(parent).is_some() && bundled_package(parent)
    })
}

/// Python source modules bundled for runtime imports.
pub(crate) fn bundled_runtime_source(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "bisect" => bundled_source!("bisect.py"),
        "unittest.main" => bundled_source!("unittest/main.py"),
        "unittest.result" => bundled_source!("unittest/result.py"),
        "unittest.runner" => bundled_source!("unittest/runner.py"),
        "unittest.suite" => bundled_source!("unittest/suite.py"),
        "unittest.signals" => bundled_source!("unittest/signals.py"),
        // CPython 3.14.5 Lib/unittest/_log.py. See stdlib/docstrings/LICENSE.
        "unittest._log" => bundled_source!("unittest/_log.py"),
        // CPython 3.14.5 Lib/unittest/util.py. See stdlib/docstrings/LICENSE.
        "unittest.util" => bundled_source!("unittest/util.py"),
        // CPython 3.14.5 Lib/unittest/async_case.py. See licenses/CPython.txt.
        "unittest.async_case" => bundled_source!("unittest/async_case.py"),
        "unittest.case" => Some((
            "unittest/case.py",
            include_str!("../stdlib/unittest/boxed_case.py"),
        )),
        "pickle" => bundled_source!("pickle.py"),
        // CPython 3.14.5 Lib/_colorize.py. See licenses/CPython.txt.
        "_colorize" => bundled_source!("_colorize.py"),
        // CPython 3.14.5 Lib/_pyio.py. See licenses/CPython.txt.
        "_pyio" => bundled_source!("_pyio.py"),
        // CPython 3.14.5 Lib/_strptime.py and its calendar dependency. See licenses/CPython.txt.
        "_strptime" => bundled_source!("_strptime.py"),
        "calendar" => bundled_source!("calendar.py"),
        "html" => bundled_source!("html/__init__.py"),
        "html.entities" => bundled_source!("html/entities.py"),
        // CPython 3.14.5's file wrapper and shared stream reader. See licenses/CPython.txt.
        "lzma" => bundled_source!("lzma.py"),
        "compression" => bundled_source!("compression/__init__.py"),
        "compression.zstd" => bundled_source!("compression/zstd/__init__.py"),
        "compression.zstd._zstdfile" => bundled_source!("compression/zstd/_zstdfile.py"),
        "compression._common" => bundled_source!("compression/_common/__init__.py"),
        "compression._common._streams" => bundled_source!("compression/_common/_streams.py"),
        // CPython 3.14.5 Lib/zoneinfo. See licenses/CPython.txt.
        "zoneinfo" => bundled_source!("zoneinfo/__init__.py"),
        "zoneinfo._common" => bundled_source!("zoneinfo/_common.py"),
        "zoneinfo._tzpath" => bundled_source!("zoneinfo/_tzpath.py"),
        "zoneinfo._zoneinfo" => bundled_source!("zoneinfo/_zoneinfo.py"),
        // CPython 3.14.5 Lib/fractions.py and its numeric ABCs. See licenses/CPython.txt.
        "fractions" => bundled_source!("fractions.py"),
        "ipaddress" => bundled_source!("ipaddress.py"),
        "numbers" => bundled_source!("numbers.py"),
        "statistics" => bundled_source!("statistics.py"),
        // CPython 3.14.5 Lib/wave.py. See licenses/CPython.txt.
        "wave" => bundled_source!("wave.py"),
        "_compat_pickle" => bundled_source!("_compat_pickle.py"),
        "_tokenize" => bundled_source!("_tokenize.py"),
        "__future__" => bundled_source!("__future__.py"),
        "__hello__" => bundled_source!("__hello__.py"),
        "ast" => bundled_source!("ast.py"),
        "_ast_unparse" => bundled_source!("_ast_unparse.py"),
        "stat" => {
            static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let modes = stat_mode_source()?;
            Some((
                "stat.py",
                SOURCE.get_or_init(|| {
                    format!("{modes}\n{}", include_str!("../stdlib/stat/boxed.py"))
                }),
            ))
        }
        "zipimport" if cfg!(unix) => Some((
            "zipimport.py",
            concat!(
                include_str!("../stdlib/zipimport_boxed.py"),
                "\n",
                include_str!("../stdlib/zipimport.py"),
                "\n",
                include_str!("../stdlib/zipimport_source.py"),
            ),
        )),
        "importlib.metadata._files" => Some((
            "importlib/metadata/_files.py",
            include_str!("../stdlib/importlib/metadata_files.py"),
        )),
        "importlib.metadata._adapters" => Some((
            "importlib/metadata/_adapters.py",
            include_str!("../stdlib/importlib/metadata_adapters.py"),
        )),
        "importlib.metadata" => Some((
            "importlib/metadata/__init__.py",
            concat!(
                include_str!("../stdlib/importlib/metadata_boxed.py"),
                "\n",
                include_str!("../stdlib/importlib/metadata_version.py"),
                "\n",
                include_str!("../stdlib/importlib/metadata_entrypoints.py"),
            ),
        )),
        "queue" => bundled_source!("queue.py"),
        "csv" => Some((
            "csv.py",
            concat!(
                include_str!("../stdlib/csv/dialect.py"),
                "\n",
                include_str!("../stdlib/csv/writer.py"),
                "\n",
                include_str!("../stdlib/csv/reader.py"),
                "\n",
                include_str!("../stdlib/csv/dict.py"),
                "\n",
                include_str!("../stdlib/csv/sniffer.py"),
            ),
        )),
        "copy" => Some((
            "copy.py",
            include_str!("../../../benchmarks/cpython_stdlib/full/copy/upstream/Lib/copy.py"),
        )),
        "copyreg" => bundled_source!("copyreg.py"),
        "dis" => bundled_source!("dis.py"),
        "opcode" => bundled_source!("opcode.py"),
        "_opcode_metadata" => bundled_source!("_opcode_metadata.py"),
        "tracemalloc" => bundled_source!("tracemalloc.py"),
        "warnings" => Some((
            "warnings.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/warnings/upstream/Lib/warnings.py"
            ),
        )),
        "_py_warnings" => bundled_source!("_py_warnings.py"),
        "re" => bundled_source!("re/__init__.py"),
        "re._constants" => bundled_source!("re/_constants.py"),
        "decimal" => bundled_source!("decimal.py"),
        "_imre_decimal_ratio" => bundled_source!("_imre_decimal_ratio.py"),
        // CPython 3.14.5 Lib/py_compile.py. See stdlib/docstrings/LICENSE.
        "py_compile" => bundled_source!("py_compile.py"),
        "pyexpat" => bundled_source!("pyexpat.py"),
        "xml" => bundled_source!("xml/__init__.py"),
        "xml.etree" => bundled_source!("xml/etree/__init__.py"),
        "xml.etree.ElementPath" => bundled_source!("xml/etree/ElementPath.py"),
        "xml.etree.ElementTree" => bundled_source!("xml/etree/ElementTree.py"),
        "xml.parsers" => bundled_source!("xml/parsers/__init__.py"),
        "xml.parsers.expat" => bundled_source!("xml/parsers/expat.py"),
        // CPython 3.14.5 Lib/pprint.py. See stdlib/docstrings/LICENSE.
        "pprint" => bundled_source!("pprint.py"),
        "difflib" => Some((
            "difflib.py",
            include_str!("../../../benchmarks/cpython_stdlib/full/difflib/upstream/Lib/difflib.py"),
        )),
        "heapq" => Some((
            "heapq.py",
            include_str!("../../../benchmarks/cpython_stdlib/full/heapq/upstream/Lib/heapq.py"),
        )),
        "importlib" => Some((
            "importlib/__init__.py",
            concat!(
                include_str!(
                    "../../../benchmarks/cpython_stdlib/full/importlib/upstream/Lib/importlib/__init__.py"
                ),
                "\n",
                include_str!("../stdlib/importlib/native_specs.py"),
            ),
        )),
        "os" => bundled_source!("os.py"),
        "genericpath" => bundled_source!("genericpath.py"),
        "posixpath" => bundled_source!("posixpath.py"),
        "pkgutil" => Some((
            "pkgutil.py",
            concat!(
                include_str!("../stdlib/pkgutil/boxed.py"),
                "\n",
                include_str!("../stdlib/pkgutil/iter_modules.py"),
            ),
        )),
        "_collections_abc" => bundled_source!("_collections_abc.py"),
        "unittest.mock" => Some((
            "unittest/mock.py",
            concat!(
                include_str!("../stdlib/unittest/mock.py"),
                "\n",
                include_str!("../stdlib/unittest/mock_calls.py"),
            ),
        )),
        // CPython 3.14.5 Lib/weakref.py and Lib/_weakrefset.py.
        "weakref" => bundled_source!("weakref.py"),
        "_weakrefset" => bundled_source!("_weakrefset.py"),
        "abc" => bundled_source!("abc.py"),
        "quopri" => bundled_source!("quopri.py"),
        "base64" => bundled_source!("base64.py"),
        // CPython 3.14.5 Lib/code.py and Lib/codeop.py. See stdlib/docstrings/LICENSE.
        "code" => bundled_source!("code.py"),
        "codecs" => bundled_source!("codecs.py"),
        "encodings" => bundled_source!("encodings/__init__.py"),
        "encodings.aliases" => Some((
            "encodings/aliases.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/linecache/upstream/Lib/encodings/aliases.py"
            ),
        )),
        "encodings.ascii" => bundled_source!("encodings/ascii.py"),
        "encodings.latin_1" => bundled_source!("encodings/latin_1.py"),
        "encodings.utf_8" => bundled_source!("encodings/utf_8.py"),
        "encodings.utf_8_sig" => bundled_source!("encodings/utf_8_sig.py"),
        "encodings.utf_16" => Some((
            "encodings/utf_16.py",
            include_str!("../stdlib/encodings/utf_wide.py"),
        )),
        "encodings.utf_32" => Some((
            "encodings/utf_32.py",
            include_str!("../stdlib/encodings/utf_wide.py"),
        )),
        "encodings.idna" => bundled_source!("encodings/idna.py"),
        "encodings.euc_jp" => Some((
            "encodings/euc_jp.py",
            include_str!("../stdlib/encodings/multibyte.py"),
        )),
        "encodings.shift_jis" => Some((
            "encodings/shift_jis.py",
            include_str!("../stdlib/encodings/multibyte.py"),
        )),
        "codeop" => bundled_source!("codeop.py"),
        "keyword" => Some((
            "keyword.py",
            include_str!("../../../benchmarks/cpython_stdlib/full/keyword/upstream/Lib/keyword.py"),
        )),
        "token" => bundled_source!("token.py"),
        "tokenize" => Some((
            "tokenize.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/linecache/upstream/Lib/tokenize.py"
            ),
        )),
        "collections" => Some((
            "collections/__init__.py",
            concat!(
                include_str!("../stdlib/collections/__init__.py"),
                "\n",
                include_str!("../stdlib/collections/bootstrap.py"),
                "\n",
                include_str!("../stdlib/collections/recursive_repr.py"),
                "\n",
                include_str!("../stdlib/collections/containers.py"),
            ),
        )),
        // CPython 3.14.5 Lib/configparser.py.
        "configparser" => bundled_source!("configparser.py"),
        "contextlib" => Some((
            "contextlib.py",
            concat!(
                include_str!("../stdlib/contextlib.py"),
                "\n",
                include_str!("../stdlib/contextlib_abstract_async.py"),
                "\n",
                include_str!("../stdlib/contextlib_nullcontext.py"),
                "\n",
                "class _BaseExitStack:\n",
                include_str!("../stdlib/contextlib_exitstack_base.inc"),
                "\n",
                include_str!("../stdlib/contextlib_exitstack.py"),
                "\n",
                include_str!("../stdlib/contextlib_async_exitstack.py"),
                "\n",
                include_str!("../stdlib/contextlib_redirect.py"),
                "\n",
                include_str!("../stdlib/contextlib_chdir.py"),
                "\n",
                include_str!("../stdlib/contextlib_closing.py"),
                "\n",
                include_str!("../stdlib/contextlib_contextmanager.py"),
                "\n",
                include_str!("../stdlib/contextlib_suppress.py"),
                "\n",
                include_str!("../stdlib/contextlib_exports.py")
            ),
        )),
        "_imre_socketserver_poll" => Some((
            "_imre_socketserver_poll.py",
            if cfg!(any(target_os = "linux", target_os = "macos")) {
                include_str!("../stdlib/_socketserver_poll.py")
            } else {
                include_str!("../stdlib/_socketserver_no_poll.py")
            },
        )),
        "_imre_socketserver_reader" => Some((
            "_imre_socketserver_reader.py",
            include_str!("../stdlib/_socketserver_reader.py"),
        )),
        "inspect" => bundled_source!("inspect.py"),
        "dataclasses" => bundled_source!("dataclasses.py"),
        "itertools" => bundled_source!("itertools.py"),
        "signal" if cfg!(unix) => bundled_source!("signal.py"),
        "typing" => bundled_source!("typing.py"),
        "logging" => Some((
            "logging/__init__.py",
            concat!(
                include_str!("../stdlib/logging/records.py"),
                "\n",
                include_str!("../stdlib/logging/formatters.py"),
                "\n",
                include_str!("../stdlib/logging/boxed_handler.py"),
                "\n",
                include_str!("../stdlib/logging/stream.py.inc"),
                "\n",
                include_str!("../stdlib/logging/boxed_stream.py.inc"),
                "\n",
                include_str!("../stdlib/logging/boxed.py"),
            ),
        )),
        // CPython 3.14.5 Lib/logging/config.py.
        "logging.config" => bundled_source!("logging/config.py"),
        // CPython 3.14.5 Lib/logging/handlers.py.
        "logging.handlers" => Some((
            "logging/handlers.py",
            include_str!("../stdlib/logging/boxed_handlers.py"),
        )),
        "platform" => Some((
            "platform.py",
            concat!(
                include_str!("../stdlib/platform/version.py"),
                "\n",
                include_str!("../stdlib/platform/metadata.py"),
            ),
        )),
        // CPython 3.14.5 Lib/ftplib.py. See stdlib/docstrings/LICENSE.
        "ftplib" if cfg!(unix) => bundled_source!("ftplib.py"),
        "mimetypes" if cfg!(unix) => Some((
            "mimetypes.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/mimetypes/upstream/Lib/mimetypes.py"
            ),
        )),
        "socketserver" => Some((
            "socketserver.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/socketserver/upstream/Lib/socketserver.py"
            ),
        )),
        // CPython 3.14.5 Lib/string/__init__.py, blob eab5067c9b133ea1210efacd92eb5f69c2336236.
        "string" => Some((
            "string/__init__.py",
            include_str!("../stdlib/string/cpython.py"),
        )),
        "string.templatelib" => bundled_source!("string/templatelib.py"),
        "email" => bundled_source!("email/__init__.py"),
        "email._encoded_words" => bundled_source!("email/_encoded_words.py", from "email"),
        "email._header_value_parser" => {
            bundled_source!("email/_header_value_parser.py", from "email")
        }
        "email._parseaddr" => bundled_source!("email/_parseaddr.py", from "email"),
        "email._policybase" => bundled_source!("email/_policybase.py"),
        "email.base64mime" => bundled_source!("email/base64mime.py"),
        "email.charset" => bundled_source!("email/charset.py"),
        "email.contentmanager" => bundled_source!("email/contentmanager.py"),
        "email.encoders" => bundled_source!("email/encoders.py"),
        "email.errors" => bundled_source!("email/errors.py", from "email"),
        "email.feedparser" => bundled_source!("email/feedparser.py"),
        "email.generator" => bundled_source!("email/generator.py"),
        "email.header" => bundled_source!("email/header.py"),
        "email.headerregistry" => bundled_source!("email/headerregistry.py"),
        "email.iterators" => bundled_source!("email/iterators.py"),
        "email.message" => Some((
            "email/message.py",
            include_str!("../stdlib/email/boxed_message.py"),
        )),
        "email.parser" => bundled_source!("email/parser.py"),
        "email.policy" => bundled_source!("email/policy.py"),
        "email.quoprimime" => bundled_source!("email/quoprimime.py"),
        "email.utils" => bundled_source!("email/utils.py", from "email"),
        "http" => bundled_source!("http/__init__.py"),
        "http.client" => Some((
            "http/client.py",
            concat!(
                include_str!("../stdlib/http/client.py"),
                include_str!("../stdlib/http/boxed_header_binding.py"),
                include_str!(
                    "../../../benchmarks/cpython_stdlib/full/http/upstream/Lib/http/client.py"
                ),
                include_str!("../stdlib/http/boxed_headers.py"),
            ),
        )),
        "http.cookiejar" => Some((
            "http/cookiejar.py",
            include_str!(
                "../../../benchmarks/cpython_stdlib/full/http/upstream/Lib/http/cookiejar.py"
            ),
        )),
        "http.server" => bundled_source!("http/server.py"),
        "locale" => bundled_source!("locale.py"),
        "_imre_gettext_locale" => bundled_source!("_imre_gettext_locale.py"),
        // CPython 3.14.5 Lib/colorsys.py. See licenses/CPython.txt.
        "colorsys" => bundled_source!("colorsys.py"),
        "getopt" => bundled_source!("getopt.py"),
        // CPython 3.14.5 Lib/optparse.py. See licenses/Optik.txt and licenses/CPython.txt.
        "optparse" => bundled_source!("optparse.py"),
        "gettext" => bundled_source!("gettext.py"),
        "uuid" => bundled_source!("uuid.py"),
        _ => charmap_provider_path(name, false)
            .map(|path| (path, include_str!("../stdlib/encodings/charmap.py"))),
    }
}

// CPython's built-in/frozen lookup precedes first-party files. These internal and
// frozen test modules have no corresponding stubs in the pinned typeshed inventory.
const INTRINSIC_MODULES: &[&str] = &[
    "__main__",
    "__hello__",
    "__hello_alias__",
    "__hello_only__",
    "__phello__",
    "__phello_alias__",
    "_hmac",
    "_suggestions",
    "_sysconfig",
    "_types",
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DefinitionId {
    pub(crate) module: ModuleId,
    pub(crate) statement: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Binding {
    Module(ModuleId),
    Definition(DefinitionId),
    /// The owning Python module initializes the checked class object.
    RegisteredClass(ruff_db::files::FileRange),
    /// The value stored by a shipped function definition after applying its decorators.
    RegisteredValue(ruff_db::files::FileRange),
    /// The module initializer establishes this binding when its guard executes.
    ConditionalValue,
    /// A class defined by the runtime arm of a TYPE_CHECKING guard.
    RuntimeClass,
    Value(DefinitionId),
    /// The value captured by one named from-import of a mutable module binding.
    ImportedValue(DefinitionId, usize),
    TypeAlias(ModuleId, usize),
    IntrinsicModule(IntrinsicModule),
    Intrinsic(Intrinsic),
}

impl Binding {
    pub(crate) fn is_registered_object(self) -> bool {
        matches!(self, Self::RegisteredClass(_) | Self::RegisteredValue(_))
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ImportSnapshot {
    Intrinsic(Intrinsic),
    Values(Vec<DefinitionId>),
}

/// Sealed namespaces expose only compiler-known symbols, never module values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicModule {
    RuntimeSource(&'static str),
    ContextVars,
    ContextVarsNative,
    Importlib,
    ImportlibAbc,
    ImportlibLoaderAbc,
    ImportlibMachinery,
    ImportlibBootstrapExternal,
    ImportlibMetadata,
    ImportlibReaders,
    ImportlibResources,
    ImportlibResourceReaders,
    ImportlibResourcesAbc,
    ImportlibUtil,
    Array,
    Imp,
    Ast,
    Atexit,
    Marshal,
    Gc,
    Pickle,
    PickleNative,
    Struct,
    AnnotationLib,
    AnnotationFormat,
    Declaration(&'static str),
    Argparse,
    Errno,
    Future,
    Builtins,
    Sys,
    SysJit,
    Traceback,
    Typing,
    Dataclasses,
    Difflib,
    DoctestRuntime,
    Select,
    MockRuntime,
    Logging,
    LoggingConfig,
    LoggingHandlers,
    Datetime,
    DatetimeNative,
    Abc,
    Asyncio,
    Contextlib,
    Inspect,
    Linecache,
    Locale,
    LocaleNative,
    Functools,
    FunctoolsNative,
    Calendar,
    Fractions,
    Fnmatch,
    Glob,
    Time,
    Warnings,
    WarningsNative,
    Tracemalloc,
    TracemallocNative,
    Tomli,
    TomliW,
    Shutil,
    Grp,
    Pwd,
    Termios,
    Resource,
    Fcntl,
    Random,
    Secrets,
    Sysconfig,
    Signal,
    SignalNative,
    Opcode,
    Unidiff,
    Pathlib,
    Shlex,
    Tempfile,
    Subprocess,
    Selectors,
    Os,
    Posix,
    OsPath,
    GenericPath,
    NtPath,
    PosixPath,
    Urllib,
    UrllibParse,
    UrllibRequest,
    UrllibRobotParser,
    UrllibError,
    Email,
    EmailFeedParser,
    EmailHeaderValueParser,
    EmailParser,
    EmailUtils,
    Collections,
    CollectionsNative,
    CollectionsAbc,
    Copy,
    Itertools,
    Math,
    Numbers,
    Operator,
    OperatorNative,
    Textwrap,
    Io,
    IoNative,
    Gzip,
    Lzma,
    LzmaNative,
    UnicodeData,
    WeakRef,
    Threading,
    Thread,
    Codecs,
    CodecsNative,
    Socket,
    SocketNative,
    Hashlib,
    Blake2,
    Sha3,
    Md5,
    Hmac,
    Types,
    Base64,
    Binascii,
    Zipfile,
    Csv,
    StringFormatting,
    Re,
    ReConstants,
    Enum,
    Zlib,
    Json,
    JsonNative,
    JsonEncoder,
    JsonDecoder,
    Imre,
    ImreJson,
    ImreToml,
    Tomllib,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intrinsic {
    RuntimeImport(&'static str),
    Context,
    ContextVar,
    ContextToken,
    CopyContext,
    ModuleSpec,
    CachePath {
        reverse: bool,
    },
    NominalClass(&'static str),
    Number,
    GetAnnotations,
    Overload,
    GetOverloads,
    OverloadRegistry,
    OverloadDummy,
    ClearOverloads,
    ForwardRef,
    AnnotationTypeRepr,
    AnnotationFormat(i64),
    Declaration(&'static str),
    BuiltinsLast,
    BuiltinsLen,
    BuiltinsAscii,
    BuiltinsPrint,
    BuiltinsHex,
    OperatorNot,
    OperatorCall,
    BuiltinsOpen,
    SysDisplayHook,
    ArgparseNamespace,
    ArgparseSuppress,
    BooleanOptionalAction,
    RawDescriptionHelpFormatter,
    FutureAnnotations,
    ShallowCopy,
    DeepCopy,
    TypeChecking,
    GetLogger,
    GetLevelName,
    LoggingDisable,
    LoggingShutdown,
    AddLevelName,
    LevelNamesMapping,
    HandlerRename,
    HandlerLock,
    HandlerLookup,
    HandlerNames,
    FileEncodingUsesUtf8,
    PrintActiveException,
    TracebackPrintExc,
    TracebackFormatter(&'static str),
    HandlerLevelType,
    TextOutputType,
    HandlerFallback,
    LevelMap {
        names: bool,
    },
    LogRecord,
    MakeLogRecord,
    LogFormatter,
    ClockConverter(bool),
    StructTime,
    Mktime,
    LogRecordFlag(u8),
    BasicConfig,
    DictConfig,
    FileConfig,
    LogLevel(i64),
    Abc,
    AbcMeta,
    AbstractMethod,
    CreateSubprocessExec,
    RunLoop,
    NewLoop,
    SetLoop,
    EnsureTask,
    CancelledError,
    Gather,
    TaskGroup,
    Semaphore,
    ContextManager,
    Linecache(crate::ir::LinecacheOperation),
    InspectGetattrStatic,
    InspectIsMethodDescriptor,
    InspectCoroutine {
        mark: bool,
    },
    InspectSentinel,
    WrapperAttributes(WrapperAttributes),
    FunctionCache,
    CacheInfo,
    LruCache,
    SingleDispatch,
    UpdateWrapper,
    Wraps,
    CachedProperty,
    NewType,
    Partial,
    Placeholder(crate::ir::PlaceholderKind),
    Reduce,
    CmpToKey,
    TomlDumps,
    TomliLoads,
    TomliLoad,
    TomliDump,
    StandardEnum,
    IsDataclass,
    Pipe,
    Devnull,
    CheckOutput,
    ShellOutput {
        status: bool,
    },
    NullContext,
    IntConstant(i64),
    WallTime,
    MathAcos,
    MathAsin,
    MathAtan,
    MathDegrees,
    MathFmod,
    MathRadians,
    MathErfc,
    MathLog10,
    MathPerm,
    TimeSleep,
    WarningFilter {
        simple: bool,
    },
    WarningReset,
    CatchWarnings,
    WarningDisplay {
        show: bool,
    },
    PatchSet,
    PatchedFile,
    Hunk,
    UnidiffParseError,
    Awaitable,
    TypeVar,
    Stderr,
    Stdout,
    OriginalStdout,
    SysExecutable,
    SysFlags,
    SysDontWriteBytecode,
    SysImplementation,
    SysPycachePrefix,
    SysPlatform,
    SysMaxSize,
    SysVersionInfo,
    SysThreadInfo,
    SysBuiltinModuleNames,
    SysStdlibModuleNames,
    SysVersion,
    SysHexversion,
    SysEncoding(SysEncoding),
    SysIntern {
        query: bool,
    },
    SysException,
    SysGetFrameModuleName,
    SysGetRecursionLimit,
    SysSetRecursionLimit,
    SysJitPredicate(&'static str),
    Stdin,
    OsName,
    OsLinesep,
    OsPathsep {
        posix: bool,
    },
    OsSeparator {
        alternate: bool,
        posix: bool,
    },
    ErrnoAttribute,
    ErrnoFrom(usize),
    ModuleType,
    GetSysconfigPath,
    OsPathExists,
    StringConstant(&'static str),
    StartupFilesystemRoots,
    ModuleSuffixes,
    SysExit,
    ShlexSplit,
    ShlexQuote,
    ShlexJoin,
    Ndiff,
    DocCaptureStart,
    DocCaptureTake,
    DocDisplay,
    DocExceptionName,
    MockCallList {
        mark: bool,
    },
    DocExecute,
    Final,
    Argv,
    OrigArgv,
    Logger,
    LogManager,
    LogRoot,
    LogEmit(Option<u8>),
    TimedFileHandler,
    ClockTime,
    Calendar(crate::ir::CalendarType),
    CalendarUtc,
    TextIO,
    BinaryIO,
    Optional,
    Union,
    Literal,
    Callable,
    Mapping,
    Iterable,
    Iterator,
    UnsupportedOperation,
    GzipOpen,
    UrlsafeB64encode,
    Base64Encode,
    Base64Decode,
    BinasciiDecode,
    BinasciiEncode,
    BinasciiUnhexlify(&'static str),
    BinasciiError,
    BinasciiIncomplete,
    PackageNotFoundError,
    RegisteredImport(&'static str),
    Socket,
    GaiError,
    HError,
    SocketGetFqdn,
    SocketControlSize {
        padded: bool,
    },
    SocketConstant(&'static str),
    Sha1,
    Sha256,
    NamedHash(&'static str),
    UnicodeProperty(UnicodeProperty),
    UnicodeNormalize {
        check: bool,
    },
    /// Available in an object program, without requesting one when the export is used.
    ObjectOnlyExport(&'static str),
    ObjectRuntimeExport(&'static str),
    /// Bind the object provider when this export is used, after checking typed calls.
    ScopedObjectExport(&'static str),
    WeakProxy,
    WeakProxyClass {
        callable: bool,
    },
    CodecLookup,
    CodecGetWriter,
    CodecBom(&'static [u8]),
    CodecNativeBom(&'static [u8], &'static [u8]),
    HashNew,
    HashFileDigest,
    HmacCompareDigest,
    SimpleNamespace,
    HashAlgorithms {
        guaranteed: bool,
    },
    ZipFile,
    ZipImportIndex,
    ZipInfo,
    BytesIo,
    StringIo,
    BadZipFile,
    BadGzipFile,
    CsvReader,
    FormatterParser,
    FormatterFieldNameSplit,
    CsvError,
    RegexCompile,
    RegexSearch,
    RegexMatchStart,
    RegexSub,
    RegexRuns {
        split: bool,
    },
    RegexVerbose,
    RegexUnicode,
    RegexAscii,
    RegexMultiline,
    RegexPattern,
    RegexMatch,
    StrEnum,
    IntEnum,
    SimpleEnum,
    EnumAuto,
    DefaultDict,
    ChainMap,
    OrderedDict,
    NamedTuple,
    Counter,
    CountElements,
    Deque,
    Repeat,
    AttrGetter,
    LengthHint,
    Count,
    Islice,
    Combinatorial(crate::ir::Combinatorial),
    Tee,
    Cycle,
    GroupBy,
    Chain,
    AbstractSet,
    TomllibLoad,
    TomlDecodeError,
    Dedent,
    ZlibError,
    Adler32,
    JsonDecode,
    JsonEncode,
    JsonDumps,
    JsonDump,
    JsonLoads,
    JsonDecoderClass,
    JsonEncoderClass,
    JsonLoad,
    JsonDecodeError,
    Any,
    TomlDecode,
    Dataclass,
    Struct,
    DataclassField,
    AsDict,
    SelfType,
    ClassVar,
    AssertNever,
    Cast,
    Path,
    PurePath(crate::ir::PathFlavor),
    Which,
    Copy(CopyOperation),
    RmTree,
    CopyTree,
    Move,
    DestInSrc,
    IgnorePatterns,
    ShutilError,
    RandomChoice {
        secure: bool,
    },
    Random(crate::ir::RandomOperation),
    SameFileError,
    SpecialFileError,
    TemporaryDirectory,
    TemporaryFile,
    Popen,
    PopenActive,
    PopenCleanup,
    PopenSelector,
    SelectorClass(bool),
    SubprocessRun,
    SubprocessCall,
    SubprocessCheckCall,
    SubprocessList2Cmdline,
    SubprocessStdout,
    CompletedProcess,
    SubprocessError,
    CalledProcessError,
    TimeoutExpired,
    AccessMode(u8),
    Environ,
    EnvironBytes,
    BoolConstant(bool),
    GetEnv,
    GetCwd {
        bytes: bool,
    },
    Chdir,
    Strerror,
    StatResult,
    Fd(crate::ir::FdOperation),
    Credential(&'static str),
    Readiness(crate::ir::Readiness),
    DescriptorConstant(&'static str),
    FsPath,
    FsDecode,
    FsEncode,
    PosixNormcase,
    PosixPath(PosixPathOperation),
    Fnmatch(FnmatchOperation),
    Quote,
    QuoteFromBytes,
    QuotePlus,
    Unquote,
    UrlEncode,
    UrlSplit,
    UrlUnsplit,
    UrlJoin,
    ParseQsl,
    EmailParseDate,
    EmailFormatDateTime,
    EmailFormatDate,
    SplitResult {
        bytes: bool,
    },
}

impl Intrinsic {
    pub(crate) const ARGUMENT_PARSER: Self = Self::ScopedObjectExport("argparse.ArgumentParser");
    pub(crate) const REGEX_DEBUG: Self = Self::ScopedObjectExport("re.DEBUG");

    pub(crate) fn unary_float_math(self) -> Option<(&'static str, Builtin)> {
        match self {
            Self::MathAcos => Some(("math.acos", Builtin::FloatAcos)),
            Self::MathAsin => Some(("math.asin", Builtin::FloatAsin)),
            Self::MathAtan => Some(("math.atan", Builtin::FloatAtan)),
            Self::MathDegrees => Some(("math.degrees", Builtin::FloatDegrees)),
            Self::MathRadians => Some(("math.radians", Builtin::FloatRadians)),
            Self::MathErfc => Some(("math.erfc", Builtin::FloatErfc)),
            Self::MathLog10 => Some(("math.log10", Builtin::FloatLog10)),
            _ => None,
        }
    }

    pub(crate) fn scoped_class_base(self) -> Option<&'static str> {
        match self {
            Self::SOCKETSERVER_CLASS => Some("socketserver base class"),
            Self::ScopedObjectExport(
                name @ ("enum.Flag"
                | "unittest.IsolatedAsyncioTestCase"
                | "importlib.abc.SourceLoader"),
            ) => Some(name),
            _ => None,
        }
    }

    pub(crate) const SYS_IMPORT_REGISTRY: Self = Self::ScopedObjectExport("sys import registry");
    pub(crate) const SOCKET_MSG_PEEK: Self = Self::ScopedObjectExport("socket.MSG_PEEK");
    pub(crate) const SOCKETSERVER_CLASS: Self = Self::ScopedObjectExport("socketserver classes");
    pub(crate) const PICKLER: Self = Self::ObjectRuntimeExport("pickle.Pickler");
    pub(crate) const IO_BASE: Self = Self::ObjectRuntimeExport("io.IOBase");
    pub(crate) const RAW_IO_BASE: Self = Self::ObjectRuntimeExport("io.RawIOBase");
    pub(crate) const BUFFERED_IO_BASE: Self = Self::ObjectRuntimeExport("io.BufferedIOBase");
    pub(crate) const TEXT_IO_BASE: Self = Self::ObjectRuntimeExport("io.TextIOBase");
    pub(crate) const TYPING_IO: Self = Self::ObjectOnlyExport("typing.IO");
    pub(crate) const TYPING_PROTOCOL: Self = Self::ObjectRuntimeExport("typing.Protocol");
    pub(crate) const TYPING_MAPPING: Self = Self::ObjectRuntimeExport("typing.Mapping");
    pub(crate) const TYPING_SEQUENCE: Self = Self::ObjectRuntimeExport("typing.Sequence");

    /// Subclasses of these immutable types keep their calendar value in the object runtime.
    pub(crate) fn is_subclassable_calendar(self) -> bool {
        use crate::ir::CalendarType::{Date, DateTime, Delta};
        matches!(
            self,
            Self::Calendar(Date | DateTime | Delta) | Self::ClockTime
        )
    }

    /// Resolve the actual public export, including when the caller imported it under an alias.
    pub(crate) fn exported_by(self, module: &str, name: &str) -> bool {
        IntrinsicModule::named(module).and_then(|m| m.symbol(name))
            == Some(Binding::Intrinsic(self))
    }
}

impl IntrinsicModule {
    /// Ty resolves these builtin names solely against its bundled stdlib. Only
    /// the exact import proved by the closed checker needs an import diagnostic.
    pub(crate) fn lacks_builtin_stub(self) -> bool {
        matches!(
            self,
            Self::Opcode
                | Self::ReConstants
                | Self::StringFormatting
                | Self::CollectionsNative
                | Self::DatetimeNative
                | Self::FunctoolsNative
                | Self::SignalNative
        )
    }

    fn named(name: &str) -> Option<Self> {
        match name {
            "array" => Some(Self::Array),
            "_imp" => Some(Self::Imp),
            "ast" => Some(Self::Ast),
            "atexit" => Some(Self::Atexit),
            "marshal" => Some(Self::Marshal),
            "gc" => Some(Self::Gc),
            "pickle" => Some(Self::Pickle),
            "_pickle" => Some(Self::PickleNative),
            "struct" => Some(Self::Struct),
            "annotationlib" => Some(Self::AnnotationLib),
            "__future__" => Some(Self::Future),
            "logging" => Some(Self::Logging),
            "logging.config" => Some(Self::LoggingConfig),
            "logging.handlers" => Some(Self::LoggingHandlers),
            "datetime" => Some(Self::Datetime),
            "_datetime" => Some(Self::DatetimeNative),
            "abc" => Some(Self::Abc),
            "asyncio" => Some(Self::Asyncio),
            "concurrent" => Some(Self::RuntimeSource("concurrent")),
            "concurrent.futures" => Some(Self::RuntimeSource("concurrent.futures")),
            "contextvars" => Some(Self::ContextVars),
            "_contextvars" => Some(Self::ContextVarsNative),
            "inspect" => Some(Self::Inspect),
            "linecache" => Some(Self::Linecache),
            "locale" => Some(Self::Locale),
            "_locale" => Some(Self::LocaleNative),
            "contextlib" => Some(Self::Contextlib),
            "functools" => Some(Self::Functools),
            "_functools" => Some(Self::FunctoolsNative),
            "calendar" => Some(Self::Calendar),
            "fractions" => Some(Self::Fractions),
            "fnmatch" => Some(Self::Fnmatch),
            "glob" => Some(Self::Glob),
            "time" => Some(Self::Time),
            "warnings" => Some(Self::Warnings),
            "_warnings" => Some(Self::WarningsNative),
            "tracemalloc" => Some(Self::Tracemalloc),
            "_tracemalloc" => Some(Self::TracemallocNative),
            "tomli" => Some(Self::Tomli),
            "tomli_w" => Some(Self::TomliW),
            "shutil" => Some(Self::Shutil),
            "fcntl" if cfg!(target_os = "linux") => Some(Self::Fcntl),
            "grp" if cfg!(any(target_os = "linux", target_os = "macos")) => Some(Self::Grp),
            "pwd" if cfg!(any(target_os = "linux", target_os = "macos")) => Some(Self::Pwd),
            "termios" if cfg!(any(target_os = "linux", target_os = "macos")) => Some(Self::Termios),
            "resource" if cfg!(target_os = "linux") => Some(Self::Resource),
            "random" => Some(Self::Random),
            "secrets" => Some(Self::Secrets),
            "sysconfig" => Some(Self::Sysconfig),
            "signal" => Some(Self::Signal),
            "_signal" if cfg!(unix) => Some(Self::SignalNative),
            "_opcode" => Some(Self::Opcode),
            "unidiff" => Some(Self::Unidiff),
            "argparse" => Some(Self::Argparse),
            "builtins" => Some(Self::Builtins),
            "sys" => Some(Self::Sys),
            "traceback" => Some(Self::Traceback),
            "typing" => Some(Self::Typing),
            "dataclasses" => Some(Self::Dataclasses),
            "difflib" => Some(Self::Difflib),
            "select" if cfg!(any(target_os = "linux", target_os = "macos")) => Some(Self::Select),
            "_imre_doctest" => Some(Self::DoctestRuntime),
            "_imre_mock" => Some(Self::MockRuntime),
            "pathlib" => Some(Self::Pathlib),
            "shlex" => Some(Self::Shlex),
            "tempfile" => Some(Self::Tempfile),
            "subprocess" => Some(Self::Subprocess),
            "selectors" => Some(Self::Selectors),
            "os" => Some(Self::Os),
            "posix" if cfg!(unix) => Some(Self::Posix),
            "os.path" => Some(Self::OsPath),
            "genericpath" => Some(Self::GenericPath),
            "ntpath" => Some(Self::NtPath),
            "posixpath" => Some(Self::PosixPath),
            "email" => Some(Self::Email),
            "email._header_value_parser" => Some(Self::EmailHeaderValueParser),
            "email.feedparser" => Some(Self::EmailFeedParser),
            "email.parser" => Some(Self::EmailParser),
            "email.utils" => Some(Self::EmailUtils),
            "urllib" => Some(Self::Urllib),
            "urllib.parse" => Some(Self::UrllibParse),
            "urllib.request" => Some(Self::UrllibRequest),
            "urllib.robotparser" => Some(Self::UrllibRobotParser),
            "urllib.error" => Some(Self::UrllibError),
            "importlib" => Some(Self::Importlib),
            "importlib.abc" => Some(Self::ImportlibAbc),
            "importlib._abc" => Some(Self::ImportlibLoaderAbc),
            "importlib.machinery" => Some(Self::ImportlibMachinery),
            "_frozen_importlib_external" | "importlib._bootstrap_external" => {
                Some(Self::ImportlibBootstrapExternal)
            }
            "importlib.metadata" => Some(Self::ImportlibMetadata),
            "importlib.readers" => Some(Self::ImportlibReaders),
            "importlib.resources" => Some(Self::ImportlibResources),
            "importlib.resources.readers" => Some(Self::ImportlibResourceReaders),
            "importlib.resources.abc" => Some(Self::ImportlibResourcesAbc),
            "importlib.util" => Some(Self::ImportlibUtil),
            "collections" => Some(Self::Collections),
            "_collections" => Some(Self::CollectionsNative),
            "collections.abc" => Some(Self::CollectionsAbc),
            "copy" => Some(Self::Copy),
            "itertools" => Some(Self::Itertools),
            "math" => Some(Self::Math),
            "numbers" => Some(Self::Numbers),
            "operator" => Some(Self::Operator),
            "_operator" => Some(Self::OperatorNative),
            "textwrap" => Some(Self::Textwrap),
            "io" => Some(Self::Io),
            "_io" => Some(Self::IoNative),
            "gzip" => Some(Self::Gzip),
            "lzma" => Some(Self::Lzma),
            "_lzma" => Some(Self::LzmaNative),
            "types" => Some(Self::Types),
            "unicodedata" => Some(Self::UnicodeData),
            "weakref" => Some(Self::WeakRef),
            "threading" => Some(Self::Threading),
            "_thread" => Some(Self::Thread),
            "codecs" => Some(Self::Codecs),
            "_codecs" => Some(Self::CodecsNative),
            "socket" => Some(Self::Socket),
            "_socket" => Some(Self::SocketNative),
            "hashlib" => Some(Self::Hashlib),
            "_blake2" => Some(Self::Blake2),
            "_sha3" => Some(Self::Sha3),
            "_md5" => Some(Self::Md5),
            "hmac" => Some(Self::Hmac),
            "base64" => Some(Self::Base64),
            "binascii" => Some(Self::Binascii),
            "zipfile" => Some(Self::Zipfile),
            "csv" => Some(Self::Csv),
            "_string" => Some(Self::StringFormatting),
            "re" => Some(Self::Re),
            "re._constants" => Some(Self::ReConstants),
            "enum" => Some(Self::Enum),
            "zlib" => Some(Self::Zlib),
            "json" => Some(Self::Json),
            "_json" => Some(Self::JsonNative),
            "json.encoder" => Some(Self::JsonEncoder),
            "json.decoder" => Some(Self::JsonDecoder),
            "imre" => Some(Self::Imre),
            "imre.json" => Some(Self::ImreJson),
            "imre.toml" => Some(Self::ImreToml),
            "errno" => Some(Self::Errno),
            "tomllib" => Some(Self::Tomllib),
            _ => declarations::module(name).map(Self::Declaration),
        }
    }

    pub(crate) fn symbol(self, name: &str) -> Option<Binding> {
        if self == Self::Fractions {
            return (name != "Fraction").then_some(Binding::ConditionalValue);
        }
        if self == Self::Calendar {
            return Some(match name {
                "Calendar" | "TextCalendar" => return None,
                "monthcalendar" => {
                    Binding::Intrinsic(Intrinsic::ScopedObjectExport("calendar.monthcalendar"))
                }
                "firstweekday" => {
                    Binding::Intrinsic(Intrinsic::ScopedObjectExport("calendar.firstweekday"))
                }
                "setfirstweekday" => {
                    Binding::Intrinsic(Intrinsic::ScopedObjectExport("calendar.setfirstweekday"))
                }
                _ => Binding::ConditionalValue,
            });
        }
        // The bundled ntpath binds platform-dependent names during module initialization.
        if self == Self::NtPath {
            return Some(Binding::ConditionalValue);
        }
        if self == Self::Socket
            && let Some(&(_, value)) = imre_ast::providers::SOCKET_LOOPBACK_HOSTS
                .iter()
                .find(|&&(attr, _)| attr == name)
        {
            return Some(Binding::Intrinsic(Intrinsic::StringConstant(value)));
        }
        if matches!(self, Self::Lzma | Self::LzmaNative)
            && let Some(&(_, value)) = imre_ast::lzma::INTEGER_CONSTANTS
                .iter()
                .find(|&&(attr, _)| attr == name)
        {
            return Some(Binding::Intrinsic(Intrinsic::IntConstant(value)));
        }
        #[cfg(target_os = "linux")]
        if self == Self::Select && imre_ast::epoll::FLAGS.iter().any(|&(flag, _)| name == flag) {
            // Resolve the target ABI flag in the same runtime as the epoll descriptor.
            return Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
                "select epoll constant",
            )));
        }
        if self == Self::Zlib
            && let Some(value) = imre_ast::zlib::integer_constant(name)
        {
            return Some(Binding::Intrinsic(Intrinsic::IntConstant(value)));
        }
        if self == Self::DatetimeNative {
            return Self::Datetime.symbol(name);
        }
        if self == Self::Inspect
            && let Some(&(_, flag)) = imre_ast::code_flags::INSPECT_FLAGS
                .iter()
                .find(|&&(attr, _)| attr == name)
        {
            return Some(Binding::Intrinsic(Intrinsic::IntConstant(i64::from(flag))));
        }
        if self == Self::AnnotationLib && name == "Format" {
            return Some(Binding::IntrinsicModule(Self::AnnotationFormat));
        }
        if self == Self::Logging && name == "handlers" {
            return Some(Binding::IntrinsicModule(Self::LoggingHandlers));
        }
        if self == Self::Logging && name == "config" {
            return Some(Binding::IntrinsicModule(Self::LoggingConfig));
        }
        if self == Self::Re && name == "_constants" {
            return Some(Binding::IntrinsicModule(Self::ReConstants));
        }
        if self == Self::Sys && name == "_jit" {
            return Some(Binding::IntrinsicModule(Self::SysJit));
        }
        if matches!(self, Self::Os | Self::OsPath | Self::PosixPath) && name == "pathsep" {
            return Some(Binding::Intrinsic(Intrinsic::OsPathsep {
                posix: self == Self::PosixPath,
            }));
        }
        if matches!(self, Self::Os | Self::OsPath | Self::PosixPath)
            && matches!(name, "sep" | "altsep")
        {
            return Some(Binding::Intrinsic(Intrinsic::OsSeparator {
                alternate: name == "altsep",
                posix: self == Self::PosixPath,
            }));
        }
        if self == Self::OsPath {
            return Self::PosixPath.symbol(name);
        }
        if self == Self::Os {
            if name == "path" {
                return Some(Binding::IntrinsicModule(Self::OsPath));
            }
            if let Some(symbol) = os::symbol(name) {
                return Some(Binding::Intrinsic(symbol));
            }
        }
        if self == Self::Json && matches!(name, "encoder" | "decoder") {
            return Some(Binding::IntrinsicModule(if name == "encoder" {
                Self::JsonEncoder
            } else {
                Self::JsonDecoder
            }));
        }
        if self == Self::Email {
            let child = match name {
                "_header_value_parser" => Some(Self::EmailHeaderValueParser),
                "feedparser" => Some(Self::EmailFeedParser),
                "parser" => Some(Self::EmailParser),
                "utils" => Some(Self::EmailUtils),
                _ => None,
            };
            if let Some(child) = child {
                return Some(Binding::IntrinsicModule(child));
            }
        }
        if self == Self::Urllib {
            let child = match name {
                "parse" => Self::UrllibParse,
                "request" => Self::UrllibRequest,
                "robotparser" => Self::UrllibRobotParser,
                "error" => Self::UrllibError,
                _ => return None,
            };
            return Some(Binding::IntrinsicModule(child));
        }
        if self == Self::ImportlibAbc && name == "machinery" {
            return Some(Binding::IntrinsicModule(Self::ImportlibMachinery));
        }
        if self == Self::ImportlibResources && name == "abc" {
            return Some(Binding::IntrinsicModule(Self::ImportlibResourcesAbc));
        }
        if self == Self::ImportlibResources && name == "readers" {
            return Some(Binding::IntrinsicModule(Self::ImportlibResourceReaders));
        }
        if self == Self::Collections && name == "abc" {
            return Some(Binding::IntrinsicModule(Self::CollectionsAbc));
        }
        if self == Self::Imre && name == "json" {
            return Some(Binding::IntrinsicModule(Self::ImreJson));
        }
        if self == Self::Imre && name == "toml" {
            return Some(Binding::IntrinsicModule(Self::ImreToml));
        }
        let symbol = match (self, name) {
            (Self::Array, "array") => Intrinsic::NominalClass("array.array"),
            (Self::Numbers, "Number") => Intrinsic::Number,
            (Self::Numbers, "Rational") => Intrinsic::ScopedObjectExport("numbers.Rational"),
            (Self::Ast, "parse") => Intrinsic::ObjectRuntimeExport("ast.parse"),
            (Self::Ast, "walk") => Intrinsic::ObjectRuntimeExport("ast.walk"),
            (Self::Ast, "unparse") => Intrinsic::ObjectRuntimeExport("ast.unparse"),
            (Self::Ast, "iter_fields") => Intrinsic::ObjectRuntimeExport("ast.iter_fields"),
            (Self::Ast, "iter_child_nodes") => {
                Intrinsic::ObjectRuntimeExport("ast.iter_child_nodes")
            }
            (Self::Ast, "NodeVisitor") => Intrinsic::ObjectRuntimeExport("ast.NodeVisitor"),
            (Self::Ast, "NodeTransformer") => Intrinsic::ObjectRuntimeExport("ast.NodeTransformer"),
            (Self::Ast, "literal_eval") => Intrinsic::ObjectRuntimeExport("ast.literal_eval"),
            (Self::Ast, name) => {
                Intrinsic::ObjectRuntimeExport(imre_ast::python_nodes::find(name)?.qualified_name)
            }
            (Self::Marshal, "dumps") => Intrinsic::ObjectRuntimeExport("marshal.dumps"),
            (Self::Marshal, "loads") => Intrinsic::ObjectRuntimeExport("marshal.loads"),
            (Self::Marshal, "version") => {
                Intrinsic::IntConstant(i64::from(imre_ast::MARSHAL_VERSION))
            }
            (Self::Codecs | Self::CodecsNative, "register") => {
                Intrinsic::ObjectRuntimeExport("_codecs.register")
            }
            (Self::Codecs | Self::CodecsNative, "unregister") => {
                Intrinsic::ObjectRuntimeExport("_codecs.unregister")
            }
            (Self::Pickle, "Pickler") => Intrinsic::PICKLER,
            (Self::PickleNative, "dumps") => Intrinsic::ScopedObjectExport("_pickle.dumps"),
            (Self::PickleNative, "PickleError") => {
                Intrinsic::ScopedObjectExport("_pickle.PickleError")
            }
            (Self::PickleNative, "PicklingError") => {
                Intrinsic::ScopedObjectExport("_pickle.PicklingError")
            }
            (Self::PickleNative, "UnpicklingError") => {
                Intrinsic::ScopedObjectExport("_pickle.UnpicklingError")
            }
            (Self::Struct, "_clearcache") => Intrinsic::ScopedObjectExport("struct._clearcache"),
            (Self::Tempfile, "gettempdir") => Intrinsic::ScopedObjectExport("tempfile.gettempdir"),
            (Self::Tempfile, "gettempdirb") => {
                Intrinsic::ScopedObjectExport("tempfile.gettempdirb")
            }
            (Self::Tempfile, "gettempprefix") => {
                Intrinsic::ScopedObjectExport("tempfile.gettempprefix")
            }
            (Self::Tempfile, "gettempprefixb") => {
                Intrinsic::ScopedObjectExport("tempfile.gettempprefixb")
            }
            (Self::CollectionsAbc, "Sequence") => {
                Intrinsic::NominalClass("collections.abc.Sequence")
            }
            (Self::CollectionsAbc, "MutableSequence") => {
                Intrinsic::NominalClass("collections.abc.MutableSequence")
            }
            (Self::CollectionsAbc, "MutableMapping") => {
                Intrinsic::NominalClass("collections.abc.MutableMapping")
            }
            (Self::AnnotationLib, "ForwardRef") => Intrinsic::ForwardRef,
            (Self::AnnotationLib, "get_annotations") => Intrinsic::GetAnnotations,
            (Self::AnnotationLib, "type_repr") => Intrinsic::AnnotationTypeRepr,
            (Self::AnnotationFormat, "VALUE") => Intrinsic::AnnotationFormat(1),
            (Self::AnnotationFormat, "VALUE_WITH_FAKE_GLOBALS") => Intrinsic::AnnotationFormat(2),
            (Self::AnnotationFormat, "FORWARDREF") => Intrinsic::AnnotationFormat(3),
            (Self::AnnotationFormat, "STRING") => Intrinsic::AnnotationFormat(4),
            // Execution requests the native loader. Binding the name alone stays on the typed path.
            (Self::Importlib, "import_module") => {
                Intrinsic::RegisteredImport("importlib.import_module")
            }
            (Self::Importlib, "invalidate_caches") => {
                Intrinsic::ObjectRuntimeExport("importlib.invalidate_caches")
            }
            (Self::Importlib, "reload") => Intrinsic::ObjectOnlyExport("importlib.reload"),
            (Self::ImportlibLoaderAbc, "Loader") => {
                Intrinsic::ObjectRuntimeExport("importlib._abc.Loader")
            }
            (Self::ImportlibReaders | Self::ImportlibResourceReaders, "FileReader") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.readers.FileReader")
            }
            (Self::ImportlibReaders | Self::ImportlibResourceReaders, "ZipReader") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.readers.ZipReader")
            }
            (Self::ImportlibReaders | Self::ImportlibResourceReaders, "MultiplexedPath") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.readers.MultiplexedPath")
            }
            (Self::ImportlibReaders | Self::ImportlibResourceReaders, "NamespaceReader") => {
                Intrinsic::ObjectRuntimeExport("importlib.resources.readers.NamespaceReader")
            }
            (Self::ImportlibMetadata, "PackageNotFoundError") => Intrinsic::PackageNotFoundError,
            (Self::ImportlibMachinery, "ModuleSpec") => Intrinsic::ModuleSpec,
            (Self::ImportlibMachinery, "FrozenImporter") => {
                Intrinsic::ScopedObjectExport("importlib.machinery.FrozenImporter")
            }
            (Self::ImportlibMachinery, "BuiltinImporter") => {
                Intrinsic::ScopedObjectExport("importlib.machinery.BuiltinImporter")
            }
            (Self::ImportlibMachinery | Self::ImportlibBootstrapExternal, "NamespaceLoader") => {
                Intrinsic::ObjectRuntimeExport("_frozen_importlib_external.NamespaceLoader")
            }
            (Self::ImportlibAbc, "SourceLoader") => {
                Intrinsic::ScopedObjectExport("importlib.abc.SourceLoader")
            }
            (Self::ImportlibBootstrapExternal, "SourceFileLoader") => {
                Intrinsic::ObjectRuntimeExport("_frozen_importlib_external.SourceFileLoader")
            }
            (
                Self::ImportlibMachinery | Self::ImportlibBootstrapExternal,
                "SourcelessFileLoader",
            ) => Intrinsic::ObjectRuntimeExport("_frozen_importlib_external.SourcelessFileLoader"),
            (Self::ImportlibUtil, "find_spec") => {
                Intrinsic::RegisteredImport("importlib.util.find_spec")
            }
            (Self::ImportlibUtil, "cache_from_source") => Intrinsic::CachePath { reverse: false },
            (Self::ImportlibUtil, "source_from_cache") => Intrinsic::CachePath { reverse: true },
            (Self::Future, "annotations") => Intrinsic::FutureAnnotations,
            (Self::Logging, "Formatter") => Intrinsic::LogFormatter,
            (Self::Time, "strptime") => Intrinsic::ScopedObjectExport("time.strptime"),
            (Self::Time, "_STRUCT_TM_ITEMS") => {
                Intrinsic::ScopedObjectExport("time._STRUCT_TM_ITEMS")
            }
            (Self::Time, "struct_time") => Intrinsic::StructTime,
            (Self::Time, "mktime") => Intrinsic::Mktime,
            (Self::Time, "gmtime") => Intrinsic::ClockConverter(true),
            (Self::Time, "localtime") => Intrinsic::ClockConverter(false),
            (Self::Logging, "LogRecord" | "_logRecordFactory") => Intrinsic::LogRecord,
            (Self::Logging, "logThreads") => Intrinsic::LogRecordFlag(1),
            (Self::Logging, "logProcesses") => Intrinsic::LogRecordFlag(2),
            (Self::Logging, "logMultiprocessing") => Intrinsic::LogRecordFlag(4),
            (Self::Logging, "logAsyncioTasks") => Intrinsic::LogRecordFlag(8),
            (Self::Logging, "raiseExceptions") => Intrinsic::LogRecordFlag(16),
            (Self::Logging, "makeLogRecord") => Intrinsic::MakeLogRecord,
            (Self::Logging, "getLogger") => Intrinsic::GetLogger,
            (Self::Logging, "disable") => Intrinsic::LoggingDisable,
            (Self::Logging, "shutdown") => Intrinsic::LoggingShutdown,
            (Self::Logging, "getLevelName") => Intrinsic::GetLevelName,
            (Self::Logging, "addLevelName") => Intrinsic::AddLevelName,
            (Self::Logging, "getLevelNamesMapping") => Intrinsic::LevelNamesMapping,
            (Self::Logging, "_nameToLevel") => Intrinsic::LevelMap { names: true },
            (Self::Logging, "_levelToName") => Intrinsic::LevelMap { names: false },
            (Self::Logging, "basicConfig") => Intrinsic::BasicConfig,
            (Self::LoggingConfig, "dictConfig") => Intrinsic::DictConfig,
            (Self::LoggingConfig, "fileConfig") => Intrinsic::FileConfig,
            (Self::Logging, "DEBUG") => Intrinsic::LogLevel(10),
            (Self::Logging, "INFO") => Intrinsic::LogLevel(20),
            (Self::Abc, "ABC") => Intrinsic::Abc,
            (Self::Abc, "ABCMeta") => Intrinsic::AbcMeta,
            (Self::Abc, "get_cache_token") => Intrinsic::ObjectRuntimeExport("abc.get_cache_token"),
            (Self::Abc, "abstractmethod") => Intrinsic::AbstractMethod,
            (Self::Asyncio, "create_subprocess_exec") => Intrinsic::CreateSubprocessExec,
            (Self::Asyncio, "run") => Intrinsic::RunLoop,
            (Self::ContextVars | Self::ContextVarsNative, "Context") => Intrinsic::Context,
            (Self::ContextVars | Self::ContextVarsNative, "ContextVar") => Intrinsic::ContextVar,
            (Self::ContextVars | Self::ContextVarsNative, "Token") => Intrinsic::ContextToken,
            (Self::ContextVars | Self::ContextVarsNative, "copy_context") => Intrinsic::CopyContext,
            (Self::Asyncio, "new_event_loop") => Intrinsic::NewLoop,
            (Self::Asyncio, "set_event_loop") => Intrinsic::SetLoop,
            (Self::Asyncio, "ensure_future") => Intrinsic::EnsureTask,
            (Self::Asyncio, "CancelledError") => Intrinsic::CancelledError,
            (Self::Asyncio, "gather") => Intrinsic::Gather,
            (Self::Asyncio, "TaskGroup") => Intrinsic::TaskGroup,
            (Self::Asyncio, "Semaphore") => Intrinsic::Semaphore,
            (Self::Linecache, name) => Intrinsic::Linecache(match name {
                "getline" => crate::ir::LinecacheOperation::Getline,
                "getlines" => crate::ir::LinecacheOperation::Getlines,
                "updatecache" => crate::ir::LinecacheOperation::Update,
                "clearcache" => crate::ir::LinecacheOperation::Clear,
                "checkcache" => crate::ir::LinecacheOperation::Check,
                "lazycache" => crate::ir::LinecacheOperation::Lazy,
                "cache" => crate::ir::LinecacheOperation::Cache,
                _ => return None,
            }),
            (Self::Inspect, "getdoc") => Intrinsic::ScopedObjectExport("inspect.getdoc"),
            (Self::Inspect, "ismemberdescriptor") => {
                Intrinsic::ScopedObjectExport("inspect.ismemberdescriptor")
            }
            (Self::Inspect, "trace") => Intrinsic::ScopedObjectExport("inspect.trace"),
            (Self::Inspect, "getinnerframes") => {
                Intrinsic::ScopedObjectExport("inspect.getinnerframes")
            }
            (Self::Inspect, "Attribute") => Intrinsic::ScopedObjectExport("inspect.Attribute"),
            (Self::Inspect, "classify_class_attrs") => {
                Intrinsic::ScopedObjectExport("inspect.classify_class_attrs")
            }
            (Self::Inspect, "getmembers") => Intrinsic::ScopedObjectExport("inspect.getmembers"),
            (Self::Inspect, "getmembers_static") => {
                Intrinsic::ScopedObjectExport("inspect.getmembers_static")
            }
            (Self::Inspect, "isdatadescriptor") => {
                Intrinsic::ScopedObjectExport("inspect.isdatadescriptor")
            }
            (Self::Inspect, "isabstract") => Intrinsic::ScopedObjectExport("inspect.isabstract"),
            (Self::Inspect, "isasyncgen") => Intrinsic::ScopedObjectExport("inspect.isasyncgen"),
            (Self::Inspect, "isasyncgenfunction") => {
                Intrinsic::ScopedObjectExport("inspect.isasyncgenfunction")
            }
            (Self::Inspect, "iscoroutine") => Intrinsic::ScopedObjectExport("inspect.iscoroutine"),
            (Self::Inspect, "isgenerator") => Intrinsic::ScopedObjectExport("inspect.isgenerator"),
            (Self::Inspect, "isgeneratorfunction") => {
                Intrinsic::ScopedObjectExport("inspect.isgeneratorfunction")
            }
            (Self::Inspect, "ispackage") => Intrinsic::ScopedObjectExport("inspect.ispackage"),
            (Self::Inspect, "isawaitable") => Intrinsic::ScopedObjectExport("inspect.isawaitable"),
            (Self::Inspect, "getattr_static") => Intrinsic::InspectGetattrStatic,
            (Self::Inspect, "ismethoddescriptor") => Intrinsic::InspectIsMethodDescriptor,
            (Self::Inspect, "iscoroutinefunction") => Intrinsic::InspectCoroutine { mark: false },
            (Self::Inspect, "markcoroutinefunction") => Intrinsic::InspectCoroutine { mark: true },
            (Self::Inspect, "_sentinel") => Intrinsic::InspectSentinel,
            (Self::Contextlib, "contextmanager") => Intrinsic::ContextManager,
            (Self::Contextlib, "nullcontext") => Intrinsic::NullContext,
            (Self::Warnings, "simplefilter") => Intrinsic::WarningFilter { simple: true },
            (Self::Warnings, "filterwarnings") => Intrinsic::WarningFilter { simple: false },
            (Self::Warnings, "catch_warnings") => Intrinsic::CatchWarnings,
            (Self::Warnings, "resetwarnings") => Intrinsic::WarningReset,
            (Self::Warnings, "formatwarning") => Intrinsic::WarningDisplay { show: false },
            (Self::Warnings, "showwarning") => Intrinsic::WarningDisplay { show: true },
            #[cfg(target_os = "linux")]
            (Self::Fcntl, "F_DUPFD") => Intrinsic::IntConstant(i64::from(libc::F_DUPFD)),
            #[cfg(target_os = "linux")]
            (Self::Fcntl, "F_DUPFD_CLOEXEC") => {
                Intrinsic::IntConstant(i64::from(libc::F_DUPFD_CLOEXEC))
            }
            #[cfg(target_os = "linux")]
            (Self::Time, "CLOCK_REALTIME") => {
                Intrinsic::IntConstant(i64::from(libc::CLOCK_REALTIME))
            }
            #[cfg(target_os = "linux")]
            (Self::Time, "clock_gettime") => Intrinsic::ObjectRuntimeExport("time.clock_gettime"),
            #[cfg(target_os = "linux")]
            (Self::Time, "clock_gettime_ns") => {
                Intrinsic::ObjectRuntimeExport("time.clock_gettime_ns")
            }
            (Self::Time, "sleep") => Intrinsic::TimeSleep,
            (Self::Time, "time") => Intrinsic::WallTime,
            (Self::Signal | Self::SignalNative, "signal") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("_signal.signal")
            }
            (Self::Signal | Self::SignalNative, "getsignal") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("_signal.getsignal")
            }
            (Self::Signal | Self::SignalNative, "default_int_handler") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("_signal.default_int_handler")
            }
            (Self::Signal | Self::SignalNative, "SIG_DFL" | "SIG_IGN") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("signal.Handlers")
            }
            (Self::Signal, "Handlers") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("signal.Handlers")
            }
            (Self::Signal, "SIGINT") => Intrinsic::IntConstant(2),
            (Self::Signal, "SIGTERM") => Intrinsic::IntConstant(15),
            (Self::Signal | Self::SignalNative, "pause") if cfg!(unix) => {
                Intrinsic::ScopedObjectExport("_signal.pause")
            }
            (Self::Signal | Self::SignalNative, "alarm") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("_signal.alarm")
            }
            (Self::Signal, "SIGALRM") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("signal.SIGALRM")
            }
            (Self::Signal, "Signals") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("signal.Signals")
            }
            #[cfg(unix)]
            (Self::Signal, "sigpending") => Intrinsic::ObjectRuntimeExport("signal.sigpending"),
            #[cfg(unix)]
            (Self::SignalNative, "sigpending") => {
                Intrinsic::ObjectRuntimeExport("_signal.sigpending")
            }
            #[cfg(unix)]
            (Self::Signal | Self::SignalNative, "NSIG") => {
                Intrinsic::ObjectRuntimeExport("signal.NSIG")
            }
            // Signal numbers are IntEnum instances when accessed through the boxed module.
            #[cfg(unix)]
            (
                Self::Signal | Self::SignalNative,
                "SIGHUP" | "SIGQUIT" | "SIGILL" | "SIGTRAP" | "SIGABRT" | "SIGIOT" | "SIGBUS"
                | "SIGFPE" | "SIGKILL" | "SIGUSR1" | "SIGSEGV" | "SIGUSR2" | "SIGPIPE" | "SIGCHLD"
                | "SIGCLD" | "SIGCONT" | "SIGSTOP" | "SIGTSTP" | "SIGTTIN" | "SIGTTOU" | "SIGURG"
                | "SIGXCPU" | "SIGXFSZ" | "SIGVTALRM" | "SIGPROF" | "SIGWINCH" | "SIGIO"
                | "SIGPOLL" | "SIGPWR" | "SIGSYS" | "SIGSTKFLT" | "SIGEMT" | "SIGRTMIN"
                | "SIGRTMAX",
            ) => Intrinsic::ObjectRuntimeExport("signal.Signals"),
            // Native executables have no adaptive bytecode interpreter.
            (Self::Opcode, "ENABLE_SPECIALIZATION" | "ENABLE_SPECIALIZATION_FT") => {
                Intrinsic::IntConstant(0)
            }
            (Self::Unidiff, "PatchSet") => Intrinsic::PatchSet,
            (Self::Unidiff, "PatchedFile") => Intrinsic::PatchedFile,
            (Self::Unidiff, "Hunk") => Intrinsic::Hunk,
            (Self::Unidiff, "UnidiffParseError") => Intrinsic::UnidiffParseError,
            (Self::CollectionsAbc, "Awaitable") => Intrinsic::Awaitable,
            (Self::Typing, "overload") => Intrinsic::Overload,
            (Self::Typing, "get_overloads") => Intrinsic::GetOverloads,
            (Self::Typing, "_overload_registry") => Intrinsic::OverloadRegistry,
            (Self::Typing, "_overload_dummy") => Intrinsic::OverloadDummy,
            (Self::Typing, "clear_overloads") => Intrinsic::ClearOverloads,
            (Self::Typing, "TypeVar") => Intrinsic::TypeVar,
            (Self::Typing, "NewType") => Intrinsic::NewType,
            (Self::Functools, "cache") => Intrinsic::FunctionCache,
            (Self::Functools, "_CacheInfo") => Intrinsic::CacheInfo,
            (Self::Functools, "WRAPPER_ASSIGNMENTS") => {
                Intrinsic::WrapperAttributes(WrapperAttributes::Assigned)
            }
            (Self::Functools, "WRAPPER_UPDATES") => {
                Intrinsic::WrapperAttributes(WrapperAttributes::Updated)
            }
            (Self::Functools, "lru_cache") => Intrinsic::LruCache,
            (Self::Functools, "singledispatch") => Intrinsic::SingleDispatch,
            (Self::Functools, "update_wrapper") => Intrinsic::UpdateWrapper,
            (Self::Functools, "wraps") => Intrinsic::Wraps,
            (Self::Glob, "glob") => Intrinsic::ObjectOnlyExport("glob.glob"),
            (Self::Glob, "iglob") => Intrinsic::ObjectOnlyExport("glob.iglob"),
            (Self::Glob, "escape") => Intrinsic::ObjectOnlyExport("glob.escape"),
            (Self::Glob, "translate") => Intrinsic::ObjectOnlyExport("glob.translate"),
            (Self::Fnmatch, "fnmatch" | "fnmatchcase") => {
                Intrinsic::Fnmatch(FnmatchOperation::Match {
                    normcase: name == "fnmatch",
                })
            }
            (Self::Fnmatch, "filter" | "filterfalse") => {
                Intrinsic::Fnmatch(FnmatchOperation::Filter {
                    inverse: name == "filterfalse",
                })
            }
            (Self::Fnmatch, "translate") => Intrinsic::Fnmatch(FnmatchOperation::Translate),
            (Self::Fnmatch, "_translate") => Intrinsic::Fnmatch(FnmatchOperation::Parts),
            (Self::Functools, "cached_property") => Intrinsic::CachedProperty,
            (Self::Functools | Self::FunctoolsNative, "partial") => Intrinsic::Partial,
            (Self::Functools | Self::FunctoolsNative, "Placeholder") => {
                Intrinsic::Placeholder(crate::ir::PlaceholderKind::Value)
            }
            (Self::Functools | Self::FunctoolsNative, "_PlaceholderType") => {
                Intrinsic::Placeholder(crate::ir::PlaceholderKind::Class)
            }
            (Self::Functools | Self::FunctoolsNative, "reduce") => Intrinsic::Reduce,
            (Self::Functools, "cmp_to_key") => Intrinsic::CmpToKey,
            (Self::TomliW, "dumps") => Intrinsic::TomlDumps,
            (Self::Tomli, "loads") => Intrinsic::TomliLoads,
            (Self::Tomli, "load") => Intrinsic::TomliLoad,
            (Self::TomliW, "dump") => Intrinsic::TomliDump,
            (Self::Enum, "Flag") => Intrinsic::ScopedObjectExport("enum.Flag"),
            (Self::Enum, "Enum") => Intrinsic::StandardEnum,
            (Self::Dataclasses, "is_dataclass") => Intrinsic::IsDataclass,
            (Self::Subprocess, "__all__") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("subprocess.__all__")
            }
            (Self::Subprocess, "PIPE") => Intrinsic::Pipe,
            (Self::Subprocess, "DEVNULL") => Intrinsic::Devnull,
            (Self::Subprocess, "check_output") => Intrinsic::CheckOutput,
            (Self::Subprocess, "getoutput") => Intrinsic::ShellOutput { status: false },
            (Self::Subprocess, "getstatusoutput") => Intrinsic::ShellOutput { status: true },
            (Self::Typing, "TYPE_CHECKING") => Intrinsic::TypeChecking,
            (Self::Sys, "stdin" | "__stdin__") => Intrinsic::Stdin,
            (Self::Sys, "stdout") => Intrinsic::Stdout,
            (Self::Sys, "__stdout__") => Intrinsic::OriginalStdout,
            (Self::Sys, "stderr" | "__stderr__") => Intrinsic::Stderr,
            (Self::Sys, "exception") => Intrinsic::SysException,
            (Self::Traceback, "print_exc") => Intrinsic::TracebackPrintExc,
            (Self::Traceback, "format_tb") => Intrinsic::ScopedObjectExport("traceback.format_tb"),
            (Self::Traceback, "print_list") => {
                Intrinsic::ScopedObjectExport("traceback.print_list")
            }
            (Self::Traceback, "format_exc") => {
                Intrinsic::TracebackFormatter("traceback.format_exc")
            }
            (Self::Traceback, "format_exception") => {
                Intrinsic::TracebackFormatter("traceback.format_exception")
            }
            (Self::Traceback, "print_exception") => {
                Intrinsic::TracebackFormatter("traceback.print_exception")
            }
            (Self::Sys, "_get_cpu_count_config") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("sys._get_cpu_count_config")
            }
            (Self::Sys, "_getframemodulename") => Intrinsic::SysGetFrameModuleName,
            (Self::Sys, "getrecursionlimit") => Intrinsic::SysGetRecursionLimit,
            (Self::Sys, "setrecursionlimit") => Intrinsic::SysSetRecursionLimit,
            (Self::SysJit, "is_available") => Intrinsic::SysJitPredicate("_jit.is_available"),
            (Self::SysJit, "is_enabled") => Intrinsic::SysJitPredicate("_jit.is_enabled"),
            (Self::SysJit, "is_active") => Intrinsic::SysJitPredicate("_jit.is_active"),
            (Self::Locale | Self::LocaleNative, "getencoding") => {
                Intrinsic::SysEncoding(SysEncoding::Locale)
            }
            (Self::Locale | Self::LocaleNative, "CHAR_MAX") => {
                Intrinsic::IntConstant(i64::from(std::ffi::c_char::MAX))
            }
            (Self::LocaleNative, "localeconv") => {
                Intrinsic::ScopedObjectExport("_locale.localeconv")
            }
            (Self::Locale, "localeconv") => Intrinsic::ScopedObjectExport("locale.localeconv"),
            (Self::Locale, "format_string") => {
                Intrinsic::ScopedObjectExport("locale.format_string")
            }
            (Self::Locale, "localize") => Intrinsic::ScopedObjectExport("locale.localize"),
            (Self::LocaleNative, "setlocale") => {
                Intrinsic::ObjectRuntimeExport("_locale.setlocale")
            }
            (Self::Locale | Self::LocaleNative, "Error") => {
                Intrinsic::ObjectRuntimeExport("_locale.Error")
            }
            (Self::Locale | Self::LocaleNative, "LC_ALL") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_ALL")
            }
            (Self::Locale | Self::LocaleNative, "LC_COLLATE") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_COLLATE")
            }
            (Self::Locale | Self::LocaleNative, "LC_CTYPE") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_CTYPE")
            }
            (Self::Locale | Self::LocaleNative, "LC_MESSAGES") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_MESSAGES")
            }
            (Self::Locale | Self::LocaleNative, "LC_MONETARY") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_MONETARY")
            }
            (Self::Locale | Self::LocaleNative, "LC_NUMERIC") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_NUMERIC")
            }
            (Self::Locale | Self::LocaleNative, "LC_TIME") => {
                Intrinsic::ObjectRuntimeExport("_locale.LC_TIME")
            }
            (Self::Sys, "getdefaultencoding") => Intrinsic::SysEncoding(SysEncoding::Default),
            (Self::Sys, "getfilesystemencoding") => Intrinsic::SysEncoding(SysEncoding::Filesystem),
            (Self::Sys, "getfilesystemencodeerrors") => {
                Intrinsic::SysEncoding(SysEncoding::FilesystemErrors)
            }
            (Self::Sys, "intern") => Intrinsic::SysIntern { query: false },
            (Self::Sys, "_is_interned") => Intrinsic::SysIntern { query: true },
            (Self::Sys, "meta_path" | "path_hooks" | "path_importer_cache") => {
                Intrinsic::SYS_IMPORT_REGISTRY
            }
            (Self::Sys, "flags") => Intrinsic::SysFlags,
            (Self::Sys, "float_info") => Intrinsic::ScopedObjectExport("sys.float_info"),
            (Self::Sys, "float_repr_style") => {
                Intrinsic::ScopedObjectExport("sys.float_repr_style")
            }
            (Self::Sys, "implementation") => Intrinsic::SysImplementation,
            (Self::Sys, "pycache_prefix") => Intrinsic::SysPycachePrefix,
            (Self::Sys, "dont_write_bytecode") => Intrinsic::SysDontWriteBytecode,
            (Self::Sys, "version_info") => Intrinsic::SysVersionInfo,
            (Self::Sys, "thread_info") => Intrinsic::SysThreadInfo,
            (Self::Sys, "builtin_module_names") => Intrinsic::SysBuiltinModuleNames,
            (Self::Sys, "stdlib_module_names") => Intrinsic::SysStdlibModuleNames,
            (Self::Sys, "version") => Intrinsic::SysVersion,
            (Self::Sys, "maxunicode") => Intrinsic::IntConstant(0x10FFFF),
            (Self::Sys, "hexversion") => Intrinsic::SysHexversion,
            (Self::Sys, "platform") => Intrinsic::SysPlatform,
            (Self::Sys, "maxsize") => Intrinsic::SysMaxSize,
            #[cfg(target_os = "linux")]
            (Self::Select, "epoll") => Intrinsic::ObjectRuntimeExport("select.epoll"),
            #[cfg(target_os = "linux")]
            (Self::Selectors, "EpollSelector") => {
                Intrinsic::ObjectRuntimeExport("selectors.EpollSelector")
            }
            (Self::Select, "poll") => Intrinsic::Readiness(crate::ir::Readiness::New),
            (Self::Select, "select") => Intrinsic::Readiness(crate::ir::Readiness::Select),
            (Self::Select, "POLLIN") => Intrinsic::DescriptorConstant("pollin"),
            (Self::Select, "POLLPRI") => Intrinsic::DescriptorConstant("pollpri"),
            (Self::Select, "POLLOUT") => Intrinsic::DescriptorConstant("pollout"),
            (Self::Select, "POLLERR") => Intrinsic::DescriptorConstant("pollerr"),
            (Self::Select, "POLLHUP") => Intrinsic::DescriptorConstant("pollhup"),
            (Self::Select, "POLLNVAL") => Intrinsic::DescriptorConstant("pollnval"),
            (Self::Os, "name") => Intrinsic::OsName,
            (Self::Os, "linesep") => Intrinsic::OsLinesep,
            (Self::Errno, _) => Intrinsic::ErrnoAttribute,
            (Self::Argparse, "ArgumentParser") => Intrinsic::ARGUMENT_PARSER,
            (Self::Argparse, "Namespace") => Intrinsic::ArgparseNamespace,
            (Self::Argparse, "SUPPRESS") => Intrinsic::ArgparseSuppress,
            (Self::Argparse, "BooleanOptionalAction") => Intrinsic::BooleanOptionalAction,
            (Self::Argparse, "RawDescriptionHelpFormatter") => {
                Intrinsic::RawDescriptionHelpFormatter
            }
            (Self::Builtins, "_") => Intrinsic::BuiltinsLast,
            (Self::Builtins, "len") => Intrinsic::BuiltinsLen,
            (Self::Builtins, "ascii") => Intrinsic::BuiltinsAscii,
            (Self::Builtins, "print") => Intrinsic::BuiltinsPrint,
            (Self::Builtins, "bin") => Intrinsic::ScopedObjectExport("builtins.bin"),
            (Self::Builtins, "hex") => Intrinsic::BuiltinsHex,
            (Self::Builtins, "open") => Intrinsic::BuiltinsOpen,
            (Self::Builtins, "oct") => Intrinsic::ScopedObjectExport("builtins.oct"),
            (Self::Builtins, "exec") => Intrinsic::ScopedObjectExport("builtins.exec"),
            (Self::Sys, "argv") => Intrinsic::Argv,
            (Self::Sys, "orig_argv") => Intrinsic::OrigArgv,
            (Self::Sys, "executable") => Intrinsic::SysExecutable,
            (Self::Sysconfig, "get_path") => Intrinsic::GetSysconfigPath,
            (Self::Sys, "exit") => Intrinsic::SysExit,
            (Self::Sys, "__displayhook__") => Intrinsic::SysDisplayHook,
            (Self::LoggingHandlers, "TimedRotatingFileHandler") => Intrinsic::TimedFileHandler,
            (Self::LoggingHandlers, "QueueHandler") => {
                Intrinsic::ScopedObjectExport("logging.handlers.QueueHandler")
            }
            (Self::LoggingHandlers, "QueueListener") => {
                Intrinsic::ScopedObjectExport("logging.handlers.QueueListener")
            }
            (Self::LoggingHandlers, "SysLogHandler") => {
                Intrinsic::ScopedObjectExport("logging.handlers.SysLogHandler")
            }
            (Self::Datetime, "time") => Intrinsic::ClockTime,
            (Self::Datetime, "date") => Intrinsic::Calendar(crate::ir::CalendarType::Date),
            (Self::Datetime, "datetime") => Intrinsic::Calendar(crate::ir::CalendarType::DateTime),
            (Self::Datetime, "timedelta") => Intrinsic::Calendar(crate::ir::CalendarType::Delta),
            (Self::Datetime, "timezone") => Intrinsic::Calendar(crate::ir::CalendarType::Zone),
            (Self::Datetime, "UTC") => Intrinsic::CalendarUtc,
            (Self::Datetime, "MINYEAR") => Intrinsic::IntConstant(1),
            (Self::Datetime, "MAXYEAR") => Intrinsic::IntConstant(9999),
            (Self::Datetime, "tzinfo") => Intrinsic::ObjectRuntimeExport("datetime.tzinfo"),
            (Self::Logging, "Logger") => Intrinsic::Logger,
            (Self::Logging, "Manager") => Intrinsic::LogManager,
            (Self::Logging, "root") => Intrinsic::LogRoot,
            (Self::Logging, "log") => Intrinsic::LogEmit(None),
            (Self::Logging, "debug") => Intrinsic::LogEmit(Some(10)),
            (Self::Logging, "info") => Intrinsic::LogEmit(Some(20)),
            (Self::Logging, "warning") => Intrinsic::LogEmit(Some(30)),
            (Self::Logging, "error") => Intrinsic::LogEmit(Some(40)),
            (Self::Logging, "critical" | "fatal") => Intrinsic::LogEmit(Some(50)),
            (Self::Logging, "NOTSET") => Intrinsic::LogLevel(0),
            (Self::Logging, "WARNING" | "WARN") => Intrinsic::LogLevel(30),
            (Self::Logging, "ERROR") => Intrinsic::LogLevel(40),
            (Self::Logging, "CRITICAL" | "FATAL") => Intrinsic::LogLevel(50),
            (Self::Shlex, "split") => Intrinsic::ShlexSplit,
            (Self::Shlex, "quote") => Intrinsic::ShlexQuote,
            (Self::Shlex, "join") => Intrinsic::ShlexJoin,
            (Self::Difflib, "ndiff") => Intrinsic::Ndiff,
            (Self::DoctestRuntime, "capture_start") => Intrinsic::DocCaptureStart,
            (Self::DoctestRuntime, "capture_take") => Intrinsic::DocCaptureTake,
            (Self::DoctestRuntime, "display") => Intrinsic::DocDisplay,
            (Self::DoctestRuntime, "exception_name") => Intrinsic::DocExceptionName,
            (Self::DoctestRuntime, "execute") => Intrinsic::DocExecute,
            (Self::MockRuntime, "mark_call_list") => Intrinsic::MockCallList { mark: true },
            (Self::MockRuntime, "is_call_list") => Intrinsic::MockCallList { mark: false },
            (Self::Typing, "Mapping") => Intrinsic::TYPING_MAPPING,
            (Self::Typing, "Protocol") => Intrinsic::TYPING_PROTOCOL,
            (Self::Typing, "Final") => Intrinsic::Final,
            (Self::Typing, "IO") => Intrinsic::TYPING_IO,
            (Self::Typing, "TextIO") => Intrinsic::TextIO,
            (Self::Typing, "BinaryIO") => Intrinsic::BinaryIO,
            (Self::Typing, "Optional") => Intrinsic::Optional,
            (Self::Typing, "Union") => Intrinsic::Union,
            (Self::Typing, "Literal") => Intrinsic::Literal,
            (Self::Typing, "Self") => Intrinsic::SelfType,
            (Self::Typing, "ClassVar") => Intrinsic::ClassVar,
            (Self::Typing, "Any") => Intrinsic::Any,
            (Self::Typing, "assert_never") => Intrinsic::AssertNever,
            (Self::Typing, "cast") => Intrinsic::Cast,
            (Self::Typing | Self::CollectionsAbc, "Callable") => Intrinsic::Callable,
            (Self::Typing, "Sequence") => Intrinsic::TYPING_SEQUENCE,
            (Self::CollectionsAbc, "Mapping") => Intrinsic::Mapping,
            (Self::Typing | Self::CollectionsAbc, "Iterable") => Intrinsic::Iterable,
            (Self::Typing | Self::CollectionsAbc, "Iterator") => Intrinsic::Iterator,
            (Self::Io, "UnsupportedOperation") => Intrinsic::UnsupportedOperation,
            (Self::Gzip, "open") => Intrinsic::GzipOpen,
            (Self::Gzip, "compress") => Intrinsic::ScopedObjectExport("gzip.compress"),
            (Self::Gzip, "decompress") => Intrinsic::ScopedObjectExport("gzip.decompress"),
            (Self::Types, "FunctionType") => Intrinsic::ObjectOnlyExport("types.FunctionType"),
            (Self::Types, "GenericAlias") => Intrinsic::ScopedObjectExport("types.GenericAlias"),
            (Self::Types, "ModuleType") => Intrinsic::ModuleType,
            (Self::Types, "SimpleNamespace") => Intrinsic::SimpleNamespace,
            (Self::Types, "TracebackType") => Intrinsic::NominalClass("traceback"),
            (Self::Types, "FrameType") => Intrinsic::NominalClass("frame"),
            (Self::Types, "CodeType") => Intrinsic::NominalClass("code"),
            (Self::Codecs | Self::CodecsNative, "utf_8_decode") => {
                Intrinsic::ObjectRuntimeExport("codecs.utf_8_decode")
            }
            (Self::Thread | Self::Threading, "get_ident") => {
                Intrinsic::ObjectRuntimeExport("_thread.get_ident")
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Thread | Self::Threading, "get_native_id") => {
                Intrinsic::ObjectRuntimeExport("_thread.get_native_id")
            }
            (Self::Thread | Self::Threading, "TIMEOUT_MAX") => {
                Intrinsic::ObjectRuntimeExport("_thread.TIMEOUT_MAX")
            }
            #[cfg(unix)]
            (Self::Thread, "interrupt_main") => {
                Intrinsic::ObjectRuntimeExport("_thread.interrupt_main")
            }
            (Self::Threading, "Lock") | (Self::Thread, "LockType") => {
                Intrinsic::ObjectRuntimeExport("_thread.lock")
            }
            (Self::Threading, "local") | (Self::Thread, "_local") => {
                Intrinsic::ObjectRuntimeExport("_thread._local")
            }
            (Self::Thread | Self::Functools, "RLock") => {
                Intrinsic::ObjectRuntimeExport("_thread.RLock")
            }
            (Self::WeakRef, "proxy") => Intrinsic::WeakProxy,
            (Self::WeakRef, "ProxyType") => Intrinsic::WeakProxyClass { callable: false },
            (Self::WeakRef, "CallableProxyType") => Intrinsic::WeakProxyClass { callable: true },
            (Self::UnicodeData, "unidata_version") => {
                Intrinsic::StringConstant(imre_ast::UNICODE_DATA_VERSION)
            }
            (Self::UnicodeData, "bidirectional") => {
                Intrinsic::UnicodeProperty(UnicodeProperty::Bidirectional)
            }
            (Self::UnicodeData, "category") => {
                Intrinsic::UnicodeProperty(UnicodeProperty::Category)
            }
            (Self::UnicodeData, "east_asian_width") => {
                Intrinsic::UnicodeProperty(UnicodeProperty::EastAsianWidth)
            }
            (Self::UnicodeData, "decomposition") => {
                Intrinsic::ObjectRuntimeExport("unicodedata.decomposition")
            }
            (Self::UnicodeData, "normalize") => Intrinsic::UnicodeNormalize { check: false },
            (Self::UnicodeData, "is_normalized") => Intrinsic::UnicodeNormalize { check: true },
            (Self::Codecs, "lookup") => Intrinsic::CodecLookup,
            (Self::Codecs, "BOM_UTF8") => Intrinsic::CodecBom(b"\xef\xbb\xbf"),
            (Self::Codecs, "BOM_LE" | "BOM32_LE" | "BOM_UTF16_LE") => {
                Intrinsic::CodecBom(b"\xff\xfe")
            }
            (Self::Codecs, "BOM_BE" | "BOM32_BE" | "BOM_UTF16_BE") => {
                Intrinsic::CodecBom(b"\xfe\xff")
            }
            (Self::Codecs, "BOM64_LE" | "BOM_UTF32_LE") => Intrinsic::CodecBom(b"\xff\xfe\0\0"),
            (Self::Codecs, "BOM64_BE" | "BOM_UTF32_BE") => Intrinsic::CodecBom(b"\0\0\xfe\xff"),
            (Self::Codecs, "BOM" | "BOM_UTF16") => {
                Intrinsic::CodecNativeBom(b"\xff\xfe", b"\xfe\xff")
            }
            (Self::Codecs, "BOM_UTF32") => {
                Intrinsic::CodecNativeBom(b"\xff\xfe\0\0", b"\0\0\xfe\xff")
            }
            (Self::Socket | Self::SocketNative, "herror") => Intrinsic::HError,
            (Self::Socket, "MsgFlag") => Intrinsic::ScopedObjectExport("socket.MsgFlag"),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Socket, "AI_CANONNAME") => Intrinsic::ScopedObjectExport("socket.AI_CANONNAME"),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::SocketNative, "AI_CANONNAME") => {
                Intrinsic::ScopedObjectExport("_socket.AI_CANONNAME")
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Socket, "AI_V4MAPPED") => Intrinsic::ScopedObjectExport("socket.AI_V4MAPPED"),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::SocketNative, "AI_V4MAPPED") => {
                Intrinsic::ScopedObjectExport("_socket.AI_V4MAPPED")
            }
            (Self::Socket, "MSG_PEEK") => Intrinsic::SOCKET_MSG_PEEK,
            (Self::SocketNative, "MSG_PEEK") => Intrinsic::ScopedObjectExport("_socket.MSG_PEEK"),
            (
                Self::Socket | Self::SocketNative,
                "MSG_OOB" | "MSG_DONTROUTE" | "MSG_DONTWAIT" | "MSG_EOR" | "MSG_TRUNC"
                | "MSG_CTRUNC" | "MSG_WAITALL" | "MSG_NOSIGNAL" | "MSG_CMSG_CLOEXEC"
                | "MSG_ERRQUEUE" | "MSG_CONFIRM" | "MSG_MORE" | "MSG_FASTOPEN" | "MSG_EOF",
            ) => Intrinsic::ScopedObjectExport("socket message flags"),
            #[cfg(unix)]
            (Self::Socket | Self::SocketNative, "SO_ERROR") => {
                Intrinsic::IntConstant(i64::from(libc::SO_ERROR))
            }
            #[cfg(target_os = "linux")]
            (Self::Socket | Self::SocketNative, "TCP_QUICKACK") => {
                Intrinsic::IntConstant(i64::from(libc::TCP_QUICKACK))
            }
            #[cfg(target_os = "linux")]
            (Self::Socket, "SOCK_NONBLOCK") => {
                Intrinsic::ScopedObjectExport("socket.SOCK_NONBLOCK")
            }
            #[cfg(target_os = "linux")]
            (Self::SocketNative, "SOCK_NONBLOCK") => {
                Intrinsic::IntConstant(i64::from(libc::SOCK_NONBLOCK))
            }
            #[cfg(target_os = "linux")]
            (Self::Socket, "SOCK_CLOEXEC") => Intrinsic::ScopedObjectExport("socket.SOCK_CLOEXEC"),
            #[cfg(target_os = "linux")]
            (Self::SocketNative, "SOCK_CLOEXEC") => {
                Intrinsic::IntConstant(i64::from(libc::SOCK_CLOEXEC))
            }
            #[cfg(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos"))]
            (Self::Socket, "getprotobyname") => {
                Intrinsic::ScopedObjectExport("socket.getprotobyname")
            }
            #[cfg(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos"))]
            (Self::SocketNative, "getprotobyname") => {
                Intrinsic::ScopedObjectExport("_socket.getprotobyname")
            }
            (Self::Socket, "htonl") => Intrinsic::ScopedObjectExport("socket.htonl"),
            (Self::Socket, "htons") => Intrinsic::ScopedObjectExport("socket.htons"),
            (Self::Socket, "ntohl") => Intrinsic::ScopedObjectExport("socket.ntohl"),
            (Self::Socket, "ntohs") => Intrinsic::ScopedObjectExport("socket.ntohs"),
            (Self::SocketNative, "htonl") => Intrinsic::ScopedObjectExport("_socket.htonl"),
            (Self::SocketNative, "htons") => Intrinsic::ScopedObjectExport("_socket.htons"),
            (Self::SocketNative, "ntohl") => Intrinsic::ScopedObjectExport("_socket.ntohl"),
            (Self::SocketNative, "ntohs") => Intrinsic::ScopedObjectExport("_socket.ntohs"),
            (Self::Socket, "timeout") => Intrinsic::ScopedObjectExport("socket.timeout"),
            (Self::SocketNative, "timeout") => Intrinsic::ScopedObjectExport("_socket.timeout"),
            (Self::SocketNative, "getdefaulttimeout") => {
                Intrinsic::ScopedObjectExport("_socket.getdefaulttimeout")
            }
            (Self::SocketNative, "setdefaulttimeout") => {
                Intrinsic::ScopedObjectExport("_socket.setdefaulttimeout")
            }
            (Self::Socket, "getdefaulttimeout") => {
                Intrinsic::ScopedObjectExport("socket.getdefaulttimeout")
            }
            (Self::Socket, "setdefaulttimeout") => {
                Intrinsic::ScopedObjectExport("socket.setdefaulttimeout")
            }
            (Self::Socket, "_GLOBAL_DEFAULT_TIMEOUT") => {
                Intrinsic::ScopedObjectExport("socket._GLOBAL_DEFAULT_TIMEOUT")
            }
            #[cfg(unix)]
            (Self::Socket, "fromfd") => Intrinsic::ScopedObjectExport("socket.fromfd"),
            (Self::Socket, "SocketIO") => Intrinsic::ScopedObjectExport("socket.SocketIO"),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Socket | Self::SocketNative, "IPV6_RECVHOPLIMIT") => {
                Intrinsic::SocketConstant("ipv6_recvhoplimit")
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Socket | Self::SocketNative, "IPV6_HOPLIMIT") => {
                Intrinsic::SocketConstant("ipv6_hoplimit")
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Socket | Self::SocketNative, "IPV6_RECVTCLASS") => {
                Intrinsic::SocketConstant("ipv6_recvtclass")
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Socket | Self::SocketNative, "IPV6_TCLASS") => {
                Intrinsic::SocketConstant("ipv6_tclass")
            }
            #[cfg(target_os = "linux")]
            (Self::Socket | Self::SocketNative, "IPPROTO_UDPLITE") => {
                Intrinsic::SocketConstant("ipproto_udplite")
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Socket | Self::SocketNative, "SCM_RIGHTS") => {
                Intrinsic::IntConstant(i64::from(libc::SCM_RIGHTS))
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            (Self::Socket | Self::SocketNative, name @ ("CMSG_LEN" | "CMSG_SPACE")) => {
                Intrinsic::SocketControlSize {
                    padded: name == "CMSG_SPACE",
                }
            }
            (Self::Socket, "getfqdn") => Intrinsic::SocketGetFqdn,
            (Self::Socket, "socket") => Intrinsic::Socket,
            (Self::Socket, "gaierror") => Intrinsic::GaiError,
            (Self::Socket, name) => Intrinsic::SocketConstant(match name {
                "AF_INET" => "af_inet",
                "SOCK_STREAM" => "sock_stream",
                "SOL_SOCKET" => "sol_socket",
                "SO_TYPE" => "so_type",
                "SO_REUSEADDR" => "so_reuseaddr",
                "IPPROTO_TCP" => "ipproto_tcp",
                "TCP_NODELAY" => "tcp_nodelay",
                "SOMAXCONN" => "somaxconn",
                "SO_SNDBUF" => "so_sndbuf",
                "SO_RCVBUF" => "so_rcvbuf",
                "SO_LINGER" => "so_linger",
                "SHUT_RD" => "shut_rd",
                "SHUT_WR" => "shut_wr",
                "SHUT_RDWR" => "shut_rdwr",
                _ => return boxed_exports::symbol(self, name),
            }),
            (Self::Codecs, "getwriter") => Intrinsic::CodecGetWriter,
            (Self::Hashlib, "sha1") => Intrinsic::Sha1,
            (Self::Hmac, "compare_digest") => Intrinsic::HmacCompareDigest,
            (Self::Hashlib, "sha256") => Intrinsic::Sha256,
            (Self::Hashlib, "sha224") => Intrinsic::NamedHash("sha224"),
            (Self::Hashlib, "sha384") => Intrinsic::NamedHash("sha384"),
            (Self::Hashlib, "sha512") => Intrinsic::NamedHash("sha512"),
            (Self::Hashlib, "blake2b") => Intrinsic::NamedHash("blake2b"),
            (Self::Hashlib, "blake2s") => Intrinsic::NamedHash("blake2s"),
            (Self::Hashlib, "sha3_224") => Intrinsic::NamedHash("sha3_224"),
            (Self::Hashlib, "sha3_256") => Intrinsic::NamedHash("sha3_256"),
            (Self::Hashlib, "sha3_384") => Intrinsic::NamedHash("sha3_384"),
            (Self::Hashlib, "sha3_512") => Intrinsic::NamedHash("sha3_512"),
            (Self::Hashlib, "shake_128") => Intrinsic::NamedHash("shake_128"),
            (Self::Hashlib, "shake_256") => Intrinsic::NamedHash("shake_256"),
            (Self::Base64, "a85encode") => Intrinsic::ScopedObjectExport("base64.a85encode"),
            (Self::Base64, "a85decode") => Intrinsic::ScopedObjectExport("base64.a85decode"),
            (Self::Base64, "b85encode") => Intrinsic::ScopedObjectExport("base64.b85encode"),
            (Self::Base64, "b85decode") => Intrinsic::ScopedObjectExport("base64.b85decode"),
            (Self::Base64, "z85encode") => Intrinsic::ScopedObjectExport("base64.z85encode"),
            (Self::Base64, "z85decode") => Intrinsic::ScopedObjectExport("base64.z85decode"),
            (Self::Base64, "standard_b64decode") => {
                Intrinsic::ScopedObjectExport("base64.standard_b64decode")
            }
            (Self::Base64, "urlsafe_b64decode") => {
                Intrinsic::ScopedObjectExport("base64.urlsafe_b64decode")
            }
            (Self::Base64, "urlsafe_b64encode") => Intrinsic::UrlsafeB64encode,
            (Self::Base64, "encodebytes") => Intrinsic::ScopedObjectExport("base64.encodebytes"),
            (Self::Base64, "b64encode") => Intrinsic::Base64Encode,
            (Self::Base64, "b64decode") => Intrinsic::Base64Decode,
            (Self::Base64, "b16encode") => Intrinsic::ScopedObjectExport("base64.b16encode"),
            (Self::Base64, "b16decode") => Intrinsic::ScopedObjectExport("base64.b16decode"),
            (Self::Base64, "b32encode") => Intrinsic::ScopedObjectExport("base64.b32encode"),
            (Self::Base64, "b32decode") => Intrinsic::ScopedObjectExport("base64.b32decode"),
            (Self::Base64, "b32hexencode") => Intrinsic::ScopedObjectExport("base64.b32hexencode"),
            (Self::Base64, "b32hexdecode") => Intrinsic::ScopedObjectExport("base64.b32hexdecode"),
            (Self::Binascii, "unhexlify") => Intrinsic::BinasciiUnhexlify("binascii.unhexlify"),
            (Self::Binascii, "a2b_hex") => Intrinsic::BinasciiUnhexlify("binascii.a2b_hex"),
            (Self::Binascii, "a2b_base64") => Intrinsic::BinasciiDecode,
            (Self::Binascii, "b2a_base64") => Intrinsic::BinasciiEncode,
            (Self::Binascii, "Error") => Intrinsic::BinasciiError,
            (Self::Binascii, "Incomplete") => Intrinsic::BinasciiIncomplete,
            (Self::Hashlib, "file_digest") => Intrinsic::HashFileDigest,
            (Self::Hashlib, "new") => Intrinsic::HashNew,
            (Self::Hashlib, "algorithms_available") => {
                Intrinsic::HashAlgorithms { guaranteed: false }
            }
            (Self::Hashlib, "algorithms_guaranteed") => {
                Intrinsic::HashAlgorithms { guaranteed: true }
            }
            (Self::Zipfile, "ZIP_STORED") => Intrinsic::IntConstant(0),
            (Self::Zipfile, "ZIP_DEFLATED") => Intrinsic::IntConstant(8),
            (Self::Zipfile, "ZIP_BZIP2") => Intrinsic::IntConstant(12),
            (Self::Zipfile, "ZIP_LZMA") => Intrinsic::IntConstant(14),
            (Self::Zipfile, "ZIP_ZSTANDARD") => Intrinsic::IntConstant(93),
            (Self::Zipfile, "ZipFile") => Intrinsic::ZipFile,
            (Self::Zipfile, "ZipInfo") => Intrinsic::ZipInfo,
            (Self::Io | Self::IoNative, "BlockingIOError") => {
                Intrinsic::ObjectRuntimeExport("io.BlockingIOError")
            }
            (Self::Io, "SEEK_SET") => Intrinsic::IntConstant(0),
            (Self::Io, "SEEK_CUR") => Intrinsic::IntConstant(1),
            (Self::Io, "SEEK_END") => Intrinsic::IntConstant(2),
            (Self::Io | Self::IoNative, "text_encoding") => {
                Intrinsic::ObjectRuntimeExport("_io.text_encoding")
            }
            (Self::Io | Self::IoNative, "open_code") => {
                Intrinsic::ObjectRuntimeExport("io.open_code")
            }
            (Self::Io, "BytesIO") => Intrinsic::BytesIo,
            (Self::Io, "IOBase") => Intrinsic::IO_BASE,
            (Self::Io, "RawIOBase") => Intrinsic::RAW_IO_BASE,
            (Self::Io, "BufferedIOBase") => Intrinsic::BUFFERED_IO_BASE,
            (Self::Io, "TextIOBase") => Intrinsic::TEXT_IO_BASE,
            (Self::Io, "StringIO") => Intrinsic::StringIo,
            (Self::Zipfile, "BadZipFile") => Intrinsic::BadZipFile,
            (Self::Csv, "reader") => Intrinsic::CsvReader,
            (Self::StringFormatting, "formatter_parser") => Intrinsic::FormatterParser,
            (Self::StringFormatting, "formatter_field_name_split") => {
                Intrinsic::FormatterFieldNameSplit
            }
            (Self::Csv, "Error") => Intrinsic::CsvError,
            (Self::Gzip, "BadGzipFile") => Intrinsic::BadGzipFile,
            (Self::ReConstants, "MAXGROUPS") => {
                Intrinsic::IntConstant(i64::from(imre_regex::MAX_GROUPS))
            }
            (Self::Re, "compile") => Intrinsic::RegexCompile,
            (Self::Re, "search") => Intrinsic::RegexSearch,
            (Self::Re, "match") => Intrinsic::RegexMatchStart,
            (Self::Re, "sub") => Intrinsic::RegexSub,
            (Self::Re, "findall") => Intrinsic::RegexRuns { split: false },
            (Self::Re, "split") => Intrinsic::RegexRuns { split: true },
            (Self::Re, "DEBUG") => Intrinsic::REGEX_DEBUG,
            (Self::Re, "VERBOSE") => Intrinsic::RegexVerbose,
            (Self::Re, "UNICODE") => Intrinsic::RegexUnicode,
            (Self::Re, "ASCII" | "A") => Intrinsic::RegexAscii,
            (Self::Re, "MULTILINE" | "M") => Intrinsic::RegexMultiline,
            (Self::Re, "Pattern") => Intrinsic::RegexPattern,
            (Self::Re, "Match") => Intrinsic::RegexMatch,
            (Self::Enum, "StrEnum") => Intrinsic::StrEnum,
            (Self::Enum, "IntEnum") => Intrinsic::IntEnum,
            (Self::Enum, "_simple_enum") => Intrinsic::SimpleEnum,
            (Self::Enum, "auto") => Intrinsic::EnumAuto,
            (Self::Collections | Self::CollectionsNative, "_count_elements") => {
                Intrinsic::CountElements
            }
            (Self::Collections | Self::CollectionsNative, "defaultdict") => Intrinsic::DefaultDict,
            (Self::Collections | Self::CollectionsNative, "deque") => Intrinsic::Deque,
            (Self::Collections | Self::CollectionsNative, "_deque_iterator") => {
                Intrinsic::ObjectRuntimeExport("collections._deque_iterator")
            }
            (Self::CollectionsNative, "_deque_reverse_iterator") => {
                Intrinsic::ObjectRuntimeExport("_collections._deque_reverse_iterator")
            }
            (Self::Collections, "ChainMap") => Intrinsic::ChainMap,
            (Self::Collections | Self::CollectionsNative, "OrderedDict") => Intrinsic::OrderedDict,
            (Self::Collections, "namedtuple") => Intrinsic::NamedTuple,
            (Self::Collections, "Counter") => Intrinsic::Counter,
            (Self::OperatorNative, "_compare_digest") => {
                Intrinsic::ObjectRuntimeExport("_operator._compare_digest")
            }
            (Self::Operator | Self::OperatorNative, "add" | "__add__") => {
                Intrinsic::ObjectRuntimeExport("operator.add")
            }
            (Self::Operator | Self::OperatorNative, "is_") => {
                Intrinsic::ScopedObjectExport("operator.is_")
            }
            (Self::Operator | Self::OperatorNative, "is_not") => {
                Intrinsic::ScopedObjectExport("operator.is_not")
            }
            (Self::Operator, "attrgetter") => Intrinsic::AttrGetter,
            (Self::Operator, "itemgetter") => Intrinsic::ObjectRuntimeExport("operator.itemgetter"),
            (Self::Operator | Self::OperatorNative, "methodcaller") => {
                Intrinsic::ScopedObjectExport("operator.methodcaller")
            }
            (Self::Operator, "length_hint") => Intrinsic::LengthHint,
            (Self::Operator | Self::OperatorNative, "call") | (Self::Operator, "__call__") => {
                Intrinsic::OperatorCall
            }
            (Self::Operator | Self::OperatorNative, "not_") | (Self::Operator, "__not__") => {
                Intrinsic::OperatorNot
            }
            (Self::Gc, "is_tracked") => Intrinsic::ScopedObjectExport("gc.is_tracked"),
            (Self::Math, "acos") => Intrinsic::MathAcos,
            (Self::Math, "acosh") => Intrinsic::ScopedObjectExport("math.acosh"),
            (Self::Math, "asin") => Intrinsic::MathAsin,
            (Self::Math, "asinh") => Intrinsic::ScopedObjectExport("math.asinh"),
            (Self::Math, "atan") => Intrinsic::MathAtan,
            (Self::Math, "atan2") => Intrinsic::ScopedObjectExport("math.atan2"),
            (Self::Math, "atanh") => Intrinsic::ScopedObjectExport("math.atanh"),
            (Self::Math, "perm") => Intrinsic::MathPerm,
            (Self::Math, "factorial") => Intrinsic::ScopedObjectExport("math.factorial"),
            (Self::Math, "comb") => Intrinsic::ScopedObjectExport("math.comb"),
            (Self::Math, "cosh") => Intrinsic::ScopedObjectExport("math.cosh"),
            (Self::Math, "degrees") => Intrinsic::MathDegrees,
            (Self::Math, "fmod") => Intrinsic::MathFmod,
            (Self::Math, "radians") => Intrinsic::MathRadians,
            (Self::Math, "erfc") => Intrinsic::MathErfc,
            (Self::Math, "exp") => Intrinsic::ScopedObjectExport("math.exp"),
            (Self::Math, "fabs") => Intrinsic::ScopedObjectExport("math.fabs"),
            (Self::Math, "fma") => Intrinsic::ScopedObjectExport("math.fma"),
            (Self::Math, "frexp") => Intrinsic::ScopedObjectExport("math.frexp"),
            (Self::Math, "fsum") => Intrinsic::ScopedObjectExport("math.fsum"),
            (Self::Math, "dist") => Intrinsic::ScopedObjectExport("math.dist"),
            (Self::Math, "hypot") => Intrinsic::ScopedObjectExport("math.hypot"),
            (Self::Math, "isclose") => Intrinsic::ScopedObjectExport("math.isclose"),
            (Self::Math, "isfinite") => Intrinsic::ScopedObjectExport("math.isfinite"),
            (Self::Math, "lcm") => Intrinsic::ScopedObjectExport("math.lcm"),
            (Self::Math, "log10") => Intrinsic::MathLog10,
            (Self::Math, "log1p") => Intrinsic::ScopedObjectExport("math.log1p"),
            (Self::Math, "log2") => Intrinsic::ScopedObjectExport("math.log2"),
            (Self::Math, "ldexp") => Intrinsic::ScopedObjectExport("math.ldexp"),
            (Self::Math, "nextafter") => Intrinsic::ScopedObjectExport("math.nextafter"),
            (Self::Math, "pow") => Intrinsic::ScopedObjectExport("math.pow"),
            (Self::Math, "prod") => Intrinsic::ScopedObjectExport("math.prod"),
            (Self::Math, "sumprod") => Intrinsic::ScopedObjectExport("math.sumprod"),
            (Self::Math, "remainder") => Intrinsic::ScopedObjectExport("math.remainder"),
            (Self::Math, "sin") => Intrinsic::ScopedObjectExport("math.sin"),
            (Self::Math, "sinh") => Intrinsic::ScopedObjectExport("math.sinh"),
            (Self::Math, "sqrt") => Intrinsic::ScopedObjectExport("math.sqrt"),
            (Self::Math, "tan") => Intrinsic::ScopedObjectExport("math.tan"),
            (Self::Math, "tanh") => Intrinsic::ScopedObjectExport("math.tanh"),
            (Self::Math, "ulp") => Intrinsic::ScopedObjectExport("math.ulp"),
            (Self::Math, "pi") => Intrinsic::ScopedObjectExport("math.pi"),
            (Self::Math, "e") => Intrinsic::ScopedObjectExport("math.e"),
            (Self::Math, "tau") => Intrinsic::ScopedObjectExport("math.tau"),
            (Self::Math, "inf") => Intrinsic::ScopedObjectExport("math.inf"),
            (Self::Math, "nan") => Intrinsic::ScopedObjectExport("math.nan"),
            (Self::Copy, "copy") => Intrinsic::ShallowCopy,
            (Self::Copy, "deepcopy") => Intrinsic::DeepCopy,
            (Self::Itertools, "product") => {
                Intrinsic::Combinatorial(crate::ir::Combinatorial::Product)
            }
            (Self::Itertools, "permutations") => {
                Intrinsic::Combinatorial(crate::ir::Combinatorial::Permutations)
            }
            (Self::Itertools, "combinations") => {
                Intrinsic::Combinatorial(crate::ir::Combinatorial::Combinations)
            }
            (Self::Itertools, "combinations_with_replacement") => {
                Intrinsic::Combinatorial(crate::ir::Combinatorial::CombinationsWithReplacement)
            }
            (Self::Itertools, "accumulate") => {
                Intrinsic::ObjectRuntimeExport("itertools.accumulate")
            }
            (Self::Itertools, "count") => Intrinsic::Count,
            (Self::Itertools, "tee") => Intrinsic::Tee,
            (Self::Itertools, "islice") => Intrinsic::Islice,
            (Self::Itertools, "cycle") => Intrinsic::Cycle,
            (Self::Itertools, "repeat") => Intrinsic::Repeat,
            (Self::Itertools, "groupby") => Intrinsic::GroupBy,
            (Self::Itertools, "chain") => Intrinsic::Chain,
            (Self::CollectionsAbc, "Set") => Intrinsic::AbstractSet,
            (Self::Tomllib, "load") => Intrinsic::TomllibLoad,
            (Self::Tomllib, "TOMLDecodeError") => Intrinsic::TomlDecodeError,
            (Self::Textwrap, "dedent") => Intrinsic::Dedent,
            (Self::Zlib, "compressobj") => Intrinsic::ScopedObjectExport("zlib.compressobj"),
            (Self::Zlib, "ZLIB_VERSION" | "ZLIB_RUNTIME_VERSION") => {
                Intrinsic::StringConstant(imre_ast::zlib::VERSION)
            }
            (Self::Lzma, "__all__") => Intrinsic::ScopedObjectExport("lzma.__all__"),
            (Self::Lzma, "LZMACompressor") => Intrinsic::ScopedObjectExport("lzma.LZMACompressor"),
            (Self::Lzma, "LZMADecompressor") => {
                Intrinsic::ScopedObjectExport("lzma.LZMADecompressor")
            }
            (Self::Lzma, "LZMAError") => Intrinsic::ScopedObjectExport("lzma.LZMAError"),
            (Self::Lzma, "LZMAFile") => Intrinsic::ScopedObjectExport("lzma.LZMAFile"),
            (Self::Lzma, "compress") => Intrinsic::ScopedObjectExport("lzma.compress"),
            (Self::Lzma, "decompress") => Intrinsic::ScopedObjectExport("lzma.decompress"),
            (Self::Lzma, "is_check_supported") => {
                Intrinsic::ScopedObjectExport("lzma.is_check_supported")
            }
            (Self::Lzma, "open") => Intrinsic::ScopedObjectExport("lzma.open"),
            (Self::Lzma, "_decode_filter_properties") => {
                Intrinsic::ScopedObjectExport("lzma._decode_filter_properties")
            }
            (Self::Lzma, "_encode_filter_properties") => {
                Intrinsic::ScopedObjectExport("lzma._encode_filter_properties")
            }
            (Self::Zlib, "decompressobj") => Intrinsic::ScopedObjectExport("zlib.decompressobj"),
            (Self::Zlib, "error") => Intrinsic::ZlibError,
            (Self::Zlib, "adler32") => Intrinsic::Adler32,
            (Self::ImreJson, "decode") => Intrinsic::JsonDecode,
            (Self::Json, "loads") => Intrinsic::JsonLoads,
            (Self::Json | Self::JsonDecoder, "JSONDecoder") => Intrinsic::JsonDecoderClass,
            (Self::Json | Self::JsonEncoder, "JSONEncoder") => Intrinsic::JsonEncoderClass,
            (Self::Json, "load") => Intrinsic::JsonLoad,
            (Self::Json, "dumps") => Intrinsic::JsonDumps,
            (Self::Json, "dump") => Intrinsic::JsonDump,
            (Self::Json, "JSONDecodeError") => Intrinsic::JsonDecodeError,
            (Self::ImreJson, "encode") => Intrinsic::JsonEncode,
            (Self::ImreToml, "decode") => Intrinsic::TomlDecode,
            (Self::Imre, "struct") => Intrinsic::Struct,
            (Self::Dataclasses, "dataclass") => Intrinsic::Dataclass,
            (Self::Dataclasses, "field") => Intrinsic::DataclassField,
            (Self::Dataclasses, "fields") => Intrinsic::ObjectOnlyExport("dataclasses.fields"),
            (Self::Dataclasses, "asdict") => Intrinsic::AsDict,
            (Self::Pathlib, "Path" | "PosixPath") => Intrinsic::Path,
            (Self::Pathlib, "PurePath" | "PurePosixPath") => {
                Intrinsic::PurePath(crate::ir::PathFlavor::Posix)
            }
            (Self::Pathlib, "PureWindowsPath") => {
                Intrinsic::PurePath(crate::ir::PathFlavor::Windows)
            }
            (Self::Random, "shuffle") => Intrinsic::Random(crate::ir::RandomOperation::Shuffle),
            (Self::Random, "randbytes") => Intrinsic::ScopedObjectExport("random.randbytes"),
            (Self::Random, "sample") => Intrinsic::ScopedObjectExport("random.sample"),
            (Self::Random, "Random") => Intrinsic::ScopedObjectExport("random.Random"),
            (Self::Random, "uniform") => Intrinsic::ScopedObjectExport("random.uniform"),
            (Self::Random, "expovariate") => Intrinsic::ScopedObjectExport("random.expovariate"),
            (Self::Random, "choices") => Intrinsic::ObjectRuntimeExport("random.choices"),
            (Self::Random, "choice") => Intrinsic::RandomChoice { secure: false },
            (Self::Secrets, "choice") => Intrinsic::RandomChoice { secure: true },
            (Self::Random, "seed") => Intrinsic::Random(crate::ir::RandomOperation::Seed),
            (Self::Random, "random") => Intrinsic::Random(crate::ir::RandomOperation::Float),
            (Self::Random, "getrandbits") => Intrinsic::Random(crate::ir::RandomOperation::Bits),
            (Self::Random, "randint") => Intrinsic::Random(crate::ir::RandomOperation::Randint),
            (Self::Random, "randrange") => Intrinsic::Random(crate::ir::RandomOperation::Randrange),
            (Self::Shutil, "rmtree") => Intrinsic::RmTree,
            (Self::Shutil, "copytree") => Intrinsic::CopyTree,
            (Self::Shutil, "move") => Intrinsic::Move,
            (Self::Shutil, "_destinsrc") => Intrinsic::DestInSrc,
            (Self::Shutil, "ignore_patterns") => Intrinsic::IgnorePatterns,
            (Self::Shutil, "which") => Intrinsic::Which,
            (Self::Shutil, "copyfile") => Intrinsic::Copy(CopyOperation::File),
            (Self::Shutil, "copymode") => Intrinsic::Copy(CopyOperation::Mode),
            (Self::Shutil, "copystat") => Intrinsic::Copy(CopyOperation::Stat),
            (Self::Shutil, "_copyxattr") => Intrinsic::Copy(CopyOperation::Xattr),
            (Self::Shutil, "copy") => Intrinsic::Copy(CopyOperation::Copy),
            (Self::Shutil, "copy2") => Intrinsic::Copy(CopyOperation::Copy2),
            (Self::Shutil, "Error") => Intrinsic::ShutilError,
            (Self::Shutil, "SameFileError") => Intrinsic::SameFileError,
            (Self::Shutil, "SpecialFileError") => Intrinsic::SpecialFileError,
            (Self::Tempfile, "TemporaryDirectory") => Intrinsic::TemporaryDirectory,
            (Self::Tempfile, "TemporaryFile") => Intrinsic::TemporaryFile,
            (Self::Subprocess, "STDOUT") => Intrinsic::SubprocessStdout,
            (Self::Subprocess, "Popen") => Intrinsic::Popen,
            (Self::Subprocess, "_active") => Intrinsic::PopenActive,
            (Self::Subprocess, "_cleanup") => Intrinsic::PopenCleanup,
            (Self::Subprocess, "_PopenSelector") => Intrinsic::PopenSelector,
            (Self::Selectors, "SelectSelector") => Intrinsic::SelectorClass(true),
            (Self::Selectors, "PollSelector") => Intrinsic::SelectorClass(false),
            (Self::Subprocess, "run") => Intrinsic::SubprocessRun,
            (Self::Subprocess, "call") => Intrinsic::SubprocessCall,
            (Self::Subprocess, "check_call") => Intrinsic::SubprocessCheckCall,
            (Self::Subprocess, "list2cmdline") => Intrinsic::SubprocessList2Cmdline,
            (Self::Subprocess, "CompletedProcess") => Intrinsic::CompletedProcess,
            (Self::Subprocess, "SubprocessError") => Intrinsic::SubprocessError,
            (Self::Subprocess, "CalledProcessError") => Intrinsic::CalledProcessError,
            (Self::Subprocess, "TimeoutExpired") => Intrinsic::TimeoutExpired,
            (Self::Posix, "_path_splitroot_ex") | (Self::PosixPath, "splitroot") => {
                Intrinsic::ObjectRuntimeExport("posix._path_splitroot_ex")
            }
            (Self::Posix, name) => os::symbol(name)?,
            (Self::Os, "makedirs") if cfg!(unix) => Intrinsic::ObjectRuntimeExport("os.makedirs"),
            (Self::Os, "removedirs") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.removedirs")
            }
            (Self::Os, "execvp") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.execvp")
            }
            (Self::Os, "execvpe") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.execvpe")
            }
            (Self::Os, "_execvpe") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os._execvpe")
            }
            (Self::Os, "spawnv") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.spawnv")
            }
            (Self::Os, "spawnve") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.spawnve")
            }
            (Self::Os, "spawnvp") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.spawnvp")
            }
            (Self::Os, "spawnvpe") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.spawnvpe")
            }
            (Self::Os, "spawnl") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.spawnl")
            }
            (Self::Os, "spawnle") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.spawnle")
            }
            (Self::Os, "spawnlp") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.spawnlp")
            }
            (Self::Os, "spawnlpe") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.spawnlpe")
            }
            (Self::Os, "P_WAIT") if cfg!(target_os = "linux") => Intrinsic::IntConstant(0),
            (Self::Os, "P_NOWAIT" | "P_NOWAITO") if cfg!(target_os = "linux") => {
                Intrinsic::IntConstant(1)
            }
            (Self::Os, "process_cpu_count") if cfg!(target_os = "linux") => {
                Intrinsic::ObjectRuntimeExport("os.process_cpu_count")
            }
            (Self::Os, "walk") if cfg!(unix) => Intrinsic::ObjectRuntimeExport("os.walk"),
            (Self::Os, "fwalk") if cfg!(unix) => Intrinsic::ObjectRuntimeExport("os.fwalk"),
            (Self::Os, "F_OK") => Intrinsic::AccessMode(0),
            (Self::Os, "X_OK") => Intrinsic::AccessMode(1),
            (Self::Os, "W_OK") => Intrinsic::AccessMode(2),
            (Self::Os, "R_OK") => Intrinsic::AccessMode(4),
            (Self::Os, "supports_dir_fd") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.supports_dir_fd")
            }
            (Self::Os, "supports_fd") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.supports_fd")
            }
            (Self::Os, "supports_follow_symlinks") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.supports_follow_symlinks")
            }
            (Self::Os, "supports_effective_ids") if cfg!(unix) => {
                Intrinsic::ObjectRuntimeExport("os.supports_effective_ids")
            }
            (Self::Os, "environ") => Intrinsic::Environ,
            (Self::Os, "environb") if cfg!(unix) => Intrinsic::EnvironBytes,
            (Self::Os, "supports_bytes_environ") => Intrinsic::BoolConstant(cfg!(unix)),
            (Self::Os, "getenv") => Intrinsic::GetEnv,
            (Self::Os, "getenvb") if cfg!(unix) => Intrinsic::ScopedObjectExport("os.getenvb"),
            (Self::Os, "getcwd") => Intrinsic::GetCwd { bytes: false },
            (Self::Os, "getcwdb") if cfg!(unix) => Intrinsic::GetCwd { bytes: true },
            (Self::Os, "fspath") => Intrinsic::FsPath,
            (Self::Os, "_fspath") if cfg!(unix) => Intrinsic::ObjectRuntimeExport("os._fspath"),
            (Self::Os, "fsdecode") => Intrinsic::FsDecode,
            (Self::Os, "fsencode") => Intrinsic::FsEncode,
            (Self::PosixPath, "normcase") => Intrinsic::PosixNormcase,
            (Self::Os, "devnull") => Intrinsic::StringConstant("/dev/null"),
            (Self::Os | Self::OsPath | Self::PosixPath, "extsep") => Intrinsic::StringConstant("."),
            (Self::Os | Self::OsPath | Self::PosixPath, "pardir") => {
                Intrinsic::StringConstant("..")
            }
            (Self::GenericPath | Self::PosixPath, "commonprefix") => {
                Intrinsic::ObjectRuntimeExport("genericpath.commonprefix")
            }
            (Self::GenericPath | Self::PosixPath, "getatime") => {
                Intrinsic::ObjectRuntimeExport("genericpath.getatime")
            }
            (Self::GenericPath | Self::PosixPath, "getctime") => {
                Intrinsic::ObjectRuntimeExport("genericpath.getctime")
            }
            (Self::GenericPath | Self::PosixPath, "getmtime") => {
                Intrinsic::ObjectRuntimeExport("genericpath.getmtime")
            }
            (Self::GenericPath | Self::PosixPath, "isjunction") => {
                Intrinsic::ObjectRuntimeExport("genericpath.isjunction")
            }
            (Self::GenericPath | Self::PosixPath, "lexists") => {
                Intrinsic::ObjectRuntimeExport("genericpath.lexists")
            }
            (Self::GenericPath | Self::PosixPath, "getsize") => {
                Intrinsic::ObjectRuntimeExport("genericpath.getsize")
            }
            (Self::GenericPath | Self::PosixPath, "samefile") => {
                Intrinsic::ObjectRuntimeExport("genericpath.samefile")
            }
            (Self::GenericPath | Self::PosixPath, "sameopenfile") => {
                Intrinsic::ObjectRuntimeExport("genericpath.sameopenfile")
            }
            (Self::GenericPath | Self::PosixPath, "samestat") => {
                Intrinsic::ObjectRuntimeExport("genericpath.samestat")
            }
            (Self::GenericPath | Self::PosixPath, "exists") => Intrinsic::OsPathExists,
            (Self::PosixPath, "commonpath") => Intrinsic::PosixPath(PosixPathOperation::Commonpath),
            (Self::PosixPath, "join") => Intrinsic::PosixPath(PosixPathOperation::Join),
            (Self::PosixPath, "basename") => Intrinsic::PosixPath(PosixPathOperation::Basename),
            (Self::PosixPath, "dirname") => Intrinsic::PosixPath(PosixPathOperation::Dirname),
            (Self::PosixPath, "splitext") => Intrinsic::PosixPath(PosixPathOperation::Splitext),
            (Self::PosixPath, "normpath") => Intrinsic::PosixPath(PosixPathOperation::Normpath),
            (Self::PosixPath, "abspath") => Intrinsic::PosixPath(PosixPathOperation::Abspath),
            (Self::UrllibRobotParser, "RobotFileParser") => {
                Intrinsic::ScopedObjectExport("urllib.robotparser.RobotFileParser")
            }
            (Self::UrllibRequest, "OpenerDirector") => {
                Intrinsic::ScopedObjectExport("urllib.request.OpenerDirector")
            }
            (Self::UrllibRequest, "HTTPPasswordMgr") => {
                Intrinsic::ScopedObjectExport("urllib.request.HTTPPasswordMgr")
            }
            (Self::UrllibRequest, "HTTPPasswordMgrWithDefaultRealm") => {
                Intrinsic::ScopedObjectExport("urllib.request.HTTPPasswordMgrWithDefaultRealm")
            }
            (Self::UrllibRequest, "HTTPPasswordMgrWithPriorAuth") => {
                Intrinsic::ScopedObjectExport("urllib.request.HTTPPasswordMgrWithPriorAuth")
            }
            (Self::UrllibRequest, "AbstractBasicAuthHandler") => {
                Intrinsic::ScopedObjectExport("urllib.request.AbstractBasicAuthHandler")
            }
            (Self::UrllibRequest, "HTTPBasicAuthHandler") => {
                Intrinsic::ScopedObjectExport("urllib.request.HTTPBasicAuthHandler")
            }
            (Self::UrllibRequest, "ProxyBasicAuthHandler") => {
                Intrinsic::ScopedObjectExport("urllib.request.ProxyBasicAuthHandler")
            }
            (Self::UrllibRequest, "AbstractDigestAuthHandler") => {
                Intrinsic::ScopedObjectExport("urllib.request.AbstractDigestAuthHandler")
            }
            (Self::UrllibRequest, "HTTPCookieProcessor") => {
                Intrinsic::ScopedObjectExport("urllib.request.HTTPCookieProcessor")
            }
            (Self::UrllibRequest, "ProxyHandler") => {
                Intrinsic::ScopedObjectExport("urllib.request.ProxyHandler")
            }
            (Self::UrllibRequest, "ProxyDigestAuthHandler") => {
                Intrinsic::ScopedObjectExport("urllib.request.ProxyDigestAuthHandler")
            }
            (Self::UrllibRequest, "HTTPDigestAuthHandler") => {
                Intrinsic::ScopedObjectExport("urllib.request.HTTPDigestAuthHandler")
            }
            (Self::UrllibRequest, "Request") => {
                Intrinsic::ScopedObjectExport("urllib.request.Request")
            }
            (Self::UrllibRequest, "pathname2url") => {
                Intrinsic::ScopedObjectExport("urllib.request.pathname2url")
            }
            (Self::UrllibRequest, "urlopen") => {
                Intrinsic::ScopedObjectExport("urllib.request.urlopen")
            }
            (Self::UrllibRequest, "urlretrieve") => {
                Intrinsic::ScopedObjectExport("urllib.request.urlretrieve")
            }
            (Self::UrllibRequest, "urlcleanup") => {
                Intrinsic::ScopedObjectExport("urllib.request.urlcleanup")
            }
            (Self::UrllibRequest, "build_opener") => {
                Intrinsic::ScopedObjectExport("urllib.request.build_opener")
            }
            (Self::UrllibRequest, "_parse_proxy") => {
                Intrinsic::ScopedObjectExport("urllib.request._parse_proxy")
            }
            (Self::UrllibRequest, "_proxy_bypass_macosx_sysconf") => {
                Intrinsic::ScopedObjectExport("urllib.request._proxy_bypass_macosx_sysconf")
            }
            (Self::UrllibRequest, "_proxy_bypass_winreg_override") => {
                Intrinsic::ScopedObjectExport("urllib.request._proxy_bypass_winreg_override")
            }
            (Self::UrllibRequest, "install_opener") => {
                Intrinsic::ScopedObjectExport("urllib.request.install_opener")
            }
            (Self::UrllibParse, "quote") => Intrinsic::Quote,
            (Self::UrllibParse, "quote_from_bytes") => Intrinsic::QuoteFromBytes,
            (Self::UrllibParse, "quote_plus") => Intrinsic::QuotePlus,
            (Self::UrllibParse, "urlencode") => Intrinsic::UrlEncode,
            (Self::UrllibParse, "unquote") => Intrinsic::Unquote,
            (Self::EmailUtils, "parsedate_to_datetime") => Intrinsic::EmailParseDate,
            (Self::EmailUtils, "format_datetime") => Intrinsic::EmailFormatDateTime,
            (Self::Email, "message_from_bytes") => {
                Intrinsic::ScopedObjectExport("email.message_from_bytes")
            }
            (Self::Email, "message_from_string") => {
                Intrinsic::ScopedObjectExport("email.message_from_string")
            }
            (Self::Email, "message_from_binary_file") => {
                Intrinsic::ScopedObjectExport("email.message_from_binary_file")
            }
            (Self::EmailFeedParser, "FeedParser") => {
                Intrinsic::ScopedObjectExport("email.feedparser.FeedParser")
            }
            (Self::EmailParser, "FeedParser") => {
                Intrinsic::ScopedObjectExport("email.parser.FeedParser")
            }
            (Self::EmailParser, "Parser") => Intrinsic::ScopedObjectExport("email.parser.Parser"),
            (Self::EmailUtils, "formatdate") => Intrinsic::EmailFormatDate,
            (Self::UrllibParse, "urljoin") => Intrinsic::UrlJoin,
            (Self::UrllibParse, "parse_qsl") => Intrinsic::ParseQsl,
            (Self::UrllibParse, "urlsplit") => Intrinsic::UrlSplit,
            (Self::UrllibParse, "urlunsplit") => Intrinsic::UrlUnsplit,
            (Self::UrllibParse, "SplitResult") => Intrinsic::SplitResult { bytes: false },
            (Self::UrllibParse, "SplitResultBytes") => Intrinsic::SplitResult { bytes: true },
            _ => {
                return boxed_exports::symbol(self, name)
                    .or_else(|| declarations::symbol(self, name));
            }
        };
        Some(Binding::Intrinsic(symbol))
    }
}

pub(crate) enum TypeAlias {
    Expression(ast::Expr),
    NewType(ast::Expr),
    /// A PEP 695 declaration. The checked initializer creates the lazy alias object.
    Declaration(usize),
}

impl TypeAlias {
    pub(crate) fn annotation_value<'a>(
        &'a self,
        module: &'a SourceModule,
    ) -> Option<&'a ast::Expr> {
        match self {
            Self::Expression(value) | Self::NewType(value) => Some(value),
            Self::Declaration(statement) => {
                let declaration = module.parsed.syntax().body[*statement].as_type_alias_stmt()?;
                declaration
                    .type_params
                    .is_none()
                    .then_some(&declaration.value)
            }
        }
    }
}

pub(crate) struct SourceModule {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) source: String,
    /// Physical text when newline normalization changed the source for analysis.
    original_source: Option<Box<str>>,
    pub(crate) raw_digest: Option<[u8; 32]>,
    doctest_documents: std::sync::OnceLock<Vec<doctest::Docstring>>,
    registered_values: std::sync::OnceLock<Box<crate::frontend::RegisteredModuleSource>>,
    pub(crate) doctest_prompts: Vec<doctest::Prompt>,
    pub(crate) bundled: bool,
    /// The registered source initializer publishes this module's live attributes.
    pub(crate) registered_namespace: bool,
    pub(crate) parsed: ParsedModuleRef,
    pub(crate) bindings: HashMap<String, Binding>,
    type_bindings: HashMap<String, Binding>,
    /// Only an executed explicit submodule import populates the bare importlib namespace.
    importlib_children: HashMap<&'static str, TextSize>,
    local_static_imports: Vec<LocalStaticImport>,
    pub(crate) aliases: Vec<TypeAlias>,
    /// Decorated declarations register their original function at module initialization.
    pub(crate) overloads: HashSet<usize>,
    pub(crate) type_guards: Vec<usize>,
    /// Entry bindings and guards that run during initialization, before later definitions.
    pub(crate) entry_globals: Vec<usize>,
    /// Names that may be bound by each optional import when the object importer executes it.
    pub(crate) optional_imports: HashMap<TextRange, Vec<String>>,
    /// First physical binding of each mutable module value.
    pub(crate) rebound_values: HashSet<usize>,
    /// The real initializer must bind these imports even if the callable is never read.
    pub(crate) registered_imports: HashMap<usize, TextRange>,
    /// Mutable exports read at the source from-import, keyed by statement.
    pub(crate) snapshots: HashMap<usize, ImportSnapshot>,
    /// Executable entry statements, or inactive main guards that still require checking.
    pub(crate) executable: Vec<usize>,
    pub(crate) is_package: bool,
    search_roots: SearchRoots,
    entry_ownership: Option<bool>,
    /// A rebuilt module is declared by a .pyi beside its platform-specific archive.
    pub(crate) archive: Option<NativeArchive>,
}

impl SourceModule {
    /// Share the module's import names, string escapes, and template tokens across its function bodies.
    pub(crate) fn registered_values(&self) -> &crate::frontend::RegisteredModuleSource {
        self.registered_values
            .get_or_init(|| Box::new(crate::frontend::RegisteredModuleSource::new(self)))
    }

    pub(crate) fn cached_registered_values(
        &self,
    ) -> Option<&crate::frontend::RegisteredModuleSource> {
        self.registered_values.get().map(Box::as_ref)
    }

    pub(crate) fn original_source(&self) -> &str {
        self.original_source.as_deref().unwrap_or(&self.source)
    }

    pub(crate) fn is_program_source(&self) -> bool {
        !self.bundled
            && self.archive.is_none()
            && self
                .entry_ownership
                .unwrap_or(matches!(self.search_roots, SearchRoots::Single(0)))
    }

    /// The existing lexical contract checker covers the launch directory. A linked
    /// signature on an additional search root also applies to live, rebound calls.
    pub(crate) fn checks_source_contracts(&self) -> bool {
        self.is_program_source() && matches!(self.search_roots, SearchRoots::Single(0))
    }
}

pub(crate) struct NativeArchive {
    path: PathBuf,
    pub(crate) identity: PathBuf,
}

enum SearchRoots {
    Single(usize),
    Namespace(Vec<usize>),
}

impl SearchRoots {
    fn contains(&self, index: usize) -> bool {
        match self {
            Self::Single(root) => *root == index,
            Self::Namespace(roots) => roots.contains(&index),
        }
    }
}

/// The boxed provider supplies public Decimal names absent from the scalar module.
static BOXED_DECIMAL_EXPORTS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let (_, source) =
        crate::dynamic::bundled_module_source("_pydecimal", true).expect("bundled _pydecimal.py");
    let provider = ruff_python_parser::parse_module(source).expect("valid bundled _pydecimal.py");
    let exports = provider
        .syntax()
        .body
        .iter()
        .find(|stmt| value_name(stmt) == Some("__all__"))
        .and_then(module_value)
        .and_then(ast::Expr::as_list_expr)
        .expect("_pydecimal.__all__ must be a list literal");
    exports
        .elts
        .iter()
        .map(|item| {
            item.as_string_literal_expr()
                .expect("_pydecimal.__all__ must contain only string literals")
                .value
                .to_str()
                .to_owned()
        })
        .collect()
});

/// These public names are supplied by the boxed unittest.mock module.
pub(crate) fn boxed_mock_export(name: &str) -> bool {
    matches!(
        name,
        "sentinel"
            | "DEFAULT"
            | "ANY"
            | "MagicMock"
            | "AsyncMock"
            | "CallableMixin"
            | "NonCallableMock"
            | "NonCallableMagicMock"
            | "PropertyMock"
            | "_callable"
            | "_patch"
            | "_get_target"
            | "create_autospec"
            | "InvalidSpecError"
            | "mock_open"
            | "seal"
            | "call"
            | "_Call"
            | "_CallList"
    )
}

impl SourceModule {
    /// Consecutive executable statements can borrow their original parsed body.
    pub(crate) fn executable_statements(&self) -> std::borrow::Cow<'_, [ast::Stmt]> {
        use std::borrow::Cow;

        let source = &self.parsed.syntax().body;
        let Some((&first, &last)) = self.executable.first().zip(self.executable.last()) else {
            return Cow::Borrowed(&[]);
        };
        if self.executable.len() == last - first + 1 {
            Cow::Borrowed(&source[first..=last])
        } else {
            Cow::Owned(self.executable.iter().map(|&i| source[i].clone()).collect())
        }
    }

    /// Entry guards and standalone initializer calls precede the remaining declarations.
    pub(crate) fn has_entry_guard(&self) -> bool {
        self.entry_globals.iter().any(|&index| {
            matches!(
                self.parsed.syntax().body[index],
                ast::Stmt::If(_) | ast::Stmt::Expr(_)
            )
        })
    }

    pub(crate) fn function_binding(
        &self,
        owner: ModuleId,
        function: &ast::StmtFunctionDef,
    ) -> Option<DefinitionId> {
        let Binding::Value(first) = *self.bindings.get(function.name.as_str())? else {
            return None;
        };
        (first.module == owner && self.parsed.syntax().body[first.statement].is_function_def_stmt())
            .then_some(first)
    }

    fn local_value(&self, id: ModuleId, statement: &ast::Stmt) -> Option<DefinitionId> {
        let name = value_name(statement)?;
        let Binding::Value(first) = *self.bindings.get(name)? else {
            return None;
        };
        (first.module == id
            && value_name(&self.parsed.syntax().body[first.statement]) == Some(name))
        .then_some(first)
    }

    pub(crate) fn rebound_target(
        &self,
        id: ModuleId,
        statement: &ast::Stmt,
    ) -> Option<DefinitionId> {
        self.local_value(id, statement)
            .filter(|first| self.rebound_values.contains(&first.statement))
    }

    pub(crate) fn is_namespace(&self) -> bool {
        matches!(self.search_roots, SearchRoots::Namespace(_))
    }

    /// Bundled contexts whose enter returns the receiver and whose exit closes it.
    pub(crate) fn closes_context(&self, class: &str) -> bool {
        (self.is_bundled("selectors") && class == "PollSelector")
            || (self.is_bundled(TEMPFILE_SPOOL) && class == "SpooledTemporaryFile")
    }

    /// These annotation-only declarations receive immutable data from the checked resolver.
    pub(crate) fn import_config_slot<'a>(
        &self,
        statement: &'a ast::Stmt,
    ) -> Option<(&'a str, Intrinsic)> {
        let ast::Stmt::AnnAssign(declaration) = statement else {
            return None;
        };
        if declaration.value.is_some() {
            return None;
        }
        let name = value_name(statement)?;
        let slot = match name {
            "_filesystem_roots"
                if self.is_bundled("pkgutil") || self.is_bundled(METADATA_VERSION) =>
            {
                Intrinsic::StartupFilesystemRoots
            }
            "_module_suffixes" if self.is_bundled("pkgutil") => Intrinsic::ModuleSuffixes,
            _ => return None,
        };
        Some((name, slot))
    }

    pub(crate) fn is_bundled(&self, name: &str) -> bool {
        self.bundled && self.name == name
    }

    /// Subclasses of these fixed-layout stdlib types can override their protocols.
    pub(crate) fn python_subclasses(&self) -> Option<&'static str> {
        match self.name.as_str() {
            CALENDAR_CLASSES if self.bundled => Some("calendar subclasses"),
            FRACTION_CLASS if self.bundled => Some("Fraction subclasses"),
            _ => None,
        }
    }

    /// A type-only import in a native stub does not add a member to the C module.
    fn export(&self, name: &str) -> Option<Binding> {
        if self.is_bundled("uuid") {
            let export = match name {
                "uuid1" => Some("uuid.uuid1"),
                "uuid3" => Some("uuid.uuid3"),
                "uuid4" => Some("uuid.uuid4"),
                "uuid5" => Some("uuid.uuid5"),
                "uuid6" => Some("uuid.uuid6"),
                "uuid7" => Some("uuid.uuid7"),
                "uuid8" => Some("uuid.uuid8"),
                "NAMESPACE_DNS" => Some("uuid.NAMESPACE_DNS"),
                "NAMESPACE_URL" => Some("uuid.NAMESPACE_URL"),
                "NAMESPACE_OID" => Some("uuid.NAMESPACE_OID"),
                "NAMESPACE_X500" => Some("uuid.NAMESPACE_X500"),
                _ => None,
            };
            if let Some(export) = export {
                return Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(export)));
            }
        }
        if self.is_bundled("pkgutil") && name == "resolve_name" {
            return Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                "pkgutil.resolve_name",
            )));
        }
        if self.is_bundled("platform") && name == "win32_is_iot" {
            return Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                "platform.win32_is_iot",
            )));
        }
        if self.is_bundled("doctest") {
            let export = match name {
                "DocFileCase" => Some("doctest.DocFileCase"),
                "DocFileTest" => Some("doctest.DocFileTest"),
                "DocFileSuite" => Some("doctest.DocFileSuite"),
                _ => None,
            };
            if let Some(export) = export {
                return Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(export)));
            }
        }
        if self.is_bundled("socketserver")
            && matches!(
                name,
                "DatagramRequestHandler"
                    | "UDPServer"
                    | "ThreadingMixIn"
                    | "ThreadingTCPServer"
                    | "ThreadingUDPServer"
            )
        {
            return Some(Binding::Intrinsic(Intrinsic::SOCKETSERVER_CLASS));
        }
        if self.is_bundled("unittest") {
            if name == "IsolatedAsyncioTestCase" {
                return Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                    "unittest.IsolatedAsyncioTestCase",
                )));
            }
            let export = match name {
                "installHandler" => Some("unittest.installHandler"),
                "removeHandler" => Some("unittest.removeHandler"),
                "registerResult" => Some("unittest.registerResult"),
                "removeResult" => Some("unittest.removeResult"),
                _ => None,
            };
            if let Some(export) = export {
                return Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(export)));
            }
        }
        if self.is_bundled("unittest.mock") && name == "ThreadingMock" {
            return Some(Binding::Intrinsic(Intrinsic::RuntimeImport(
                "unittest.mock",
            )));
        }
        if self.is_bundled("unittest.mock") && boxed_mock_export(name) {
            return Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
                "unittest.mock objects",
            )));
        }
        let Some(&binding) = self.bindings.get(name) else {
            return (self.is_bundled("decimal") && BOXED_DECIMAL_EXPORTS.contains(name)).then_some(
                Binding::Intrinsic(Intrinsic::ScopedObjectExport("boxed decimal")),
            );
        };
        if binding == Binding::Intrinsic(Intrinsic::ZipImportIndex) {
            return None;
        }
        (self.archive.is_none() || matches!(binding, Binding::Definition(_))).then_some(binding)
    }

    pub(crate) fn diagnostic(&self, node: &impl Ranged, message: &str) -> Diagnostic {
        Diagnostic::at(&self.source, node, message.to_owned()).with_path(&self.path)
    }

    pub(crate) fn error(&self, node: &impl Ranged, message: &str) -> String {
        format!(
            "{}:{}",
            self.path.display(),
            Diagnostic::at(&self.source, node, message)
        )
    }
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(crate) enum FloatComponents {
    #[default]
    Native,
    Used,
    Boxed,
}

/// Whether native lowering may try an inferred layout and recover the ordinary checked path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutHint {
    Available,
    Applied,
    Disabled,
}

/// Resolver facts which native lowering can extend or consume while trying an inferred layout.
pub(crate) struct LoweringBaseline {
    explicit_check: bool,
    needs_object_runtime: bool,
    deferred_object_error: Option<Diagnostic>,
    reachable: Option<Arc<reachability::Reachable>>,
    traceback_objects: bool,
}

#[derive(Default)]
struct RegisteredFunctions {
    admitted: HashMap<ModuleId, crate::dynamic::CheckedFunctions>,
    /// Completed solved bodies which cannot establish graph admission. Lexical
    /// placement may use them after the selected source passes its own checks.
    lexical: Vec<(ModuleId, TextSize, crate::frontend::RegisteredFunction)>,
}

pub(crate) struct SourceGraph {
    /// Fixed at graph construction, in the checked resolver's search order.
    pub(crate) filesystem_roots: Box<[PathBuf]>,
    /// The launcher has already selected a registered module initializer.
    pub(crate) registered_startup: bool,
    registered_functions: std::cell::RefCell<RegisteredFunctions>,
    /// Default-rule error ranges for the physical sources shared by every layout attempt.
    pub(crate) registered_source_errors:
        std::cell::RefCell<HashMap<ModuleId, Vec<Option<TextRange>>>>,
    /// Contract of the original graph, valid only while its declarations still win.
    pub(crate) source_contract: Option<crate::analysis::SourceContract>,
    /// The original entry's reached call check when it consulted no import bindings.
    pub(crate) closed_initializer: Option<crate::analysis::InitializerCheck>,
    pub(crate) layout_hint: std::cell::Cell<LayoutHint>,
    pub(crate) float_components: std::cell::Cell<FloatComponents>,
    pub(crate) command_startup: Option<(crate::CommandFlags, String)>,
    pub(crate) virtual_command: bool,
    pub(crate) handled_tracebacks: bool,
    pub(crate) traceback_objects: bool,
    pub(crate) empty_keywords: empty_keywords::EmptyKeywords,
    /// Import proofs belong to source resolution and survive representation changes.
    checked_builtin_imports: Vec<(ModuleId, TextRange)>,
    checked_diagnostics: std::cell::RefCell<Vec<Vec<CheckedDiagnostic>>>,
    pub(crate) needs_object_runtime: std::cell::Cell<bool>,
    /// A reachable native constructor can use its object implementation if typed lowering fails.
    pub(crate) has_object_constructor_fallback: std::cell::Cell<bool>,
    pub(crate) check_fallback_calls: std::cell::Cell<FallbackCalls>,
    pub(crate) imports_runtime_class: bool,
    pub(crate) check_method_factories: std::cell::Cell<bool>,
    pub(crate) class_layout_error: std::cell::RefCell<Option<Diagnostic>>,
    pub(crate) runtime_initializer: Option<String>,
    /// Object discovery must establish its own entry when an import was admitted only
    /// for checked lowering.
    pub(crate) has_checked_only_import: bool,
    pub(crate) deferred_object_error: std::cell::RefCell<Option<Diagnostic>>,
    pub(crate) entry_profile: entry_profile::Profile,
    pub(crate) mutable_builtin_len: bool,
    reachable: Option<Arc<reachability::Reachable>>,
    required_library: Option<(ModuleId, reachability::Reachable)>,
    pub(crate) inputs: crate::SourceInputs,
    zip_files: Vec<(PathBuf, PathBuf, String)>,
    pub(crate) session: Session,
    pub(crate) modules: Vec<SourceModule>,
    by_name: HashMap<String, ModuleId>,
    pub(crate) entry: ModuleId,
    ordered_bindings: Option<ordered_imports::Bindings>,
    /// The absolute entry spelling used for Python's `__file__`.
    entry_filename: PathBuf,
    /// Split literal prefixes at imports so allocations and package exports are
    /// available to the child at the original point in Python's initialization.
    initialization: Vec<(ModuleId, Range<usize>)>,
    pub(crate) errno_names: Vec<String>,
    pub(crate) doctest_imports: Vec<String>,
}

pub(crate) const MEMORY_SOURCE_PATH: &str = "/input.py";

impl SourceGraph {
    pub(crate) fn lowering_baseline(&self) -> LoweringBaseline {
        LoweringBaseline {
            explicit_check: self.session.explicit_check,
            needs_object_runtime: self.needs_object_runtime.get(),
            deferred_object_error: self.deferred_object_error.borrow().clone(),
            reachable: self.reachable.clone(),
            traceback_objects: self.traceback_objects,
        }
    }

    /// Start ordinary lowering with the original import decisions and proof scopes.
    pub(crate) fn retry_without_layout_hints(&mut self, baseline: &LoweringBaseline) {
        self.source_contract = None;
        self.session.resume_layout_queries();
        self.session.explicit_check = baseline.explicit_check;
        self.layout_hint.set(LayoutHint::Disabled);
        self.float_components.set(FloatComponents::Native);
        *self.class_layout_error.get_mut() = None;
        self.has_object_constructor_fallback.set(false);
        self.check_fallback_calls.set(FallbackCalls::None);
        self.check_method_factories.set(false);
        self.needs_object_runtime.set(baseline.needs_object_runtime);
        self.deferred_object_error
            .get_mut()
            .clone_from(&baseline.deferred_object_error);
        self.reachable.clone_from(&baseline.reachable);
        self.traceback_objects = baseline.traceback_objects;
        for proofs in self.checked_diagnostics.get_mut() {
            proofs.clear();
        }
    }

    /// A representation trial must neither discard a Python operation nor change the
    /// admission or diagnostics of the enclosing registered program.
    pub(crate) fn try_registered_body<T>(
        &self,
        module: ModuleId,
        lower: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let diagnostics = self.checked_diagnostic_count(module);
        let deferred = self.deferred_object_error.take();
        let needs_object = self.needs_object_runtime.replace(false);
        let calls = self.check_fallback_calls.get();
        let constructor = self.has_object_constructor_fallback.replace(false);
        let floats = self.float_components.get();
        let set = self.layout_hint.get();
        let result = lower();
        let deferred_here = self.deferred_object_error.replace(deferred).is_some();
        let needed_here = self.needs_object_runtime.replace(needs_object);
        let used_constructor = self.has_object_constructor_fallback.replace(constructor);
        let result = result.filter(|_| !deferred_here && !needed_here && !used_constructor);
        self.check_fallback_calls.set(calls);
        self.float_components.set(floats);
        self.layout_hint.set(set);
        self.checked_diagnostics.borrow_mut()[module].truncate(diagnostics);
        result
    }

    pub(crate) fn clear_registered_functions(&self) {
        let mut functions = self.registered_functions.borrow_mut();
        functions.admitted.clear();
        functions.lexical.clear();
    }

    pub(crate) fn register_function(
        &self,
        module: ModuleId,
        start: TextSize,
        body: crate::frontend::RegisteredFunction,
    ) {
        self.registered_functions
            .borrow_mut()
            .admitted
            .entry(module)
            .or_default()
            .insert(start, body);
    }

    pub(crate) fn defer_registered_function(
        &self,
        module: ModuleId,
        start: TextSize,
        use_lexical_body: bool,
    ) {
        let mut functions = self.registered_functions.borrow_mut();
        let entries = functions
            .admitted
            .get_mut(&module)
            .expect("captured module");
        let body = entries.remove(&start).expect("captured definition");
        if entries.is_empty() {
            functions.admitted.remove(&module);
        }
        // A fixed body carries its own placement proof. Its failed source check
        // cannot authorize that placement at a later Python call.
        if use_lexical_body && !matches!(body.inputs, crate::frontend::FrameInputs::Protocol { .. })
        {
            functions.lexical.push((module, start, body));
        }
    }

    pub(crate) fn registered_functions(
        &self,
    ) -> std::cell::Ref<'_, HashMap<ModuleId, crate::dynamic::CheckedFunctions>> {
        std::cell::Ref::map(self.registered_functions.borrow(), |functions| {
            &functions.admitted
        })
    }

    /// The selected source contract has succeeded. A solved body's checked
    /// parameter convention must still be valid with live Python arguments.
    pub(crate) fn take_registered_functions(
        &self,
    ) -> HashMap<ModuleId, crate::dynamic::CheckedFunctions> {
        let RegisteredFunctions {
            mut admitted,
            lexical,
        } = self.registered_functions.take();
        for (module, start, mut body) in lexical {
            use crate::frontend::{FrameArguments, FrameInputs};
            match body.inputs {
                FrameInputs::Deferred { sites, live: true } => {
                    body.inputs = FrameInputs::Protocol { sites };
                }
                FrameInputs::Checked { .. } => {
                    // A subsequent registration can replace the provisional body at this site.
                    let Some((inputs, slots)) =
                        crate::llvm::registered::frame_inputs(&body, FrameArguments::Python)
                    else {
                        continue;
                    };
                    body.inputs = inputs;
                    body.frame_slots = slots;
                }
                _ => continue,
            }
            admitted
                .entry(module)
                .or_default()
                .entry(start)
                .or_insert(body);
        }
        admitted
    }

    /// The fixed adapter displaced when this source supplies the import.
    pub(crate) fn replaced_adapter(&self, name: &str, body: &str) -> Option<ModuleId> {
        self.bundled_module(name)
            .filter(|&id| self.modules[id].original_source() != body)
    }

    pub(crate) fn bundled_module(&self, name: &str) -> Option<ModuleId> {
        let id = *self.by_name.get(name)?;
        let module = &self.modules[id];
        (module.bundled && !module.source.is_empty()).then_some(id)
    }

    /// A concrete Python file registered by the checked resolver, before object lowering.
    pub(crate) fn python_file(&self, name: &str) -> Option<ModuleId> {
        let id = *self.by_name.get(name)?;
        let module = &self.modules[id];
        (!module.bundled
            && !module.is_namespace()
            && module.archive.is_none()
            && !(id == self.entry && self.virtual_command))
            .then_some(id)
    }

    /// Match a registered import against the file chosen under its checked search root.
    pub(crate) fn registration_at(
        &self,
        id: ModuleId,
        root: &Path,
        file: &Path,
        init: &Path,
    ) -> Option<bool> {
        let module = &self.modules[id];
        let path = if module.is_package { init } else { file };
        let matches = module.path == path
            || self.inputs.directories.iter().any(|(spelling, identity)| {
                spelling == root
                    && module
                        .path
                        .strip_prefix(identity)
                        .is_ok_and(|relative| path.strip_prefix(root).ok() == Some(relative))
            });
        matches.then_some(module.is_package)
    }

    /// Request checked-file diagnostics without replacing checks requested by a Python entry.
    pub(crate) fn require_fallback_calls(&self) {
        if matches!(self.check_fallback_calls.get(), FallbackCalls::None) {
            self.check_fallback_calls.set(FallbackCalls::All);
        }
    }

    /// The native value-struct decorator has no corresponding Python object runtime export.
    pub(crate) fn imports_struct_decorator(&self) -> bool {
        self.modules.iter().any(|module| {
            module.bindings.values().any(|binding| {
                matches!(
                    binding,
                    Binding::Intrinsic(Intrinsic::Struct)
                        | Binding::IntrinsicModule(IntrinsicModule::Imre)
                )
            })
        })
    }

    pub(crate) fn boxed_pattern_method(
        &self,
        module: usize,
        expr: &ast::Expr,
    ) -> Option<&'static str> {
        let attr = expr.as_attribute_expr()?;
        let method = match attr.attr.as_str() {
            "findall" => "re.Pattern.findall",
            "split" => "re.Pattern.split",
            "subn" => "re.Pattern.subn",
            _ => return None,
        };
        self.session
            .is_regex_pattern(module, &attr.value)
            .then_some(method)
    }

    /// A first-party loader can select object execution before a typed call in another module.
    pub(crate) fn uses_source_loader(&self) -> bool {
        self.modules.iter().enumerate().any(|(id, module)| {
            !module.bundled
                && (module.bindings.values().any(|binding| {
                    *binding
                        == Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                            "importlib.abc.SourceLoader",
                        ))
                }) || ast::helpers::any_over_body(&module.parsed.syntax().body, |expr| {
                    expr.as_attribute_expr().is_some_and(|attr| {
                        attr.attr.as_str() == "SourceLoader"
                            && self
                                .session
                                .is_stdlib_module(id, &attr.value, "importlib.abc")
                    })
                }))
        })
    }

    pub(crate) fn is_abstract_context_manager(&self, definition: DefinitionId) -> bool {
        let source = &self.modules[definition.module];
        source.is_bundled(CONTEXTLIB_ABSTRACT)
            && source.export("AbstractContextManager") == Some(Binding::Definition(definition))
    }

    /// Resolve an implicit call or class reference in its selected bundled module.
    pub(crate) fn bundled_definition(&self, owner: &str, name: &str) -> Option<DefinitionId> {
        let module = &self.modules[*self.by_name.get(owner)?];
        match module.bindings.get(name)? {
            Binding::Definition(def) if module.bundled => Some(*def),
            _ => None,
        }
    }

    pub(crate) fn logging_definition(&self, name: &str) -> Option<DefinitionId> {
        self.bundled_definition(LOGGING_CLASSES, name)
    }

    pub(crate) fn checked_diagnostic_count(&self, module: ModuleId) -> usize {
        self.checked_diagnostics.borrow()[module].len()
    }

    pub(crate) fn is_unittest_main(&self, id: DefinitionId) -> bool {
        let owner = &self.modules[id.module];
        owner.bundled
            && owner.name == "unittest"
            && owner.bindings.get("main") == Some(&Binding::Definition(id))
    }

    /// Keep module indices stable for the semantic cache. Only after lowering
    /// expose the files a disk build must revalidate and protect from writes.
    pub(crate) fn disk_inputs(
        &mut self,
        profile: Option<&Path>,
    ) -> (crate::SourceInputs, Vec<PathBuf>) {
        let mut inputs = std::mem::take(&mut self.inputs);
        let mut modules = self.modules.iter().enumerate();
        inputs.files.retain(|(path, _, _)| {
            let (id, module) = modules.next().expect("one input per module");
            !module.bundled
                && !module.is_namespace()
                && !(id == self.entry && self.virtual_command)
                && !self
                    .zip_files
                    .iter()
                    .any(|(archive, _, _)| path.starts_with(archive))
        });
        inputs.files.append(&mut self.zip_files);
        let paths = inputs.protected_paths(profile);
        // The linker receives physical archive paths. A hit must also validate the
        // import spelling in case a wheel symlink was retargeted or removed.
        for archive in self
            .modules
            .iter()
            .filter_map(|module| module.archive.as_ref())
        {
            match FileSystem::default().open_hashed(&archive.identity) {
                Ok((_, hash)) => inputs.files.push((
                    archive.path.clone(),
                    archive.identity.clone(),
                    hash.to_hex().to_string(),
                )),
                Err(_) => inputs.relink_site_wheel = true,
            }
        }
        (inputs, paths)
    }

    /// A write affects bare len calls in every module, including earlier definitions.
    fn writes_builtin_len(&self) -> bool {
        use ruff_python_ast::visitor::{Visitor, walk_expr};
        struct Scan<'a> {
            graph: &'a SourceGraph,
            module: ModuleId,
            writes: bool,
        }
        impl<'a> Visitor<'a> for Scan<'_> {
            fn visit_expr(&mut self, expr: &'a ast::Expr) {
                if let ast::Expr::Attribute(attr) = expr
                    && attr.ctx != ast::ExprContext::Load
                    && self.graph.resolve(self.module, expr)
                        == Some(Binding::Intrinsic(Intrinsic::BuiltinsLen))
                {
                    self.writes = true;
                }
                if !self.writes {
                    walk_expr(self, expr);
                }
            }
        }
        if !self.modules.iter().any(|source| {
            source
                .bindings
                .values()
                .any(|binding| *binding == Binding::IntrinsicModule(IntrinsicModule::Builtins))
        }) {
            return false;
        }
        self.modules.iter().enumerate().any(|(module, source)| {
            let mut scan = Scan {
                graph: self,
                module,
                writes: false,
            };
            for stmt in &source.parsed.syntax().body {
                scan.visit_stmt(stmt);
            }
            scan.writes
        })
    }

    /// Ordinary file builds keep assertions at every native optimizer level.
    pub(crate) fn python_optimization(&self) -> i32 {
        self.command_startup
            .as_ref()
            .map_or(0, |(flags, _)| flags.optimize)
    }

    pub(crate) fn include_key_reprs(&mut self, owners: &HashSet<DefinitionId>) {
        self.reachable = Some(Arc::new(reachability::Reachable::collect_with_reprs(
            self, false, owners,
        )));
        self.refresh_traceback_objects();
    }

    fn refresh_traceback_objects(&mut self) {
        if self.handled_tracebacks {
            self.traceback_objects = self.reachable.as_ref().map_or_else(
                || reachability::Reachable::collect(self, false).traceback_objects,
                |roots| roots.traceback_objects,
            );
        }
    }

    pub(crate) fn reachable_only(&self) -> bool {
        self.reachable.is_some()
    }

    fn required_definitions(&self, module: ModuleId) -> Option<&reachability::Reachable> {
        if let Some((library, roots)) = &self.required_library
            && *library == module
        {
            return Some(roots);
        }
        self.reachable.as_deref()
    }

    pub(crate) fn includes(&self, definition: DefinitionId) -> bool {
        !self.modules[definition.module].registered_namespace
            && self
                .required_definitions(definition.module)
                .is_none_or(|r| r.definitions.contains(&definition))
    }

    pub(crate) fn inherits_class_method(&self, class: DefinitionId, name: &str) -> bool {
        self.reachable
            .as_ref()
            .is_none_or(|r| r.inherited_class_method(class, name))
    }

    pub(crate) fn unused_literal_global(&self, definition: DefinitionId) -> bool {
        self.reachable
            .as_ref()
            .is_some_and(|r| r.unused_literals.contains(&definition))
    }

    pub(crate) fn includes_method(&self, module: ModuleId, method: &ast::StmtFunctionDef) -> bool {
        self.required_definitions(module)
            .is_none_or(|r| r.methods.contains(&(module, method.start())))
    }

    pub(crate) fn module_filename(&self, module: ModuleId) -> &Path {
        if module == self.entry {
            &self.entry_filename
        } else {
            &self.modules[module].path
        }
    }

    /// The import package of a linked source module; direct script entries have no package.
    pub(crate) fn module_package(&self, module: ModuleId) -> Option<&str> {
        let source = &self.modules[module];
        (module != self.entry).then(|| import_package(&source.name, source.is_package))
    }

    /// Record only this call's overload diagnostic. Nested arguments still get
    /// their own diagnostics, including calls with the same method name.
    pub(crate) fn checked_overload(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::Call(call));
    }

    /// The live namespace supplies the callable for this checked Python call. Its binder
    /// checks the arguments against that object; nested expressions keep their own checks.
    pub(crate) fn checked_live_call(&self, module: ModuleId, call: &ast::ExprCall) {
        let checked = &mut self.checked_diagnostics.borrow_mut()[module];
        checked.extend([
            CheckedDiagnostic::NonCallable(call.range()),
            CheckedDiagnostic::Call(call.range()),
            CheckedDiagnostic::MissingArgument(call.range()),
        ]);
        for arg in &call.arguments.args {
            checked.push(CheckedDiagnostic::ExcessArgument(arg.range()));
            checked.push(CheckedDiagnostic::ArgumentType(arg.range()));
        }
        let mut names = HashSet::new();
        for keyword in &call.arguments.keywords {
            checked.extend([
                CheckedDiagnostic::Keyword(keyword.range()),
                CheckedDiagnostic::PositionalKeyword(keyword.range()),
                CheckedDiagnostic::ArgumentType(keyword.range()),
                CheckedDiagnostic::ArgumentType(keyword.value.range()),
            ]);
            // An explicitly repeated name is invalid regardless of the callable.
            if let Some(name) = &keyword.arg
                && names.insert(name.as_str())
            {
                checked.push(CheckedDiagnostic::RepeatedKeyword(keyword.range()));
            }
        }
    }

    /// Native evaluation of this exact call raises for a known noncallable object.
    pub(crate) fn checked_noncallable(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::NonCallable(call));
    }

    /// A checked runtime overload can legally stand alone: calling the dummy raises.
    pub(crate) fn runtime_overload(&self, module: ModuleId, name: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::RuntimeOverload(name));
    }

    /// Mark the precise call whose behavior for missing arguments is handled by native lowering.
    pub(crate) fn checked_missing_argument(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::MissingArgument(call));
    }

    /// This exact operand is excessive for a resolved native method that raises TypeError.
    pub(crate) fn checked_excess_argument(&self, module: ModuleId, argument: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::ExcessArgument(argument));
    }

    /// Native lowering accepts an inert body at this exact return annotation.
    pub(crate) fn checked_inert_return(&self, module: ModuleId, annotation: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::InertReturn(annotation));
    }

    /// The native positional-only binder already raises at this exact keyword.
    pub(crate) fn checked_positional_keyword(&self, module: ModuleId, keyword: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::PositionalKeyword(keyword));
    }

    /// Lowering checked this exact operand against the native integer protocol.
    pub(crate) fn checked_argument_type(&self, module: ModuleId, argument: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::ArgumentType(argument));
    }

    pub(crate) fn deferred_name(&self, module: ModuleId, name: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::DeferredName(name));
    }

    /// The native binder raises because this keyword's parameter was already bound.
    pub(crate) fn checked_repeated_keyword(&self, module: ModuleId, keyword: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::RepeatedKeyword(keyword));
    }

    /// An optional handler lock either enters the captured lock or raises TypeError on None.
    pub(crate) fn checked_optional_lock_context(&self, module: ModuleId, expr: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::OptionalLockContext(expr));
    }

    /// This operand is evaluated by a tuple display that raises a native TypeError.
    pub(crate) fn checked_non_iterable_spread(&self, module: ModuleId, value: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::NonIterableSpread(value));
    }

    pub(crate) fn checked_keywords(
        &self,
        module: ModuleId,
        keywords: impl Iterator<Item = TextRange>,
    ) {
        self.checked_diagnostics.borrow_mut()[module]
            .extend(keywords.map(CheckedDiagnostic::Keyword));
    }

    /// Target headers decide whether an errno from-import succeeds or raises ImportError.
    pub(crate) fn checked_builtin_import(&self, module: ModuleId, alias: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::BuiltinImport(alias));
    }

    /// A sealed module exports this exact member even when typeshed omits it.
    pub(crate) fn checked_builtin_attribute(&self, module: ModuleId, name: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::BuiltinAttribute(name));
    }

    /// Record an integer access with a proved native indexing implementation.
    /// Other diagnostics at this index or inside the receiver remain visible.
    pub(crate) fn checked_index_read(&self, module: ModuleId, read: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::CheckedIndexRead(read));
    }

    /// The accelerated result of cmp_to_key remains callable after binding an object.
    pub(crate) fn checked_callable_key(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::CallableKey(call));
    }

    /// The native table is an actual dict, including errno.errorcode (a Mapping in typeshed).
    pub(crate) fn checked_dict_deletion(&self, module: ModuleId, target: TextRange) {
        self.checked_diagnostics.borrow_mut()[module].push(CheckedDiagnostic::DictDeletion(target));
    }

    /// This read checks a local's assignment flag and raises Python's error if unset.
    pub(crate) fn checked_unbound_local_read(&self, module: ModuleId, read: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::UnboundLocalRead(read));
    }

    /// Native construction raises TypeError at this abstract class call.
    pub(crate) fn checked_abstract_class_call(&self, module: ModuleId, call: TextRange) {
        self.checked_diagnostics.borrow_mut()[module]
            .push(CheckedDiagnostic::AbstractClassCall(call));
    }

    /// Source ranges where the reachable profile or native lowering handles diagnostics.
    pub(crate) fn diagnostic_scopes(
        &self,
    ) -> (Option<Vec<Vec<TextRange>>>, Vec<Vec<CheckedDiagnostic>>) {
        let mut overloads = self.checked_diagnostics.borrow().clone();
        for &(id, range) in &self.checked_builtin_imports {
            overloads[id].push(CheckedDiagnostic::BuiltinImport(range));
        }
        for (id, diagnostics) in overloads.iter_mut().enumerate() {
            diagnostics.sort_unstable_by_key(|note| note.sort_key());
            diagnostics.dedup();
            let decorators = self.modules[id]
                .parsed
                .syntax()
                .body
                .iter()
                .filter_map(|stmt| {
                    if let ast::Stmt::ClassDef(c) = stmt {
                        Some(&c.body)
                    } else {
                        None
                    }
                })
                .flatten()
                .filter_map(|stmt| {
                    let ast::Stmt::FunctionDef(method) = stmt else {
                        return None;
                    };
                    let [decorator] = method.decorator_list.as_slice() else {
                        return None;
                    };
                    (self.resolve(id, &decorator.expression)
                        == Some(Binding::Intrinsic(Intrinsic::ContextManager)))
                    .then_some(CheckedDiagnostic::Decorator(decorator.range()))
                });
            diagnostics.splice(..0, decorators);
        }
        let mut skipped = self.reachable.as_ref().map(|r| r.skipped.clone());
        if let Some((library, roots)) = &self.required_library {
            skipped.get_or_insert_with(|| vec![Vec::new(); self.modules.len()])[*library]
                .extend_from_slice(&roots.skipped[*library]);
        }
        // The bundled _simple_enum factories are lowered to immutable member tables.
        // Keep the actual category property bodies and every caller in ty's check.
        if let Some((id, http)) = self
            .modules
            .iter()
            .enumerate()
            .find(|(_, m)| m.is_bundled("http"))
        {
            let ranges =
                &mut skipped.get_or_insert_with(|| vec![Vec::new(); self.modules.len()])[id];
            for stmt in &http.parsed.syntax().body {
                match stmt {
                    ast::Stmt::ImportFrom(import) if import.module.as_deref() == Some("enum") => {
                        ranges.push(import.range());
                    }
                    ast::Stmt::ClassDef(class) => {
                        ranges.extend(class.body.iter().filter_map(|s| {
                            if let ast::Stmt::FunctionDef(f) = s
                                && f.name.as_str() == "__new__"
                            {
                                Some(f.range())
                            } else {
                                None
                            }
                        }))
                    }
                    _ => {}
                }
            }
        }
        if self.python_optimization() > 0 {
            use ast::visitor::{Visitor, walk_stmt};

            struct OmittedAssertions<'a>(&'a mut Vec<TextRange>);
            impl<'ast> Visitor<'ast> for OmittedAssertions<'_> {
                fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
                    if matches!(stmt, ast::Stmt::Assert(_)) {
                        self.0.push(stmt.range());
                    } else {
                        walk_stmt(self, stmt);
                    }
                }

                fn visit_expr(&mut self, _: &'ast ast::Expr) {}
            }
            let ranges = skipped.get_or_insert_with(|| vec![Vec::new(); self.modules.len()]);
            for (module, ranges) in self.modules.iter().zip(ranges) {
                OmittedAssertions(ranges).visit_body(&module.parsed.syntax().body);
            }
        }
        (skipped, overloads)
    }

    /// Entry segments preserve globals, platform import checks, and definition-time effects.
    pub(crate) fn initializers(
        &self,
        entry_declarations: bool,
    ) -> impl Iterator<Item = (ModuleId, Range<usize>)> + '_ {
        self.initialization
            .iter()
            .filter(move |(id, segment)| {
                let module = &self.modules[*id];
                *id != self.entry
                    || !module.snapshots.is_empty()
                    || !module.registered_imports.is_empty()
                    || entry_declarations
                    || !module.entry_globals.is_empty()
                    || !module.overloads.is_empty()
                    || !module.rebound_values.is_empty()
                    || (!segment.is_empty()
                        && errno_import(&module.parsed.syntax().body[segment.end - 1]).is_some())
            })
            .cloned()
    }

    /// An isolated source API supports sealed imports only; first-party imports
    /// require the path API and its filesystem search root.
    #[cfg(test)]
    pub(crate) fn single(source: &str) -> Result<Self, String> {
        Self::single_with_mode(source, false)
    }

    pub(crate) fn single_with_mode(source: &str, reachable_only: bool) -> Result<Self, String> {
        let mut loader = Loader::new(PathBuf::from("/"), &[], &[], None, FileSystem::default())?;
        loader.allow_imports = false;
        loader.reachable_only = reachable_only;
        let filename = PathBuf::from(MEMORY_SOURCE_PATH);
        let entry = loader.add(
            filename.clone(),
            filename.clone(),
            "__main__".into(),
            false,
            source,
        )?;
        loader.scan(entry)?;
        loader.finish(entry, filename, reachable_only, None)
    }

    /// Parse command bytes as __main__; only a non-safe command sees the working directory.
    pub(crate) fn command(
        command: &crate::PythonCommand,
        options: &crate::CompileOptions,
        fs: FileSystem<'_>,
        import_failure: &mut Option<EntryImportFailure>,
        site_probes: &mut SiteProbes,
    ) -> Result<Self, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let mut loader = Loader::new(
            root.clone(),
            &options.extension_paths,
            &options.python_archive_indices,
            Some((options, site_probes)),
            fs,
        )?;
        loader.skip_current_root = command.flags.safe_path;
        loader.frozen_modules = command.flags.frozen_modules;
        loader.reachable_only = options.reachable_only || options.entry_profile.is_some();
        let filename = root.join("<string>.py");
        let entry = loader.add_source(
            filename.clone(),
            filename.clone(),
            "__main__".into(),
            false,
            ModuleText::Text(command.source.as_str().into(), crate::SourceKind::Command),
        )?;
        loader.scan_python_entry(entry, import_failure)?;
        let reachable_only = loader.reachable_only;
        let mut graph = loader.finish(
            entry,
            filename,
            reachable_only,
            options.entry_profile.as_deref(),
        )?;
        graph.command_startup = Some((command.flags.clone(), command.executable.clone()));
        graph.virtual_command = true;
        Ok(graph)
    }

    /// Resolve a Python module operand through the same ordered loader used for imports.
    /// The runtime launcher still executes runpy and the selected package initializers.
    /// A non-project provider, or a launcher whose selection requires Python execution,
    /// leaves the authoritative source set to that launcher's manifest.
    pub(crate) fn module_entry(
        path: &Path,
        name: &str,
        options: &crate::CompileOptions,
        mut fs: FileSystem<'_>,
        site_probes: &mut SiteProbes,
    ) -> Result<Option<Self>, String> {
        if !valid_module_operand(name)
            || !options.path_entry && name.split('.').next() == Some("__main__")
            || options.sitecustomize
            || options.stdlib_sitecustomize
            || options.user_site_pth.is_some()
        {
            return Ok(None);
        }
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let spelling = if options.path_entry {
            cwd.join(path)
        } else {
            cwd
        };
        let archive_entry = options.path_entry && !fs.is_dir(&spelling);
        let root = if archive_entry {
            spelling.clone()
        } else {
            fs.canonicalize(&spelling).map_err(|e| e.to_string())?
        };
        let mut loader = Loader::new(
            root.clone(),
            &options.extension_paths,
            &options.python_archive_indices,
            Some((options, site_probes)),
            fs.reborrow(),
        )?;
        let startup = options.python_startup();
        loader.skip_current_root =
            !options.path_entry && startup.is_some_and(|(flags, _)| flags.safe_path);
        loader.frozen_modules = startup.is_none_or(|(flags, _)| flags.frozen_modules);
        loader.reachable_only = options.reachable_only || options.entry_profile.is_some();
        if archive_entry {
            let Some(archive) = crate::zip_entry::ZipSources::discover(
                &spelling,
                &mut loader.inputs,
                &mut loader.fs,
            )?
            else {
                return Ok(None);
            };
            loader.zip_files.append(&mut loader.inputs.files);
            loader.zip_sources.insert(root, archive);
        } else if !loader.skip_current_root {
            loader.inputs.directories.push((spelling, root));
        }
        loader.module_operand = Some(name.to_owned());
        if loader.has_project_source("runpy") {
            return Ok(None);
        }
        let entry = match loader.load_module_entry(name) {
            Ok(Some(id)) => id,
            Ok(None) => return Ok(None),
            Err(_) if loader.import_failure.is_some() => return Ok(None),
            Err(error) => return Err(error),
        };
        let filename = loader.modules[entry].path.clone();
        let skipped = usize::from(loader.skip_current_root);
        let reachable_only = loader.reachable_only;
        let mut graph = loader.finish(
            entry,
            filename,
            reachable_only,
            options.entry_profile.as_deref(),
        )?;
        if let SearchRoots::Single(index) = graph.modules[entry].search_roots {
            let root = &graph.filesystem_roots[index - skipped];
            if let Some(owner) = crate::installed_wheels::ModuleEntryRoot::select(
                root,
                &graph.modules[entry].path,
                index == 0,
                &mut site_probes.wheel_records,
                options,
                &mut graph.inputs,
                &mut fs,
            ) {
                for module in &mut graph.modules {
                    if matches!(module.search_roots, SearchRoots::Single(i) if i == index) {
                        module.entry_ownership = owner.is_program(root, &module.path);
                    }
                }
            }
        }
        if let Some((flags, executable)) = startup {
            graph.command_startup = Some((flags.clone(), executable.to_owned()));
        }
        Ok(Some(graph))
    }

    /// A directly invoked file is __main__; its enclosing package is not imported.
    pub(crate) fn script(
        path: &Path,
        flags: &crate::CommandFlags,
        executable: &str,
        options: &crate::CompileOptions,
        mut fs: FileSystem<'_>,
        import_failure: &mut Option<EntryImportFailure>,
        site_probes: &mut SiteProbes,
    ) -> Result<Self, String> {
        let filename = std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(path);
        let entry = fs
            .canonicalize(path)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let bytes = if crate::named_bytecode(path) {
            std::fs::File::open(&entry).and_then(crate::read_bytecode_prefix)
        } else {
            fs.read_source(&entry)
        }
        .map_err(|err| format!("{}: {err}", path.display()))?;
        if let Some(error) = crate::bytecode_script_error(path, &bytes) {
            return Err(error.into());
        }
        let source = ModuleText::disk(&filename, bytes)?;
        let root = entry
            .parent()
            .ok_or("script has no parent directory")?
            .to_path_buf();
        let mut loader = Loader::new(
            root,
            &options.extension_paths,
            &options.python_archive_indices,
            Some((options, site_probes)),
            fs,
        )?;
        loader.skip_current_root = flags.safe_path;
        loader.frozen_modules = flags.frozen_modules;
        let reachable = options.reachable_only || options.entry_profile.is_some();
        loader.reachable_only = reachable;
        let id = loader.add_source(entry.clone(), entry, "__main__".into(), false, source)?;
        loader.scan_python_entry(id, import_failure)?;
        let mut graph = loader.finish(id, filename, reachable, options.entry_profile.as_deref())?;
        graph.command_startup = Some((flags.clone(), executable.to_owned()));
        Ok(graph)
    }

    #[cfg(test)]
    pub(crate) fn load(
        entry: &Path,
        module_root: Option<&Path>,
        extension_paths: &[PathBuf],
    ) -> Result<Self, String> {
        Self::load_with(
            entry,
            module_root,
            &crate::CompileOptions {
                extension_paths: extension_paths.to_vec(),
                ..Default::default()
            },
            FileSystem::default(),
            &mut None,
            &mut SiteProbes::default(),
        )
    }

    pub(crate) fn load_with(
        entry: &Path,
        module_root: Option<&Path>,
        options: &crate::CompileOptions,
        mut fs: FileSystem<'_>,
        import_failure: &mut Option<EntryImportFailure>,
        site_probes: &mut SiteProbes,
    ) -> Result<Self, String> {
        let profile = options.entry_profile.as_deref();
        let reachable_only = options.reachable_only || profile.is_some();
        // __file__ preserves the invoked spelling, including symlinks and '..'.
        // Use the canonical path below for import resolution and containment.
        let filename = std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(entry);
        let entry = fs
            .canonicalize(entry)
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        if entry.extension().and_then(|value| value.to_str()) != Some("py") {
            return Err("entry source must be a .py file".into());
        }
        if entry.file_name().and_then(|value| value.to_str()) == Some("__init__.py") {
            return Err("use a program module or package __main__.py as the entry source".into());
        }
        let inferred = module_root.is_none();
        let root = if let Some(root) = module_root {
            fs.canonicalize(root)
                .map_err(|error| format!("{}: {error}", root.display()))?
        } else {
            let mut root = entry
                .parent()
                .ok_or("entry source has no parent")?
                .to_path_buf();
            while site_probes.is_file(&mut fs, root.join("__init__.py")) {
                root = root
                    .parent()
                    .ok_or("package has no module root")?
                    .to_path_buf();
            }
            site_probes.package_root = Some(root.clone());
            root
        };
        if !fs.is_dir(&root) {
            return Err("module root must be a directory".into());
        }
        let relative = entry
            .strip_prefix(&root)
            .map_err(|_| "entry source must be inside the module root")?;
        let stem = relative.with_extension("");
        let parts = stem
            .components()
            .map(|part| {
                part.as_os_str()
                    .to_str()
                    .ok_or("module paths must be valid UTF-8")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let name = parts.join(".");
        // Keep the physical FILE's import identity even if scanning its imports
        // selects the registered runtime. A literal dot in a filename is not a
        // package separator.
        if parts.len() > 1 && parts.iter().all(|part| !part.contains('.')) {
            site_probes.packaged_file_module = Some(name.clone());
        }
        let bytes = fs
            .read_source(&entry)
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        let source = ModuleText::disk(&entry, bytes)?;
        let mut loader = Loader::new(
            root,
            &options.extension_paths,
            &options.python_archive_indices,
            None,
            fs,
        )?;
        loader.reachable_only = reachable_only;
        if inferred {
            loader.inputs.missing.push(loader.root.join("__init__.py"));
        }
        let id = loader.add_source(entry.clone(), entry, name.clone(), false, source)?;
        // Like `python -m package.program`, package initialization precedes the entry.
        // The entry itself is __main__, and is deliberately not attached as a package child.
        if let Some((package, _)) = name.rsplit_once('.') {
            let package = loader.load_module(package)?;
            if !loader.modules[package].is_package {
                return Err(
                    "entry source is inside a directory without an __init__.py package".into(),
                );
            }
        }
        if options.python_archive_indices.is_empty() {
            loader.scan(id)?;
        } else {
            loader.scan_python_entry(id, import_failure)?;
        }
        loader.finish(id, filename, reachable_only, profile)
    }

    /// Resolves namespace paths while leaving local-value attributes to the expression checker.
    pub(crate) fn resolve(&self, module: ModuleId, expression: &ast::Expr) -> Option<Binding> {
        if let Some(import) = self.local_static_binding(module, expression) {
            return self.local_import_value(module, expression, import);
        }
        let binding = resolve_binding_in(&self.modules, module, expression, Some(&self.session))?;
        if let Binding::Definition(id) = binding
            && self.builtin_before_function(module, expression, id)
        {
            return None;
        }
        Some(binding)
    }

    pub(crate) fn builtin_function_shadow(
        &self,
        name: &str,
        binding: Binding,
    ) -> Option<&ast::StmtFunctionDef> {
        let Binding::Definition(id) = binding else {
            return None;
        };
        if !crate::frontend::reserved_builtin_function(name) {
            return None;
        }
        self.modules[id.module].parsed.syntax().body[id.statement].as_function_def_stmt()
    }

    /// A bare read during module initialization can still reach the builtin before a local def.
    fn builtin_before_function(
        &self,
        module: ModuleId,
        expression: &ast::Expr,
        id: DefinitionId,
    ) -> bool {
        let ast::Expr::Name(name) = expression else {
            return false;
        };
        let statement = &self.modules[id.module].parsed.syntax().body[id.statement];
        id.module == module
            && name.start() < statement.end()
            && statement
                .as_function_def_stmt()
                .is_some_and(|function| function.name.id == name.id)
            && crate::frontend::reserved_builtin_function(name.id.as_str())
            && self.session.is_unbound_module_read(module, expression)
    }

    /// Recognize Protocol and Protocol[T] at the point where the class is defined.
    pub(crate) fn is_protocol_base(&self, module: ModuleId, expression: &ast::Expr) -> bool {
        let origin = match expression {
            ast::Expr::Subscript(subscript) => &*subscript.value,
            base => base,
        };
        matches!(
            self.resolve(module, origin),
            Some(Binding::Intrinsic(Intrinsic::TYPING_PROTOCOL))
        ) && self.session.is_typing_protocol(module, origin)
    }

    /// An import binds its name throughout the function, even before the import executes.
    fn local_static_binding(
        &self,
        module: ModuleId,
        expression: &ast::Expr,
    ) -> Option<&LocalStaticImport> {
        let imports = &self.modules[module].local_static_imports;
        if imports.is_empty() {
            return None;
        }
        let mut root = expression;
        while let ast::Expr::Attribute(attr) = root {
            root = &attr.value;
        }
        let name = root.as_name_expr()?.id.as_str();
        imports
            .iter()
            .find(|i| i.name == name && i.body.contains_range(expression.range()))
    }

    fn local_import_value(
        &self,
        module: ModuleId,
        expression: &ast::Expr,
        import: &LocalStaticImport,
    ) -> Option<Binding> {
        if expression.start() < import.initialized {
            return None;
        }
        if import.module_name == "importlib.metadata" {
            let attr = expression.as_attribute_expr()?;
            return self
                .session
                .is_stdlib_module(module, &attr.value, "importlib.metadata")
                .then(|| {
                    stdlib_symbol(
                        &self.modules,
                        IntrinsicModule::ImportlibMetadata,
                        attr.attr.as_str(),
                    )
                    .ok()
                })?;
        }
        let (root, attribute) = match expression {
            ast::Expr::Attribute(attr) => (&*attr.value, Some(attr.attr.as_str())),
            ast::Expr::Name(_) => (expression, None),
            _ => return None,
        };
        if !self.session.has_import_binding(
            module,
            root,
            import.alias,
            &self.modules[module].parsed,
        ) {
            return None;
        }
        let owner = IntrinsicModule::named(import.module_name)?;
        match (&import.symbol, attribute) {
            (None, None) => Some(Binding::IntrinsicModule(owner)),
            (None, Some(attr)) => owner.symbol(attr),
            (Some(symbol), None) => owner.symbol(symbol),
            (Some(_), Some(_)) => None,
        }
    }

    /// The loader has already resolved this import and recorded the scope of its binding.
    pub(crate) fn local_static_import(&self, module: ModuleId, stmt: &ast::Stmt) -> bool {
        matches!(stmt, ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_))
            && self.modules[module]
                .local_static_imports
                .iter()
                .any(|i| i.initialized == stmt.end())
    }

    /// A postponed annotation may reference a type imported under TYPE_CHECKING.
    pub(crate) fn resolve_type(&self, module: ModuleId, expression: &ast::Expr) -> Option<Binding> {
        if let Some(import) = self.local_static_binding(module, expression) {
            return self.local_import_value(module, expression, import);
        }
        match expression {
            ast::Expr::Name(name) => self.modules[module]
                .type_bindings
                .get(name.id.as_str())
                .copied()
                .or_else(|| self.resolve(module, expression)),
            ast::Expr::Attribute(attribute) => match self.resolve_type(module, &attribute.value)? {
                Binding::Module(target) => self.modules[target].export(attribute.attr.as_str()),
                Binding::IntrinsicModule(target) => {
                    stdlib_attribute(&self.modules, module, target, attribute).ok()
                }
                _ => None,
            },
            _ => None,
        }
    }
}

struct LocalStaticImport {
    name: String,
    module_name: &'static str,
    symbol: Option<String>,
    alias: TextRange,
    body: TextRange,
    initialized: TextSize,
}

/// An unconditional function-local import dominates the remaining statements in that body.
/// The semantic model checks the receiver at each use, including local rebindings.
/// Collect the type declarations and static bindings of function-local imports.
fn function_imports(
    f: &ast::StmtFunctionDef,
    imports: &mut Vec<LocalStaticImport>,
    mut declaration: impl FnMut(&str),
) {
    for stmt in &f.body {
        // These modules have no observable initialization or mutable exports.
        // Other local imports still take their ordinary lowering path.
        let (alias, module_name, symbol, default_name) = match stmt {
            ast::Stmt::Import(import) if !import.is_lazy => {
                for alias in &import.names {
                    declaration(alias.name.as_str());
                }
                let [alias] = import.names.as_slice() else {
                    continue;
                };
                match alias.name.as_str() {
                    "importlib.metadata" => (alias, "importlib.metadata", None, "importlib"),
                    "fnmatch" => (alias, "fnmatch", None, "fnmatch"),
                    "re" => (alias, "re", None, "re"),
                    _ => continue,
                }
            }
            ast::Stmt::ImportFrom(import) if !import.is_lazy && import.level == 0 => {
                if let Some(module) = import.module.as_deref() {
                    declaration(module);
                }
                let [alias] = import.names.as_slice() else {
                    continue;
                };
                let owner = match import.module.as_deref() {
                    Some("fnmatch") => "fnmatch",
                    Some("re") => "re",
                    _ => continue,
                };
                let name = alias.name.as_str();
                if !matches!(IntrinsicModule::named(owner).and_then(|m| m.symbol(name)), Some(Binding::Intrinsic(symbol)) if !matches!(symbol, Intrinsic::Declaration(_)))
                {
                    continue;
                }
                (alias, owner, Some(name.to_owned()), name)
            }
            _ => continue,
        };
        imports.push(LocalStaticImport {
            name: alias
                .asname
                .as_ref()
                .map_or(default_name, |n| n.as_str())
                .into(),
            module_name,
            symbol,
            alias: alias.range(),
            body: TextRange::new(f.body[0].start(), f.end()),
            initialized: stmt.end(),
        });
    }
}

fn importlib_child(name: &str) -> Option<IntrinsicModule> {
    match name {
        "abc" => Some(IntrinsicModule::ImportlibAbc),
        "_abc" => Some(IntrinsicModule::ImportlibLoaderAbc),
        "machinery" => Some(IntrinsicModule::ImportlibMachinery),
        "_bootstrap_external" => Some(IntrinsicModule::ImportlibBootstrapExternal),
        "metadata" => Some(IntrinsicModule::ImportlibMetadata),
        "readers" => Some(IntrinsicModule::ImportlibReaders),
        "resources" => Some(IntrinsicModule::ImportlibResources),
        "util" => Some(IntrinsicModule::ImportlibUtil),
        _ => None,
    }
}

fn importlib_child_name(module: IntrinsicModule) -> Option<&'static str> {
    match module {
        IntrinsicModule::ImportlibAbc => Some("abc"),
        IntrinsicModule::ImportlibLoaderAbc => Some("_abc"),
        IntrinsicModule::ImportlibMachinery => Some("machinery"),
        IntrinsicModule::ImportlibBootstrapExternal => Some("_bootstrap_external"),
        IntrinsicModule::ImportlibMetadata => Some("metadata"),
        IntrinsicModule::ImportlibReaders => Some("readers"),
        IntrinsicModule::ImportlibResources => Some("resources"),
        IntrinsicModule::ImportlibUtil => Some("util"),
        _ => None,
    }
}

fn stdlib_attribute(
    modules: &[SourceModule],
    owner: ModuleId,
    target: IntrinsicModule,
    attr: &ast::ExprAttribute,
) -> Result<Binding, StdlibMiss> {
    if target == IntrinsicModule::Importlib
        && modules[owner]
            .importlib_children
            .get(attr.attr.as_str())
            .is_some_and(|at| *at <= attr.start())
        && let Some(child) = importlib_child(attr.attr.as_str())
    {
        return Ok(Binding::IntrinsicModule(child));
    }
    stdlib_symbol(modules, target, attr.attr.as_str())
}

fn resolve_binding(
    modules: &[SourceModule],
    module: ModuleId,
    expression: &ast::Expr,
) -> Option<Binding> {
    resolve_binding_in(modules, module, expression, None)
}

fn resolve_binding_in(
    modules: &[SourceModule],
    module: ModuleId,
    expression: &ast::Expr,
    session: Option<&Session>,
) -> Option<Binding> {
    match expression {
        ast::Expr::Name(name) => modules[module].bindings.get(name.id.as_str()).copied(),
        ast::Expr::Attribute(attribute) => {
            match resolve_binding(modules, module, &attribute.value)? {
                Binding::Module(module) => modules[module].export(attribute.attr.as_str()),
                Binding::IntrinsicModule(target) => {
                    match stdlib_attribute(modules, module, target, attribute) {
                        Ok(binding) => Some(binding),
                        Err(StdlibMiss::Sealed) => None,
                        Err(StdlibMiss::Unclaimed) => {
                            let session = session?;
                            let owner = session.module_name_at(module, &attribute.value)?;
                            if IntrinsicModule::named(owner) != Some(target) {
                                return None;
                            }
                            session.shipped_declaration(owner, attribute.attr.as_str())
                        }
                    }
                }
                Binding::Definition(_)
                | Binding::RegisteredClass(_)
                | Binding::RegisteredValue(_)
                | Binding::ConditionalValue
                | Binding::RuntimeClass
                | Binding::Value(_)
                | Binding::ImportedValue(_, _)
                | Binding::TypeAlias(..)
                | Binding::Intrinsic(_) => None,
            }
        }
        _ => None,
    }
}

pub(crate) const LOGGING_CLASSES: &str = "__imre_logging_classes__";
const LOGGING_HANDLER_CLASSES: &str = "__imre_logging_handler_classes__";
pub(crate) const CONTEXTLIB_ABSTRACT: &str = "__imre_contextlib_abstract__";
const CONTEXTLIB_EXIT_STACK: &str = "__imre_contextlib_exit_stack__";
const METADATA_VERSION: &str = "__imre_metadata_version__";
const INSPECT_FUNCTIONS: &str = "__imre_inspect_functions__";
const TEMPFILE_SPOOL: &str = "__imre_tempfile_spool__";
const TEXTWRAP_FUNCTIONS: &str = "__imre_textwrap_functions__";
pub(crate) const FRACTION_CLASS: &str = "__imre_fraction_class__";
pub(crate) const CALENDAR_CLASSES: &str = "__imre_calendar_classes__";
const TEXTWRAP_WRAPPER: &str = "__imre_textwrap_wrapper__";

fn textwrap_implementation(name: &str) -> Option<&'static str> {
    match name {
        "indent" => Some(TEXTWRAP_FUNCTIONS),
        "TextWrapper" | "wrap" | "fill" | "shorten" => Some(TEXTWRAP_WRAPPER),
        _ => None,
    }
}

enum StdlibMiss {
    Sealed,
    Unclaimed,
}

fn checked_stdlib_owner(module: IntrinsicModule, name: &str) -> Option<&'static str> {
    match (module, name) {
        (IntrinsicModule::Dataclasses, "MISSING" | "_MISSING_TYPE") => Some("dataclasses"),
        (IntrinsicModule::Inspect, "cleandoc") => Some("inspect"),
        _ => None,
    }
}

/// A selected checked interface owns its misses as well as its public definitions.
fn stdlib_symbol(
    modules: &[SourceModule],
    module: IntrinsicModule,
    name: &str,
) -> Result<Binding, StdlibMiss> {
    if let Some(owner) = checked_stdlib_owner(module, name)
        && let Some(source) = modules.iter().find(|source| source.is_bundled(owner))
    {
        return source.export(name).ok_or(StdlibMiss::Sealed);
    }
    if module == IntrinsicModule::Selectors
        && matches!(
            name,
            "PollSelector" | "DefaultSelector" | "SelectorKey" | "EVENT_READ" | "EVENT_WRITE"
        )
        && let Some(source) = modules.iter().find(|source| source.is_bundled("selectors"))
    {
        return source.export(name).ok_or(StdlibMiss::Sealed);
    }
    if let Some(binding) = module.symbol(name) {
        return Ok(binding);
    }
    let implementation = match module {
        IntrinsicModule::Contextlib if name == "AbstractContextManager" => CONTEXTLIB_ABSTRACT,
        IntrinsicModule::Contextlib if name == "ExitStack" => CONTEXTLIB_EXIT_STACK,
        IntrinsicModule::ImportlibMetadata if name == "version" => METADATA_VERSION,
        IntrinsicModule::Tempfile if name == "SpooledTemporaryFile" => TEMPFILE_SPOOL,
        IntrinsicModule::Logging => (!name.starts_with('_'))
            .then_some(LOGGING_CLASSES)
            .ok_or(StdlibMiss::Sealed)?,
        IntrinsicModule::LoggingHandlers => (!name.starts_with('_'))
            .then_some(LOGGING_HANDLER_CLASSES)
            .ok_or(StdlibMiss::Sealed)?,
        IntrinsicModule::Fractions if name == "Fraction" => FRACTION_CLASS,
        IntrinsicModule::Calendar if matches!(name, "Calendar" | "TextCalendar") => {
            CALENDAR_CLASSES
        }
        IntrinsicModule::Textwrap => textwrap_implementation(name).ok_or(StdlibMiss::Sealed)?,
        _ => return Err(StdlibMiss::Unclaimed),
    };
    let (id, source) = modules
        .iter()
        .enumerate()
        .find(|(_, m)| m.bundled && m.name == implementation)
        .ok_or(StdlibMiss::Sealed)?;
    source.export(name).filter(|b| {
            matches!(b, Binding::Definition(d) if d.module == id &&
                matches!((module, &source.parsed.syntax().body[d.statement]),
                    (IntrinsicModule::Calendar | IntrinsicModule::Fractions | IntrinsicModule::Contextlib, ast::Stmt::ClassDef(_))
                    | (IntrinsicModule::ImportlibMetadata, ast::Stmt::FunctionDef(_))
                    | (IntrinsicModule::Tempfile, ast::Stmt::ClassDef(_))
                    | (IntrinsicModule::Logging, ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_))
                    | (IntrinsicModule::LoggingHandlers, ast::Stmt::ClassDef(_))
                    | (IntrinsicModule::Textwrap, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))))
        }).ok_or(StdlibMiss::Sealed)
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum EntryImportFailure {
    AfterStatement,
    MissingModule,
    ZipImport,
    RuntimeProvider,
}

/// File probes that affect checked admission, even when the program later uses object imports.
#[derive(Default)]
pub(crate) struct SiteProbes {
    pub(crate) wheel_records: crate::installed_wheels::WheelRecords,
    /// Canonical import root inferred for a file build, including when its checked scan fails.
    pub(super) package_root: Option<PathBuf>,
    pub(super) packaged_file_module: Option<String>,
    missing: Vec<PathBuf>,
    selected: bool,
    file_checks: BTreeMap<PathBuf, bool>,
}

impl SiteProbes {
    fn is_file(&mut self, fs: &mut FileSystem<'_>, path: PathBuf) -> bool {
        *self
            .file_checks
            .entry(path)
            .or_insert_with_key(|p| fs.is_file(p))
    }

    fn paired(&mut self, fs: &mut FileSystem<'_>, archive: &Path, stub: &Path) -> bool {
        for file in [archive, stub] {
            if !fs.is_file(file) {
                self.missing.push(file.to_owned());
                return false;
            }
        }
        self.selected = true;
        true
    }

    pub(crate) fn record(&self, inputs: &mut crate::SourceInputs) {
        inputs.missing.extend_from_slice(&self.missing);
        inputs.file_checks.extend(self.file_checks.clone());
        inputs.relink_site_wheel = self.selected && inputs.boxed_imports;
    }
}

/// Import path, canonical identity, package flag, search-root index and native archive.
type ModuleFile = (PathBuf, PathBuf, bool, usize, Option<NativeArchive>);

enum ModuleText<'a> {
    Text(std::borrow::Cow<'a, str>, crate::SourceKind),
    Disk(crate::DiskSource, String),
}

impl ModuleText<'_> {
    fn disk(path: &Path, bytes: Vec<u8>) -> Result<Self, String> {
        let hash = blake3::hash(&bytes).to_hex().to_string();
        Ok(Self::Disk(crate::DiskSource::decode(path, bytes)?, hash))
    }
}

struct Loader<'a> {
    local_declarations: BTreeSet<String>,
    python_optimization: i32,
    module_operand: Option<String>,
    fs: FileSystem<'a>,
    /// ty only searches typeshed for builtins; record sealed imports missing from its inventory.
    checked_builtin_imports: Vec<(ModuleId, TextRange)>,
    needs_object_imports: bool,
    runtime_initializer: Option<String>,
    has_checked_only_import: bool,
    imports_runtime_class: bool,
    wildcard_import: Option<String>,
    inputs: crate::SourceInputs,
    zip_files: Vec<(PathBuf, PathBuf, String)>,
    zip_sources: HashMap<PathBuf, crate::zip_entry::ZipSources>,
    root: PathBuf,
    search_roots: Vec<PathBuf>,
    archive_roots: usize,
    site_probes: Option<(HashSet<PathBuf>, &'a mut SiteProbes)>,
    session: Session,
    modules: Vec<SourceModule>,
    by_name: HashMap<String, ModuleId>,
    by_path: HashMap<PathBuf, ModuleId>,
    loading: HashSet<ModuleId>,
    /// A from-import of a still-initializing module observes the value at that point.
    partial_value_imports: HashSet<DefinitionId>,
    ordered_imports: bool,
    import_failure: Option<EntryImportFailure>,
    import_depth: usize,
    allow_imports: bool,
    skip_current_root: bool,
    frozen_modules: bool,
    reachable_only: bool,
    initialization: Vec<(ModuleId, Range<usize>)>,
    pub(crate) errno_names: Vec<String>,
}

impl<'a> Loader<'a> {
    fn new(
        root: PathBuf,
        extension_paths: &[PathBuf],
        python_archive_indices: &[usize],
        python: Option<(&crate::CompileOptions, &'a mut SiteProbes)>,
        mut fs: FileSystem<'a>,
    ) -> Result<Self, String> {
        let mut search_roots = Vec::new();
        let mut zip_sources = HashMap::new();
        let mut zip_inputs = crate::SourceInputs::default();
        let mut inputs = crate::SourceInputs::default();
        for (index, path) in extension_paths.iter().enumerate() {
            let logical = std::path::absolute(path).map_err(|e| e.to_string())?;
            if python_archive_indices.contains(&index) {
                if let std::collections::hash_map::Entry::Vacant(entry) = zip_sources.entry(logical)
                {
                    let archive =
                        crate::zip_entry::ZipSources::open(entry.key(), &mut zip_inputs, &mut fs)?;
                    search_roots.push(entry.key().clone());
                    entry.insert(archive);
                }
                continue;
            }
            let directory = fs
                .canonicalize(path)
                .map_err(|e| format!("invalid extension path {}: {e}", path.display()))?;
            if !fs.is_dir(&directory) {
                return Err(format!(
                    "extension path must be a directory: {}",
                    path.display()
                ));
            }
            inputs.directories.push((logical, directory.clone()));
            if !search_roots.contains(&directory) {
                search_roots.push(directory);
            }
        }
        let archive_roots = search_roots.len();
        let mut site_roots = HashSet::new();
        let mut site_probes = None;
        let python_optimization = python
            .as_ref()
            .and_then(|(options, _)| options.python_startup())
            .map_or(0, |(flags, _)| flags.optimize);
        if let Some((options, probes)) = python {
            for dir in options.installed_dirs() {
                let canonical = fs.canonicalize(dir).map_err(|e| e.to_string())?;
                inputs.directories.push((dir.to_owned(), canonical.clone()));
                if options.site_dirs.contains(dir) {
                    site_roots.insert(canonical.clone());
                }
                if !search_roots.contains(&canonical) {
                    search_roots.push(canonical);
                }
            }
            site_probes = Some((site_roots, probes));
        }
        Ok(Self {
            python_optimization,
            module_operand: None,
            fs,
            local_declarations: BTreeSet::new(),
            archive_roots,
            site_probes,
            session: Session::new(&root, &search_roots)?,
            search_roots,
            inputs,
            zip_sources,
            zip_files: zip_inputs.files,
            checked_builtin_imports: Vec::new(),
            needs_object_imports: false,
            runtime_initializer: None,
            has_checked_only_import: false,
            imports_runtime_class: false,
            wildcard_import: None,
            root,
            modules: Vec::new(),
            by_name: HashMap::new(),
            by_path: HashMap::new(),
            loading: HashSet::new(),
            partial_value_imports: HashSet::new(),
            ordered_imports: false,
            import_failure: None,
            import_depth: 0,
            allow_imports: true,
            skip_current_root: false,
            frozen_modules: true,
            reachable_only: false,
            initialization: Vec::new(),
            errno_names: Vec::new(),
        })
    }

    fn add(
        &mut self,
        path: PathBuf,
        identity: PathBuf,
        name: String,
        is_package: bool,
        source: &str,
    ) -> Result<ModuleId, String> {
        self.add_source(
            path,
            identity,
            name,
            is_package,
            ModuleText::Text(source.into(), crate::SourceKind::File),
        )
    }

    fn add_source(
        &mut self,
        path: PathBuf,
        identity: PathBuf,
        name: String,
        is_package: bool,
        input: ModuleText<'_>,
    ) -> Result<ModuleId, String> {
        let (text, kind, hash, raw_digest) = match input {
            ModuleText::Text(text, kind) => {
                let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
                (text, kind, hash, None)
            }
            ModuleText::Disk(source, hash) => (
                source.text.into(),
                crate::SourceKind::DiskFile,
                hash,
                source.raw_digest,
            ),
        };
        self.inputs
            .files
            .push((path.clone(), identity.clone(), hash));
        let (source, original_source) = match crate::normalize_source(&text, kind)
            .map_err(|error| format!("{}: {error}", path.display()))?
        {
            std::borrow::Cow::Borrowed(_) => (text.into_owned(), None),
            std::borrow::Cow::Owned(normalized) => {
                (normalized, Some(text.into_owned().into_boxed_str()))
            }
        };
        let parsed = self.session.add(&path, &source, kind)?;
        let id = self.modules.len();
        self.by_name.insert(name.clone(), id);
        self.by_path.insert(identity, id);
        self.loading.insert(id);
        self.modules.push(SourceModule {
            path,
            name,
            source,
            original_source,
            raw_digest,
            doctest_documents: Default::default(),
            registered_values: Default::default(),
            doctest_prompts: Vec::new(),
            bundled: false,
            registered_namespace: false,
            parsed,
            is_package,
            search_roots: SearchRoots::Single(0),
            entry_ownership: None,
            archive: None,
            bindings: HashMap::new(),
            type_bindings: HashMap::new(),
            importlib_children: HashMap::new(),
            local_static_imports: Vec::new(),
            aliases: Vec::new(),
            overloads: HashSet::new(),
            type_guards: Vec::new(),
            entry_globals: Vec::new(),
            optional_imports: HashMap::new(),
            rebound_values: HashSet::new(),
            registered_imports: HashMap::new(),
            snapshots: HashMap::new(),
            executable: Vec::new(),
        });
        Ok(id)
    }

    fn finish(
        mut self,
        entry: ModuleId,
        entry_filename: PathBuf,
        reachable_only: bool,
        profile: Option<&Path>,
    ) -> Result<SourceGraph, String> {
        let mut declarations = std::mem::take(&mut self.local_declarations);
        // Resolve against the completed graph. Type-only imports neither run a
        // module initializer nor introduce runtime bindings or import cycles.
        for id in 0..self.modules.len() {
            let parsed = self.modules[id].parsed.clone();
            for index in self.modules[id].type_guards.clone() {
                let ast::Stmt::If(guard) = &parsed.syntax().body[index] else {
                    continue; // ordinary runtime-visible type declarations have no guarded imports
                };
                for statement in &guard.body {
                    if let ast::Stmt::Import(import) = statement {
                        for alias in &import.names {
                            let full = alias.name.as_str();
                            let name = if alias.asname.is_some() {
                                full
                            } else {
                                full.split('.').next().unwrap()
                            };
                            let found = self
                                .by_name
                                .get(name)
                                .copied()
                                .map(Binding::Module)
                                .or_else(|| {
                                    IntrinsicModule::named(name).map(Binding::IntrinsicModule)
                                });
                            if let Some(binding) = found {
                                let local = alias.asname.as_ref().map_or(name, |id| id.as_str());
                                self.modules[id]
                                    .type_bindings
                                    .insert(local.to_owned(), binding);
                            }
                        }
                        continue;
                    }
                    let ast::Stmt::ImportFrom(import) = statement else {
                        continue;
                    };
                    let name = self.import_name(id, import)?;
                    for alias in &import.names {
                        let binding = self
                            .by_name
                            .get(&name)
                            .and_then(|&target| {
                                let target = &self.modules[target];
                                target
                                    .type_bindings
                                    .get(alias.name.as_str())
                                    .copied()
                                    .or_else(|| target.export(alias.name.as_str()))
                            })
                            .or_else(|| {
                                stdlib_symbol(
                                    &self.modules,
                                    IntrinsicModule::named(&name)?,
                                    alias.name.as_str(),
                                )
                                .ok()
                            });
                        if let Some(binding) = binding {
                            let local = alias.asname.as_ref().unwrap_or(&alias.name);
                            self.modules[id]
                                .type_bindings
                                .insert(local.to_string(), binding);
                        }
                    }
                    declarations.insert(name);
                }
            }
        }
        let doctest_imports = self.doctest_program()?;
        let guard_search = self.inputs.missing.len();
        for name in declarations {
            if !self.by_name.contains_key(&name) {
                self.add_declaration_module(&name)?;
            }
        }
        // A previously absent declaration may appear before the next cached build.
        for path in self.inputs.missing.drain(guard_search..) {
            self.inputs.file_checks.insert(path, false);
        }
        self.inputs.missing.sort_unstable();
        self.inputs.missing.dedup();
        let mut graph = SourceGraph {
            filesystem_roots: std::iter::once(self.root)
                .skip(usize::from(self.skip_current_root))
                .chain(self.search_roots)
                .collect(),
            doctest_imports,
            command_startup: None,
            virtual_command: false,
            traceback_objects: false,
            handled_tracebacks: self.modules.iter().any(|module| {
                module.bindings.values().any(|binding| {
                    matches!(
                        binding,
                        Binding::IntrinsicModule(IntrinsicModule::Traceback)
                            | Binding::Intrinsic(
                                Intrinsic::TracebackPrintExc | Intrinsic::TracebackFormatter(_)
                            )
                    )
                })
            }),
            empty_keywords: empty_keywords::EmptyKeywords::default(),
            checked_builtin_imports: self.checked_builtin_imports,
            checked_diagnostics: std::cell::RefCell::new(vec![Vec::new(); self.modules.len()]),
            float_components: Default::default(),
            // An import that cannot be initialized here must not turn an unused
            // object-capable namespace into a reason to box the caller.
            needs_object_runtime: std::cell::Cell::new(
                self.needs_object_imports && self.runtime_initializer.is_none(),
            ),
            has_object_constructor_fallback: Default::default(),
            registered_functions: Default::default(),
            registered_source_errors: Default::default(),
            registered_startup: false,
            source_contract: None,
            closed_initializer: None,
            layout_hint: std::cell::Cell::new(LayoutHint::Available),
            check_fallback_calls: Default::default(),
            imports_runtime_class: self.imports_runtime_class,
            check_method_factories: Default::default(),
            class_layout_error: Default::default(),
            runtime_initializer: self.runtime_initializer,
            has_checked_only_import: self.has_checked_only_import,
            deferred_object_error: self.wildcard_import.map(Into::into).into(),
            entry_profile: profile
                .map(|p| entry_profile::Profile::load(p, &self.modules, &mut self.fs))
                .transpose()?
                .unwrap_or_default(),
            mutable_builtin_len: false,
            reachable: None,
            required_library: None,
            entry_filename,
            inputs: self.inputs,
            zip_files: self.zip_files,
            session: self.session,
            ordered_bindings: self
                .ordered_imports
                .then(|| ordered_imports::Bindings::new(&self.modules)),
            modules: self.modules,
            by_name: self.by_name,
            entry,
            initialization: self.initialization,
            errno_names: self.errno_names,
        };
        if reachable_only {
            graph.reachable = Some(Arc::new(reachability::Reachable::collect(&graph, false)));
        } else if let Some(library) = graph
            .modules
            .iter()
            .position(|m| m.is_bundled(CONTEXTLIB_EXIT_STACK))
        {
            // A sealed library can own higher-order methods that require object dispatch.
            // Strict builds still visit even an uncalled first-party function using them.
            graph.required_library =
                Some((library, reachability::Reachable::collect(&graph, true)));
        }
        graph.empty_keywords = empty_keywords::EmptyKeywords::collect(&graph);
        graph.refresh_traceback_objects();
        graph.mutable_builtin_len = graph.writes_builtin_len();
        Ok(graph)
    }

    /// A valid Python entry may resolve certain failed imports when it executes.
    fn scan_python_entry(
        &mut self,
        id: ModuleId,
        failure: &mut Option<EntryImportFailure>,
    ) -> Result<(), String> {
        self.scan(id).inspect_err(|_| {
            if !self.has_checked_only_import
                && self.import_failure.is_some()
                && self.modules[id].parsed.has_no_syntax_errors()
                && self.session.has_valid_semantic_syntax(id)
            {
                *failure = self.import_failure;
            }
        })
    }

    fn scan(&mut self, id: ModuleId) -> Result<(), String> {
        let is_entry = self.is_entry(id);
        let parsed = self.modules[id].parsed.clone();
        let global_writes = self.session.nested_global_writes(id, parsed.tokens());
        let mut imports = Vec::new();
        let first_party = self.modules[id].is_program_source();
        let mut declaration = |name: &str| {
            if first_party {
                self.local_declarations.insert(name.to_owned());
            }
        };
        for stmt in &parsed.syntax().body {
            match stmt {
                ast::Stmt::FunctionDef(f) => function_imports(f, &mut imports, &mut declaration),
                ast::Stmt::ClassDef(c) => {
                    for f in c.body.iter().filter_map(ast::Stmt::as_function_def_stmt) {
                        function_imports(f, &mut imports, &mut declaration);
                    }
                }
                _ => {}
            }
        }
        if !imports.is_empty() {
            for module_name in imports
                .iter()
                .map(|import| import.module_name)
                .collect::<std::collections::BTreeSet<_>>()
            {
                self.intrinsic_module(module_name, None)?;
            }
            // Typeshed omits CPython's private _translate companion. The import
            // was resolved above against the sealed module's real export.
            self.checked_builtin_imports.extend(
                imports
                    .iter()
                    .filter(|i| {
                        i.module_name == "fnmatch" && i.symbol.as_deref() == Some("_translate")
                    })
                    .map(|i| (id, i.alias)),
            );
            self.modules[id].local_static_imports = imports;
        }
        let last_definition = parsed.syntax().body.iter().rposition(|statement| {
            matches!(
                statement,
                ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)
            )
        });
        let mut executable = false;
        let mut segment = 0;
        let mut definition_prefix = false;
        let mut field_separators = Vec::new();
        let mut runtime_classes = Vec::new();
        for (index, statement) in parsed.syntax().body.iter().enumerate() {
            if self.modules[id].archive.is_some() && !native_declaration(statement) {
                return Err(self.modules[id].error(statement,
                    "native stubs allow type-only imports and fully annotated, required positional-only methods with an ellipsis body"));
            }
            if let Some((name, slot)) = self.modules[id].import_config_slot(statement) {
                self.bind(id, name, Binding::Intrinsic(slot), statement)?;
                continue;
            }
            match statement {
                ast::Stmt::If(guard)
                    if resolve_binding(&self.modules, id, &guard.test)
                        == Some(Binding::Intrinsic(Intrinsic::TypeChecking))
                        && guard.elif_else_clauses.is_empty()
                        && guard.body.iter().all(type_aliases::type_statement) =>
                {
                    self.modules[id].type_guards.push(index);
                    for stmt in &guard.body {
                        if let (Some(name), Some(value)) = (value_name(stmt), module_value(stmt)) {
                            let known = self.add_alias(id, TypeAlias::Expression(value.clone()));
                            self.modules[id]
                                .type_bindings
                                .insert(name.to_owned(), known);
                        }
                    }
                }
                ast::Stmt::Try(guard)
                    if (is_entry || self.partial_source(id))
                        && !executable
                        && self.modules[id].entry_globals.is_empty()
                        && let Some((module, names)) =
                            optional_stdlib_import(&self.modules, id, guard) =>
                {
                    self.reject_stdlib_shadow(module)?;
                    for name in &names {
                        self.bind(id, name, Binding::ConditionalValue, statement)?;
                    }
                    self.modules[id].optional_imports.insert(
                        statement.range(),
                        names.into_iter().map(str::to_owned).collect(),
                    );
                    self.modules[id].executable.push(index);
                }
                _ if self.alias_declaration(id, statement)? => {
                    self.modules[id].type_guards.push(index);
                }
                ast::Stmt::TypeAlias(alias) if let Some(name) = alias.name.as_name_expr() => {
                    let binding = self.add_alias(id, TypeAlias::Declaration(index));
                    self.bind(id, name.id.as_str(), binding, statement)?;
                    // Its initializer binds the object in source order. The alias
                    // body can first run when an ordinary operand reads __value__.
                    self.modules[id].executable.push(index);
                }
                ast::Stmt::FunctionDef(function) => {
                    definition_prefix |= self.partial_source(id);
                    let name = function.name.as_str();
                    let current = Binding::Definition(DefinitionId {
                        module: id,
                        statement: index,
                    });
                    let decorated = matches!(function.decorator_list.as_slice(), [d]
                        if resolve_binding(&self.modules, id, &d.expression) == Some(Binding::Intrinsic(Intrinsic::Overload)));
                    let continues = matches!(self.modules[id].bindings.get(name), Some(Binding::Definition(previous))
                        if previous.module == id && previous.statement + 1 == index && self.modules[id].overloads.contains(&previous.statement));
                    if continues {
                        // Nothing can observe the intermediate name between these
                        // consecutive definitions. Intervening statements still go
                        // through the ordinary static rebind rejection.
                        self.modules[id].bindings.insert(name.into(), current);
                    } else if let Some(Binding::Definition(first) | Binding::Value(first)) =
                        self.modules[id].bindings.get(name).copied()
                        && first.module == id
                        && parsed.syntax().body[first.statement].is_function_def_stmt()
                        && self.modules[id].is_program_source()
                        && !decorated
                        && !self.modules[id].overloads.contains(&first.statement)
                        && !global_writes.contains(name)
                        && (self.modules[id].rebound_values.contains(&first.statement)
                            || self.session.module_function_declarations(id, name))
                    {
                        let module = &mut self.modules[id];
                        module.rebound_values.insert(first.statement);
                        module
                            .bindings
                            .insert(name.to_owned(), Binding::Value(first));
                    } else {
                        self.bind(id, name, current, statement)?;
                    }
                    if decorated {
                        self.modules[id].overloads.insert(index);
                    }
                }
                ast::Stmt::ClassDef(class) => {
                    definition_prefix |= self.partial_source(id)
                        || class
                            .body
                            .iter()
                            .any(|stmt| matches!(stmt, ast::Stmt::Assign(_)));
                    self.bind(
                        id,
                        class.name.as_str(),
                        Binding::Definition(DefinitionId {
                            module: id,
                            statement: index,
                        }),
                        statement,
                    )?;
                }
                ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) => {
                    if executable || !self.modules[id].entry_globals.is_empty() {
                        // Emit accepted prefixes before the dependency starts:
                        // a literal can fail to allocate, and a guard can raise.
                        let module = &self.modules[id];
                        let prefix = module
                            .executable
                            .iter()
                            .rev()
                            .take_while(|&&at| at >= segment)
                            .map(|&at| &parsed.syntax().body[at]);
                        let simple = prefix.clone().all(|stmt| {
                            module_name_guard(stmt)
                                || conditional_imports(&self.modules, id, stmt).is_some()
                                || (module.is_package
                                    && value_name(stmt).is_some()
                                    && module_value(stmt).is_some_and(literal_export))
                        });
                        let nested_source = module.name.contains('.') && !module.bundled;
                        let ordered = !module.is_package
                            && module.archive.is_none()
                            && prefix.clone().all(|stmt| {
                                module_name_guard(stmt)
                                    || value_name(stmt).is_some()
                                        && module_value(stmt).is_some_and(|value| {
                                            nested_source
                                                || literal_export(value)
                                                    && module.rebound_target(id, stmt).is_none()
                                        })
                                    || self.partial_source(id) && matches!(stmt, ast::Stmt::Expr(_))
                            });
                        if is_entry || !simple && !ordered {
                            self.import_failure =
                                is_entry.then_some(EntryImportFailure::AfterStatement);
                            return Err(self.modules[id].error(
                                statement,
                                "imports must precede executable module statements",
                            ));
                        }
                        self.ordered_imports |= !simple;
                        self.initialization.push((id, segment..index));
                        segment = index;
                        executable = false;
                    } else if definition_prefix {
                        // A child can call a definition that already exists in this module.
                        self.initialization.push((id, segment..index));
                        segment = index;
                    }
                    definition_prefix = false;
                    if self.modules[id]
                        .snapshots
                        .keys()
                        .any(|at| (segment..index).contains(at))
                    {
                        self.initialization.push((id, segment..index));
                        segment = index;
                    }
                    self.import(id, index, statement)
                        .map_err(|error| self.modules[id].error(statement, &error))?;
                    if errno_import(statement).is_some() {
                        // The target decides which forward symbols exist. A failed from-import
                        // must precede the next imported module, even if its alias is unused.
                        self.initialization.push((id, segment..index + 1));
                        segment = index + 1;
                    }
                }
                ast::Stmt::Expr(value)
                    if matches!(&*value.value, ast::Expr::StringLiteral(_)) && !executable => {}
                ast::Stmt::Assign(_)
                    if value_name(statement).is_some()
                        && matches!(module_value(statement), Some(ast::Expr::Call(call))
                        if resolve_binding(&self.modules, id, &call.func) == Some(Binding::Intrinsic(Intrinsic::NamedTuple))) =>
                {
                    if let Some(ast::Expr::Call(call)) = module_value(statement)
                        && let [_, fields] = call.arguments.args.as_ref()
                    {
                        namedtuple_fields::separators(
                            fields,
                            &self.modules[id].source,
                            &mut field_separators,
                        );
                    }
                    if executable && is_entry {
                        return Err(self.modules[id].error(
                            statement,
                            "definitions must precede executable module statements",
                        ));
                    }
                    self.bind(
                        id,
                        value_name(statement).unwrap(),
                        Binding::Definition(DefinitionId {
                            module: id,
                            statement: index,
                        }),
                        statement,
                    )?;
                }
                _ => {
                    if let Some(names) = conditional_imports(&self.modules, id, statement) {
                        for (name, binding) in names {
                            // An assignment to an established module value keeps its slot.
                            // scan_value_writes records its rebinding and checks cyclic reads.
                            if self.modules[id].local_value(id, binding).is_none() {
                                self.bind(id, name, Binding::ConditionalValue, statement)?;
                            }
                        }
                        self.needs_object_imports = true;
                    }
                    if let ast::Stmt::If(guard) = statement
                        && resolve_binding(&self.modules, id, &guard.test)
                            == Some(Binding::Intrinsic(Intrinsic::TypeChecking))
                        && let [otherwise] = guard.elif_else_clauses.as_slice()
                        && otherwise.test.is_none()
                    {
                        for runtime_statement in &otherwise.body {
                            if let ast::Stmt::ClassDef(class) = runtime_statement {
                                runtime_classes.push(class.name.as_str());
                            } else if let Some(name) = value_name(runtime_statement)
                                && module_value(runtime_statement).is_some()
                            {
                                self.bind(id, name, Binding::ConditionalValue, runtime_statement)?;
                            }
                        }
                    }
                    if let ast::Stmt::If(guard) = statement
                        && guard
                            .elif_else_clauses
                            .last()
                            .is_some_and(|clause| clause.test.is_none())
                    {
                        for statement in &guard.body {
                            let Some(name) = conditional_export_name(statement) else {
                                continue;
                            };
                            // Assignments to existing globals retain their storage and write tracking.
                            if module_value(statement).is_some()
                                && self.modules[id].bindings.contains_key(name)
                            {
                                continue;
                            }
                            if guard.elif_else_clauses.iter().all(|clause| {
                                clause
                                    .body
                                    .iter()
                                    .any(|other| conditional_export_name(other) == Some(name))
                            }) {
                                self.bind(id, name, Binding::ConditionalValue, statement)?;
                                self.needs_object_imports = true;
                            }
                        }
                    }
                    let entry_global = is_entry
                        && !executable
                        && (last_definition.is_some_and(|last| index < last)
                            || matches!(
                                module_value(statement),
                                Some(ast::Expr::Call(call)) if resolve_binding(&self.modules, id, &call.func) == Some(Binding::Intrinsic(Intrinsic::RegexCompile))
                            ));
                    if entry_global
                        && let ast::Stmt::If(guard) = statement
                        && guard.elif_else_clauses.is_empty()
                        && let [ast::Stmt::ClassDef(class)] = guard.body.as_slice()
                    {
                        self.bind(
                            id,
                            class.name.as_str(),
                            Binding::ConditionalValue,
                            statement,
                        )?;
                        self.needs_object_imports = true;
                        self.modules[id].entry_globals.push(index);
                        continue;
                    }
                    self.scan_value_writes(id, statement)?;
                    if let Some(name) = value_name(statement)
                        && (!is_entry || entry_global)
                        && self.modules[id].rebound_target(id, statement).is_none()
                    {
                        self.bind(
                            id,
                            name,
                            Binding::Value(DefinitionId {
                                module: id,
                                statement: index,
                            }),
                            statement,
                        )?;
                        if global_writes.contains(name) {
                            self.modules[id].rebound_values.insert(index);
                        }
                    }
                    if entry_global {
                        self.modules[id].entry_globals.push(index);
                        continue;
                    }
                    executable = true;
                    self.modules[id].executable.push(index);
                }
            }
        }
        if is_entry && let Some(last) = last_definition {
            let module = &mut self.modules[id];
            // Optional imports and type aliases keep their import-scan rules. Execute
            // their prefix in the same initializer as the intervening declarations.
            let prefix = module.executable.partition_point(|&index| index < last);
            if prefix != 0 {
                module
                    .entry_globals
                    .extend(module.executable.drain(..prefix));
                module.entry_globals.sort_unstable();
            }
        }
        // Expose otherwise unbound classes after module initialization. Preserve the
        // existing resolution of names assigned or imported elsewhere in the module.
        for name in runtime_classes {
            self.modules[id]
                .bindings
                .entry(name.to_owned())
                .or_insert(Binding::RuntimeClass);
        }
        if !field_separators.is_empty() {
            let module = &mut self.modules[id];
            let mut source = module.source.clone();
            for range in field_separators {
                let spaces = " ".repeat(range.len());
                source.replace_range(range, &spaces);
            }
            module.parsed = self.session.replace_source(id, source)?;
        }
        self.loading.remove(&id);
        self.initialization
            .push((id, segment..parsed.syntax().body.len()));
        Ok(())
    }

    /// Record writes to established values, including those inside module-level control flow.
    fn scan_value_writes(&mut self, id: ModuleId, statement: &ast::Stmt) -> Result<(), String> {
        use ast::statement_visitor::{StatementVisitor, walk_stmt};

        #[derive(Default)]
        struct Writes<'a>(Vec<&'a ast::Stmt>);
        impl<'a> StatementVisitor<'a> for Writes<'a> {
            fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
                if value_name(stmt).is_some() {
                    self.0.push(stmt);
                } else if !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
                    walk_stmt(self, stmt);
                }
            }
        }
        let mut writes = Writes::default();
        writes.visit_stmt(statement);
        let module = &mut self.modules[id];
        for stmt in writes.0 {
            if let Some(first) = module.local_value(id, stmt) {
                if self.partial_value_imports.contains(&first) {
                    return Err(module.error(stmt,
                        "rebinding a value already imported during cyclic initialization requires a runtime namespace"));
                }
                module.rebound_values.insert(first.statement);
            }
        }
        Ok(())
    }

    /// Preserve the original spelling of target-dependent symbols even when imported under an alias.
    fn named_import(&mut self, binding: Binding, name: &str) -> Binding {
        if binding != Binding::Intrinsic(Intrinsic::ErrnoAttribute) {
            return binding;
        }
        let slot = self
            .errno_names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| {
                self.errno_names.push(name.to_owned());
                self.errno_names.len() - 1
            });
        Binding::Intrinsic(Intrinsic::ErrnoFrom(slot))
    }

    fn bind(
        &mut self,
        module: ModuleId,
        name: &str,
        binding: Binding,
        node: &impl Ranged,
    ) -> Result<(), String> {
        // Resolve names for the checked graph, but never erase a provider's initializer.
        // An independently selected object entry will execute the normal Python import.
        if let Binding::IntrinsicModule(IntrinsicModule::RuntimeSource(owner))
        | Binding::Intrinsic(Intrinsic::RuntimeImport(owner)) = binding
        {
            self.runtime_initializer.get_or_insert_with(|| {
                self.modules[module].error(
                    node,
                    &format!("{owner} requires runtime import initialization"),
                )
            });
        }
        if self.modules[module].bindings.get(name) == Some(&Binding::ConditionalValue) {
            self.modules[module].bindings.insert(name.into(), binding);
            return Ok(());
        }
        if let Some(previous) = self.modules[module].bindings.get(name) {
            if *previous != binding {
                return Err(self.modules[module]
                    .error(node, &format!("static binding {name:?} cannot be rebound")));
            }
        } else {
            self.modules[module].bindings.insert(name.into(), binding);
        }
        Ok(())
    }

    /// A dotted alias follows visible package attributes after loading the physical leaf.
    fn aliased_import(&self, name: &str) -> Result<Binding, String> {
        let mut parts = name.split('.');
        let root = parts.next().unwrap();
        let mut binding = Binding::Module(self.by_name[root]);
        for part in parts {
            binding = match binding {
                Binding::Module(parent) => self.modules[parent].export(part).or_else(|| {
                    // A loaded child may not be published on its parent yet.
                    let child = format!("{}.{part}", self.modules[parent].name);
                    self.by_name.get(&child).copied().map(Binding::Module)
                }),
                Binding::IntrinsicModule(module) => stdlib_symbol(&self.modules, module, part).ok(),
                _ => None,
            }
            .ok_or_else(|| format!("cannot resolve {part:?} in dotted import {name:?}"))?;
        }
        Ok(binding)
    }

    /// A resolved import still needs its initializer when the module was prepared earlier.
    fn import_namespace(&mut self, importer: ModuleId, origin: TextRange, target: ModuleId) {
        let module = &self.modules[target];
        if module.registered_namespace {
            self.runtime_initializer.get_or_insert_with(|| {
                self.modules[importer].error(
                    &origin,
                    &format!("{} requires runtime import initialization", module.name),
                )
            });
        }
    }

    fn package_child(
        &mut self,
        parent: ModuleId,
        member: &str,
        source_exports: bool,
    ) -> Result<Binding, String> {
        let name = format!("{}.{member}", self.modules[parent].name);
        if let Some(&child) = self.by_name.get(&name)
            && self.loading.contains(&child)
            && self.partial_source(child)
            && !self.modules[parent].bindings.contains_key("__getattr__")
        {
            // A fromlist can find the loading child before its parent publishes it.
            self.ordered_imports = true;
            return Ok(Binding::Module(child));
        }
        self.load_source(&name, source_exports).map(Binding::Module)
    }

    fn import(&mut self, id: ModuleId, index: usize, statement: &ast::Stmt) -> Result<(), String> {
        match statement {
            ast::Stmt::Import(import) => {
                if import.is_lazy {
                    return Err("lazy imports are unsupported".into());
                }
                for alias in &import.names {
                    let name = alias.name.as_str();
                    if let Some(module) = self.intrinsic_module(name, None)? {
                        if let Some(child) = importlib_child_name(module) {
                            self.modules[id]
                                .importlib_children
                                .entry(child)
                                .or_insert(statement.end());
                        }
                        if module.lacks_builtin_stub() {
                            self.checked_builtin_imports.push((id, alias.range()));
                        }
                        if alias.asname.is_none()
                            && let Some((local, _)) = name.split_once('.')
                            && let Some(namespace) = IntrinsicModule::named(local)
                        {
                            self.bind(id, local, Binding::IntrinsicModule(namespace), alias)?;
                            continue;
                        }
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        self.bind(id, local.as_str(), Binding::IntrinsicModule(module), alias)?;
                        continue;
                    }
                    self.load_module(name)?;
                    let (local, binding) = if let Some(local) = &alias.asname {
                        (local.as_str(), self.aliased_import(name)?)
                    } else {
                        let root = name.split('.').next().unwrap();
                        (root, Binding::Module(self.by_name[root]))
                    };
                    self.bind(id, local, binding, alias)?;
                }
            }
            ast::Stmt::ImportFrom(import) => {
                if import.is_lazy {
                    return Err("lazy imports are unsupported".into());
                }
                let name = self.import_name(id, import)?;
                if import.names.iter().any(|alias| alias.name.as_str() == "*") {
                    // The module initializer chooses its exports. Still resolve the
                    // dependency so checked files retain their ordinary diagnostics.
                    let module = if import.level == 0 {
                        self.intrinsic_module(&name, None)?
                    } else {
                        None
                    };
                    if module.is_none() {
                        self.load_module(&name)?;
                    }
                    if let Some(IntrinsicModule::RuntimeSource(owner)) = module {
                        self.runtime_initializer.get_or_insert_with(|| {
                            self.modules[id].error(
                                import,
                                &format!("{owner} requires runtime import initialization"),
                            )
                        });
                        return Ok(());
                    }
                    self.needs_object_imports = true;
                    self.wildcard_import.get_or_insert_with(|| {
                        self.modules[id].error(import, "wildcard imports require runtime binding")
                    });
                    return Ok(());
                }
                if import.level == 0
                    && name == "imre._calendar"
                    && let [alias] = import.names.as_slice()
                    && alias.name.as_str() == "month_grid"
                {
                    if !self.modules[id].is_bundled(CALENDAR_CLASSES) {
                        return Err("the private calendar grid builder is not importable".into());
                    }
                    if let Some(module) = &import.module {
                        self.checked_builtin_imports.push((id, module.range()));
                    }
                    self.checked_builtin_imports.push((id, alias.range()));
                    let local = alias.asname.as_ref().unwrap_or(&alias.name);
                    return self.bind(
                        id,
                        local.as_str(),
                        Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                            "imre._calendar.month_grid",
                        )),
                        alias,
                    );
                }
                if import.level == 0
                    && name == "imre._zipimport"
                    && let [alias] = import.names.as_slice()
                    && alias.name.as_str() == "open_index"
                {
                    if !self.modules[id].is_bundled("zipimport") {
                        return Err(
                            "the private zipimport directory opener is not importable".into()
                        );
                    }
                    let local = alias.asname.as_ref().unwrap_or(&alias.name);
                    return self.bind(
                        id,
                        local.as_str(),
                        Binding::Intrinsic(Intrinsic::ZipImportIndex),
                        alias,
                    );
                }
                if import.level == 0 && name == "imre._logging" {
                    if !self.modules[id].is_bundled(LOGGING_CLASSES) {
                        return Err("the private logging implementation is not importable".into());
                    }
                    for alias in &import.names {
                        let method = match alias.name.as_str() {
                            "_rename_handler" => Intrinsic::HandlerRename,
                            "_Lock" => Intrinsic::HandlerLock,
                            "_lookup_handler" => Intrinsic::HandlerLookup,
                            "_handler_names" => Intrinsic::HandlerNames,
                            "_file_encoding_uses_utf8" => Intrinsic::FileEncodingUsesUtf8,
                            "_print_active_exception" => Intrinsic::PrintActiveException,
                            "_Level" => Intrinsic::HandlerLevelType,
                            "_TextOutput" => Intrinsic::TextOutputType,
                            "_fallback" => Intrinsic::HandlerFallback,
                            _ => {
                                return Err(format!(
                                    "unknown private logging symbol: {}",
                                    alias.name
                                ));
                            }
                        };
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        self.bind(id, local.as_str(), Binding::Intrinsic(method), alias)?;
                    }
                    return Ok(());
                }
                if import.level == 0
                    && let Some(module) = self.intrinsic_module(&name, Some(&import.names))?
                {
                    if let Some(child) = importlib_child_name(module) {
                        self.modules[id]
                            .importlib_children
                            .entry(child)
                            .or_insert(statement.end());
                    }
                    for alias in &import.names {
                        if module == IntrinsicModule::Logging && alias.name.as_str() == "handlers" {
                            self.intrinsic_module("logging.handlers", None)?;
                        }
                        // A from-import loads the child before reading the package attribute.
                        let binding = if module == IntrinsicModule::Importlib
                            && let Some(child) = importlib_child(alias.name.as_str())
                        {
                            self.intrinsic_module(&format!("importlib.{}", alias.name), None)?;
                            self.modules[id].importlib_children.entry(importlib_child_name(child).unwrap()).or_insert(statement.end());
                            Some(Binding::IntrinsicModule(child))
                        } else if module == IntrinsicModule::Sys && alias.name.as_str() == "pycache_prefix" {
                            return Err("from-importing mutable sys.pycache_prefix is unsupported; use qualified access".into());
                        } else {
                            let member = alias.name.as_str();
                            match stdlib_symbol(&self.modules, module, member) {
                                Ok(binding) => Some(binding),
                                Err(_) if bundled_package(&name)
                                    && bundled_runtime_source(&format!("{name}.{member}")).is_some() =>
                                {
                                    let parent = self.load_module(&name)?;
                                    let binding = match self.modules[parent].export(member) {
                                        Some(binding) => binding,
                                        None => self.package_child(parent, member, true)?,
                                    };
                                    if let Binding::Module(child) = binding {
                                        self.import_namespace(id, statement.range(), child);
                                    }
                                    Some(binding)
                                }
                                Err(_) if sealed_package_child(&name) && bundled_runtime_source(&name).is_some() => {
                                    let provider = self.load_source(&name, true)?;
                                    self.import_namespace(id, statement.range(), provider);
                                    self.modules[provider].export(member)
                                }
                                Err(StdlibMiss::Unclaimed) => {
                                    self.session.demanded_shipped_declaration(&name, member)?
                                }
                                Err(StdlibMiss::Sealed) => None,
                            }
                        }
                        .filter(|binding| {
                            self.reachable_only
                                || !matches!(
                                    binding,
                                    Binding::Intrinsic(Intrinsic::Declaration(_))
                                        | Binding::IntrinsicModule(IntrinsicModule::Declaration(_))
                                )
                        })
                        .ok_or_else(|| {
                            format!(
                                "unsupported sealed standard-library export {name}.{}",
                                alias.name
                            )
                        })?;
                        if let Binding::IntrinsicModule(child) = binding {
                            let child_name = format!("{name}.{}", alias.name);
                            if IntrinsicModule::named(&child_name) == Some(child) {
                                self.session.prepare_shipped_source(&child_name)?;
                            }
                        }
                        if matches!(binding, Binding::RegisteredValue(_)) {
                            self.modules[id]
                                .registered_imports
                                .entry(index)
                                .or_insert(alias.range());
                        }
                        if binding
                            == Binding::Intrinsic(Intrinsic::Fnmatch(FnmatchOperation::Parts))
                            || binding.is_registered_object()
                            || binding == Binding::IntrinsicModule(IntrinsicModule::ReConstants)
                            || module == IntrinsicModule::EmailHeaderValueParser
                            || matches!(
                                (module, binding),
                                (
                                    IntrinsicModule::Socket,
                                    Binding::Intrinsic(Intrinsic::StringConstant(_))
                                )
                            )
                        {
                            self.checked_builtin_imports.push((id, alias.range()));
                        }
                        if binding == Binding::Intrinsic(Intrinsic::BuiltinsLen) {
                            return Err("from builtins import len needs an import-time snapshot; use a qualified lookup".into());
                        }
                        let binding = if let Binding::Intrinsic(
                            symbol @ (Intrinsic::BuiltinsLast | Intrinsic::Stdout),
                        ) = binding
                        {
                            // Typeshed omits the interactive name; this import
                            // performs the real lookup and may raise ImportError.
                            if symbol == Intrinsic::BuiltinsLast {
                                self.checked_builtin_imports.push((id, alias.range()));
                            }
                            self.modules[id]
                                .snapshots
                                .insert(index, ImportSnapshot::Intrinsic(symbol));
                            Binding::Value(DefinitionId {
                                module: id,
                                statement: index,
                            })
                        } else {
                            binding
                        };
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        let binding = self.named_import(binding, alias.name.as_str());
                        // A Sequence annotation keeps its typed list contract; value uses request boxing.
                        self.needs_object_imports |= !matches!(
                            module,
                            IntrinsicModule::EmailHeaderValueParser
                                | IntrinsicModule::Marshal
                                | IntrinsicModule::Pickle
                                | IntrinsicModule::Struct
                                | IntrinsicModule::Warnings
                                | IntrinsicModule::ImportlibReaders
                                | IntrinsicModule::ImportlibResourceReaders
                                | IntrinsicModule::ImportlibResources
                        ) && matches!(
                            binding,
                            Binding::Intrinsic(
                                export @ (Intrinsic::ObjectRuntimeExport(_) | Intrinsic::Awaitable)
                            ) if export != Intrinsic::TYPING_SEQUENCE
                        );
                        if matches!(
                            (module, alias.name.as_str()),
                            (IntrinsicModule::Collections, "_count_elements")
                                | (IntrinsicModule::Struct, "_clearcache")
                        ) {
                            self.checked_builtin_imports.push((id, alias.range()));
                        }
                        self.bind(id, local.as_str(), binding, alias)?;
                    }
                    if module.lacks_builtin_stub()
                        && let Some(name) = &import.module
                    {
                        self.checked_builtin_imports.push((id, name.range()));
                    }
                    return Ok(());
                }
                // Loading a new child briefly publishes the module on this package. A
                // from-import in the package can immediately replace that very binding.
                let fresh_child = name.rsplit_once('.').and_then(|(parent, child)| {
                    (parent == self.modules[id].name && !self.by_name.contains_key(&name))
                        .then_some(child)
                });
                // Only a from-import may read an explicitly ready binding from a loading module.
                let target = match self.by_name.get(&name).copied() {
                    Some(target)
                        if self.loading.contains(&target)
                            && (self.modules[target].is_package || self.partial_source(target)) =>
                    {
                        target
                    }
                    _ => self.load_module(&name)?,
                };
                for alias in &import.names {
                    let binding = if let Some(binding) =
                        self.modules[target].export(alias.name.as_str())
                    {
                        if self.loading.contains(&target) {
                            let package = self.modules[target].is_package;
                            let available = match binding {
                                Binding::Definition(def) => {
                                    let statement = &self.modules[def.module].parsed.syntax().body
                                        [def.statement];
                                    let ready = def.module == target
                                        && self.partial_source(target)
                                        && self.initialized(def)
                                        && matches!(statement, ast::Stmt::FunctionDef(f) if f.decorator_list.is_empty());
                                    self.ordered_imports |= ready;
                                    ready
                                }
                                Binding::Value(def) | Binding::ImportedValue(def, _) => {
                                    package && self.initialized(def)
                                }
                                Binding::TypeAlias(owner, index)
                                    if let TypeAlias::Declaration(statement) =
                                        self.modules[owner].aliases[index] =>
                                {
                                    package
                                        && self.initialized(DefinitionId {
                                            module: owner,
                                            statement,
                                        })
                                }
                                Binding::ConditionalValue | Binding::RuntimeClass => false,
                                _ => package,
                            };
                            if !available {
                                return Err(format!("cyclic import through {name:?}"));
                            }
                        }
                        if let Binding::Module(child) = binding
                            && self.loading.contains(&child)
                        {
                            return Err(format!(
                                "cyclic import through {:?}",
                                self.modules[child].name
                            ));
                        }
                        binding
                    } else if self.modules[target].is_bundled("zoneinfo")
                        || self.modules[target].is_bundled("pyexpat")
                        || self.modules[target].is_bundled("xml.parsers.expat")
                        || self.modules[target].is_bundled("unittest.signals")
                    {
                        Binding::ConditionalValue
                    } else if self.modules[target].is_package {
                        self.package_child(target, alias.name.as_str(), false)?
                    } else if self.modules[target].is_bundled("http.client")
                        || self.modules[target].is_bundled("dis")
                        || self.modules[target].is_bundled("xml.etree.ElementTree")
                        || self.modules[target].is_bundled("xml.etree.ElementPath")
                    {
                        // These source providers resolve additional exports at runtime.
                        Binding::ConditionalValue
                    } else if self.modules[target].bundled
                        && !self.loading.contains(&target)
                        && let Some(binding @ Binding::RegisteredClass(_)) = self
                            .session
                            .demanded_shipped_declaration(&name, alias.name.as_str())?
                    {
                        binding
                    } else {
                        return Err(format!(
                            "module {name:?} has no static binding {:?}",
                            alias.name.as_str()
                        ));
                    };
                    if let Binding::Value(def) = binding
                        && self.loading.contains(&def.module)
                    {
                        self.partial_value_imports.insert(def);
                    }
                    let binding = if let Binding::Value(def) = binding
                        && self.modules[def.module]
                            .rebound_values
                            .contains(&def.statement)
                    {
                        let ImportSnapshot::Values(values) = self.modules[id]
                            .snapshots
                            .entry(index)
                            .or_insert_with(|| ImportSnapshot::Values(Vec::new()))
                        else {
                            unreachable!("a from-import has a single source module")
                        };
                        let member = values.len();
                        values.push(def);
                        Binding::ImportedValue(
                            DefinitionId {
                                module: id,
                                statement: index,
                            },
                            member,
                        )
                    } else {
                        binding
                    };
                    if binding == Binding::Intrinsic(Intrinsic::SOCKETSERVER_CLASS) {
                        self.checked_builtin_imports.push((id, alias.range()));
                    }
                    self.needs_object_imports |= binding == Binding::ConditionalValue;
                    self.imports_runtime_class |= binding == Binding::RuntimeClass;
                    let local = alias.asname.as_ref().unwrap_or(&alias.name);
                    if fresh_child == Some(local.as_str())
                        && let Some(current) = self.modules[id].bindings.get_mut(local.as_str())
                        && *current == Binding::Module(target)
                    {
                        *current = binding;
                    } else {
                        self.bind(id, local.as_str(), binding, alias)?;
                    }
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Intrinsic imports use fixed identities, but never silently accept a
    /// first-party file that attempts to shadow the standard-library namespace.
    fn intrinsic_module(
        &mut self,
        name: &str,
        imports: Option<&[ast::Alias]>,
    ) -> Result<Option<IntrinsicModule>, String> {
        if self.project_shadows_native_package(name) {
            return Ok(None);
        }
        if self.allow_imports
            && imre_ast::providers::is_native_fallback(name)
            && self.has_project_source(name)
        {
            return Ok(None);
        }
        let module = IntrinsicModule::named(name).filter(|module| {
            self.reachable_only || !matches!(module, IntrinsicModule::Declaration(_))
        });
        if let Some(module) = module {
            self.reject_stdlib_shadow(name)?;
            if imports.is_none() {
                self.session.prepare_shipped_source(name)?;
            }
            if module == IntrinsicModule::Fractions {
                // The checked representation has the same import shadowing rules as fractions.py.
                for dependency in ["functools", "math", "numbers", "operator", "re", "sys"] {
                    self.reject_stdlib_shadow(dependency)?;
                }
            }
            self.needs_object_imports |= matches!(
                module,
                IntrinsicModule::Calendar
                    | IntrinsicModule::Fractions
                    | IntrinsicModule::Ast
                    | IntrinsicModule::NtPath
                    | IntrinsicModule::Blake2
                    | IntrinsicModule::OperatorNative
                    | IntrinsicModule::Sha3
                    | IntrinsicModule::Md5
                    | IntrinsicModule::Atexit
                    | IntrinsicModule::Imp
                    | IntrinsicModule::ImportlibLoaderAbc
                    | IntrinsicModule::ImportlibBootstrapExternal
                    | IntrinsicModule::ImportlibResourcesAbc
            ) || matches!(
                module,
                IntrinsicModule::SocketNative | IntrinsicModule::ImportlibAbc
            ) && imports.is_none_or(|names| {
                names.iter().any(|alias| {
                    !matches!(
                        module.symbol(alias.name.as_str()),
                        Some(Binding::Intrinsic(
                            Intrinsic::IntConstant(_)
                                | Intrinsic::SocketControlSize { .. }
                                | Intrinsic::ScopedObjectExport(_)
                        ))
                    )
                })
            });
        }
        if let Some(owner @ (IntrinsicModule::Dataclasses | IntrinsicModule::Inspect)) = module
            && imports.is_none_or(|names| {
                names
                    .iter()
                    .any(|a| checked_stdlib_owner(owner, a.name.as_str()).is_some())
            })
        {
            self.load_module(name)?;
        }
        if module == Some(IntrinsicModule::Selectors)
            && cfg!(any(target_os = "linux", target_os = "macos"))
        {
            self.load_module(name)?;
        }
        let needs_textwrap = |implementation| {
            matches!(module, Some(IntrinsicModule::Textwrap))
                && imports.is_none_or(|names| {
                    names.iter().any(|alias| {
                        textwrap_implementation(alias.name.as_str()) == Some(implementation)
                    })
                })
        };
        // A named import loads only the bundled definitions it can call.
        let bundles = [
            (module == Some(IntrinsicModule::ImportlibMetadata)
                && imports.is_none_or(|names| names.iter().any(|a| a.name.as_str() == "version")))
            .then_some((
                METADATA_VERSION,
                concat!(
                    include_str!("../stdlib/importlib/metadata_typed.py"),
                    "\n",
                    include_str!("../stdlib/importlib/metadata_version.py"),
                ),
            )),
            (module == Some(IntrinsicModule::Fractions)
                && imports.is_none_or(|names| names.iter().any(|a| a.name.as_str() == "Fraction")))
            .then_some((FRACTION_CLASS, include_str!("../stdlib/fraction_pair.py"))),
            (module == Some(IntrinsicModule::Calendar)
                && imports.is_none_or(|names| {
                    names
                        .iter()
                        .any(|a| matches!(a.name.as_str(), "Calendar" | "TextCalendar"))
                }))
            .then_some((
                CALENDAR_CLASSES,
                include_str!("../stdlib/calendar_grid_typed.py"),
            )),
            (module == Some(IntrinsicModule::Contextlib)
                && imports.is_none_or(|names| {
                    names
                        .iter()
                        .any(|a| a.name.as_str() == "AbstractContextManager")
                }))
            .then_some((CONTEXTLIB_ABSTRACT, include_str!("../stdlib/contextlib.py"))),
            (module == Some(IntrinsicModule::Contextlib)
                && imports
                    .is_none_or(|names| names.iter().any(|a| a.name.as_str() == "ExitStack")))
            .then_some((
                CONTEXTLIB_EXIT_STACK,
                concat!(
                    // The typed layout uses one inheritance chain for the two stack bases.
                    "from contextlib import AbstractContextManager\n",
                    "class _BaseExitStack(AbstractContextManager):\n",
                    include_str!("../stdlib/contextlib_exitstack_base.inc"),
                    "\n",
                    include_str!("../stdlib/contextlib_exitstack.py")
                ),
            )),
            (module == Some(IntrinsicModule::Tempfile)
                && imports.is_none_or(|names| {
                    names
                        .iter()
                        .any(|a| a.name.as_str() == "SpooledTemporaryFile")
                }))
            .then_some((TEMPFILE_SPOOL, include_str!("../stdlib/tempfile/spool.py"))),
            (name.split('.').next() == Some("logging") && module.is_some()).then_some((
                LOGGING_CLASSES,
                concat!(
                    include_str!("../stdlib/logging/typed_handler.py"),
                    "\n",
                    include_str!("../stdlib/logging/stream.py.inc"),
                    "\n",
                    include_str!("../stdlib/logging/typed_stream.py.inc"),
                    "\n",
                    include_str!("../stdlib/logging/classes.py")
                ),
            )),
            (module == Some(IntrinsicModule::LoggingHandlers)).then_some((
                LOGGING_HANDLER_CLASSES,
                concat!(
                    include_str!("../stdlib/logging/handlers.py"),
                    "\n",
                    include_str!("../stdlib/logging/buffering.py"),
                ),
            )),
            needs_textwrap(TEXTWRAP_FUNCTIONS).then_some((
                TEXTWRAP_FUNCTIONS,
                include_str!("../stdlib/textwrap/functions.py"),
            )),
            needs_textwrap(TEXTWRAP_WRAPPER).then_some((
                TEXTWRAP_WRAPPER,
                concat!(
                    include_str!("../stdlib/textwrap/wrapper.py"),
                    "\n\n",
                    include_str!("../stdlib/textwrap/functions_typed.py"),
                ),
            )),
        ];
        for (implementation, source) in bundles.into_iter().flatten() {
            if self.by_name.contains_key(implementation) {
                continue;
            }
            let path = self.private_implementation_path(implementation)?;
            let id = self.add(path.clone(), path, implementation.into(), false, source)?;
            self.modules[id].bundled = true;
            self.scan(id)?;
        }
        Ok(module)
    }

    fn private_implementation_path(&mut self, implementation: &str) -> Result<PathBuf, String> {
        let path = self.root.join(format!("{implementation}.py"));
        if self.fs.exists(&path) {
            return Err(format!(
                "standard-library shadowing is unsupported: {implementation:?}"
            ));
        }
        self.inputs.missing.push(path.clone());
        Ok(path)
    }

    /// Whether a physical application package or module owns this top-level import.
    fn project_shadows_native_package(&mut self, name: &str) -> bool {
        let root = name.split('.').next().unwrap_or(name);
        self.allow_imports
            && imre_ast::providers::is_native_package(root)
            && !imre_ast::providers::is_frozen(root, || self.frozen_modules)
            && self.has_project_source(root)
    }

    /// Look for a source file on the project search path before using a bundled fallback.
    fn has_project_source(&mut self, name: &str) -> bool {
        let roots = std::iter::once(&self.root)
            .skip(usize::from(self.skip_current_root))
            .chain(&self.search_roots[..self.archive_roots]);
        for root in roots {
            if let Some(archive) = self.zip_sources.get(root) {
                if archive.package(name).is_some() {
                    return true;
                }
                continue;
            }
            let base = root.join(name);
            for path in [base.join("__init__.py"), base.with_extension("py")] {
                if self.fs.is_file(&path) {
                    return true;
                }
                self.inputs.missing.push(path);
            }
        }
        false
    }

    fn reject_stdlib_shadow(&mut self, name: &str) -> Result<(), String> {
        if self.allow_imports
            && !self.skip_current_root
            && !imre_ast::providers::is_frozen(name, || self.frozen_modules)
        {
            for base in [
                self.root.join(name.split('.').next().unwrap()),
                self.root.join(name.replace('.', "/")),
            ] {
                for candidate in [base.with_extension("py"), base.join("__init__.py")] {
                    if self.fs.exists(&candidate) {
                        return Err(format!(
                            "standard-library shadowing is unsupported: {name:?}"
                        ));
                    }
                    self.inputs.missing.push(candidate);
                }
            }
        }
        Ok(())
    }

    fn import_name(&self, id: ModuleId, import: &ast::StmtImportFrom) -> Result<String, String> {
        let suffix = import.module.as_ref().map_or("", |name| name.as_str());
        let module = &self.modules[id];
        let package = import_package(&module.name, module.is_package);
        resolve_import(package, import.level, suffix)
            .ok_or_else(|| "relative import goes beyond the first-party package root".into())
    }

    fn partial_source(&self, id: ModuleId) -> bool {
        let module = &self.modules[id];
        !self.is_entry(id) && !module.bundled && module.archive.is_none()
    }

    fn initialized(&self, def: DefinitionId) -> bool {
        self.initialization.iter().any(|(module, statements)| {
            *module == def.module && statements.contains(&def.statement)
        })
    }

    fn is_entry(&self, id: ModuleId) -> bool {
        let Some(name) = &self.module_operand else {
            return id == 0;
        };
        let module = &self.modules[id];
        !module.is_package
            && (module.name == *name
                || module.name.strip_suffix(".__main__") == Some(name.as_str()))
    }

    fn load_module_entry(&mut self, name: &str) -> Result<Option<ModuleId>, String> {
        let mut id = self.load_module(name)?;
        if self.modules[id].bundled || self.modules[id].archive.is_some() {
            return Ok(None);
        }
        if self.modules[id].is_package {
            if name == "__main__" || name.ends_with(".__main__") {
                return Ok(None);
            }
            id = self.load_module(&format!("{name}.__main__"))?;
        }
        let module = &self.modules[id];
        let project_source = !module.is_package
            && !module.bundled
            && module.archive.is_none()
            && matches!(module.search_roots, SearchRoots::Single(index) if index <= self.archive_roots);
        Ok(project_source.then_some(id))
    }

    fn load_module(&mut self, name: &str) -> Result<ModuleId, String> {
        self.load_source(name, false)
    }

    /// An unresolved sealed from-import may ask the selected bundled file for its exports.
    /// Ordinary module loads keep their checked definitions.
    fn load_source(&mut self, name: &str, source_exports: bool) -> Result<ModuleId, String> {
        if name == FRACTION_CLASS {
            return Err("the private Fraction implementation is not an importable module".into());
        }
        if name == METADATA_VERSION {
            return Err(
                "the private importlib.metadata implementation is not an importable module".into(),
            );
        }
        if name == TEMPFILE_SPOOL {
            return Err("the private tempfile implementation is not an importable module".into());
        }
        if name == INSPECT_FUNCTIONS {
            return Err("the private inspect implementation is not an importable module".into());
        }
        if matches!(name, LOGGING_CLASSES | LOGGING_HANDLER_CLASSES) {
            return Err("the private logging implementation is not an importable module".into());
        }
        if matches!(name, TEXTWRAP_FUNCTIONS | TEXTWRAP_WRAPPER) {
            return Err("the private textwrap implementation is not an importable module".into());
        }
        if let Some(&id) = self.by_name.get(name) {
            if self.loading.contains(&id) {
                return Err(format!(
                    "cyclic import through {name:?} (the entry module cannot import itself)"
                ));
            }
            return Ok(id);
        }
        let mut registered_namespace = false;
        let bundled = match name {
            _ if self.project_shadows_native_package(name) => None,
            _ if !self.frozen_modules
                && imre_ast::providers::is_optional_frozen(name)
                && self.has_project_source(name) =>
            {
                None
            }
            _ if self.allow_imports
                && (matches!(name, "_pyio" | "quopri")
                    || imre_ast::providers::is_native_fallback(name))
                && self.has_project_source(name) =>
            {
                None
            }
            "stat" => stat_mode_source().map(|source| ("stat.py", source)),
            // Keep these checked definitions behind the complete public typeshed interfaces.
            "inspect" => {
                // Preserve the typed function's filename and its reserved-name diagnostic.
                self.private_implementation_path(INSPECT_FUNCTIONS)?;
                Some((
                    "__imre_inspect_functions__.py",
                    self.session.checked_shipped_prefix(name, "cleandoc")?,
                ))
            }
            "dataclasses" => Some((
                "<imre stdlib>/dataclasses.py",
                self.session.checked_shipped_prefix(name, "MISSING")?,
            )),
            "socketserver" => Some(("socketserver.py", include_str!("../stdlib/socketserver.py"))),
            "_imre_socketserver_reader" => Some((
                "_imre_socketserver_reader.py",
                include_str!("../stdlib/_socketserver_reader_typed.py"),
            )),
            "selectors" if cfg!(any(target_os = "linux", target_os = "macos")) => {
                Some(("selectors.py", include_str!("../stdlib/selectors.py")))
            }
            "doctest" => Some((
                "doctest.py",
                concat!(
                    include_str!("../stdlib/doctest.py"),
                    "\n",
                    include_str!("../stdlib/doctest_typed.py"),
                ),
            )),
            // The object runtime owns these exports, including the live unittest result registry.
            "pyexpat" | "xml.parsers.expat" | "unittest.signals" => {
                self.needs_object_imports = true;
                bundled_runtime_source(name).map(|(path, _)| (path, ""))
            }
            "pkgutil" => Some((
                "pkgutil.py",
                concat!(
                    include_str!("../stdlib/pkgutil.py"),
                    "\n",
                    include_str!("../stdlib/pkgutil/iter_modules.py")
                ),
            )),
            "zipimport" if cfg!(unix) => Some((
                "zipimport.py",
                concat!(
                    include_str!("../stdlib/zipimport.py"),
                    "\n",
                    include_str!("../stdlib/zipimport_typed.py"),
                ),
            )),
            "platform" => Some((
                "platform.py",
                concat!(
                    include_str!("../stdlib/platform/version.py"),
                    "\n",
                    include_str!("../stdlib/platform/typed.py"),
                ),
            )),
            "mimetypes" if cfg!(unix) => {
                Some(("mimetypes.py", include_str!("../stdlib/mimetypes.py")))
            }
            "string" => Some((
                "string/__init__.py",
                include_str!("../stdlib/string/__init__.py"),
            )),
            // The Python evaluator resolves ElementTree's optional C accelerator and the
            // globals that ElementPath initializes from its tokenizer.
            "xml.etree.ElementTree" | "xml.etree.ElementPath" => {
                bundled_runtime_source(name).map(|(filename, _)| (filename, ""))
            }
            // ZoneInfo exports belong to the boxed provider. Loading the package's children
            // while scanning this typed namespace would recursively enter its initializer.
            "zoneinfo" => Some(("zoneinfo/__init__.py", "")),
            // The boxed email package is intrinsic. Typed child modules need a package root.
            "email" => Some(("email/__init__.py", "")),
            "email.message" => Some((
                "email/message.py",
                include_str!("../stdlib/email/message.py"),
            )),
            "unittest" => Some((
                "unittest/__init__.py",
                include_str!("../stdlib/unittest/__init__.py"),
            )),
            "http.client" => Some(("http/client.py", include_str!("../stdlib/http/client.py"))),
            // The checked socketserver provides the synchronous server and handler bases.
            "http.server" => Some((
                "http/server.py",
                self.session
                    .checked_shipped_prefix(name, "BaseHTTPRequestHandler")?,
            )),
            _ => {
                // bisect selects its optional accelerator in its module initializer.
                registered_namespace = source_exports || name == "bisect";
                bundled_runtime_source(name)
            }
        };
        if let Some((filename, source)) = bundled {
            self.reject_stdlib_shadow(name)?;
            // token initializes its tables and quopri selects a codec from live module globals.
            self.needs_object_imports |= matches!(
                name,
                "token"
                    | "html"
                    | "wave"
                    | "http.client"
                    | "fractions"
                    | "quopri"
                    | "xml"
                    | "zoneinfo"
                    | "compression._common._streams"
            );
            let parent = self.package_parent(name)?;
            // Initializing the package may already have imported this child.
            if let Some(&id) = self.by_name.get(name) {
                return Ok(id);
            }
            // ty resolves this immutable source at its Python import path in the
            // in-memory project. It is part of the compiler, not a disk input.
            let path = self.root.join(filename);
            let id = self.add(
                path.clone(),
                path,
                name.into(),
                filename.ends_with("/__init__.py"),
                source,
            )?;
            self.modules[id].bundled = true;
            // codeop sets compiler flags on Python instances.
            self.needs_object_imports |= name == "codeop";
            if registered_namespace {
                let binding = Binding::Intrinsic(Intrinsic::ScopedObjectExport(filename));
                self.modules[id].registered_namespace = true;
                self.loading.remove(&id);
                // Query the checked source namespace. Its dependencies and initialization
                // effects belong to the ordinary registered import at the caller's site.
                self.modules[id].bindings.extend(
                    self.session
                        .bound_module_names(id)
                        .into_iter()
                        .map(|name| (name, binding)),
                );
            } else {
                self.scan(id)?;
            }
            self.bind_child(name, id, parent)?;
            return Ok(id);
        }
        if !self.allow_imports {
            return Err("imports require a source file and a first-party module root".into());
        }
        // Both scanning an import and resolving a dotted package parent can
        // re-enter the loader. Count active calls, including unresolved parents.
        if self.import_depth >= 96 {
            return Err("first-party import nesting exceeds 96 levels".into());
        }
        self.import_depth += 1;
        let result = self.load_new_module(name);
        self.import_depth -= 1;
        result
    }

    /// Resolve the containing package, including one that is importing this child itself.
    fn package_parent<'n>(&mut self, name: &'n str) -> Result<Option<(ModuleId, &'n str)>, String> {
        let Some((parent, child)) = name.rsplit_once('.') else {
            return Ok(None);
        };
        let id = match self.by_name.get(parent).copied() {
            Some(id) if self.loading.contains(&id) && self.modules[id].is_package => id,
            _ => self.load_module(parent)?,
        };
        if !self.modules[id].is_package {
            return Err(format!("{parent:?} is not a package"));
        }
        Ok(Some((id, child)))
    }

    /// A typeshed declaration alone does not supply a runtime provider. Probe only the
    /// installed roots, even if an earlier source will win the actual import.
    fn has_installed_source(&mut self, name: &str) -> bool {
        let Some((_, probes)) = &mut self.site_probes else {
            return false;
        };
        self.search_roots[self.archive_roots..].iter().any(|site| {
            let path = site.join(name);
            probes.is_file(&mut self.fs, path.with_extension("py"))
                || probes.is_file(&mut self.fs, path.join("__init__.py"))
        })
    }

    /// A declaration supplied by ty can describe a third-party module whose executable
    /// is on a checked search root. Other sealed names keep their normal precedence.
    fn checked_import_declaration(&mut self, first: &str) -> Result<Option<String>, String> {
        let declaration = if INTRINSIC_MODULES.contains(&first) {
            None
        } else {
            let Some(file) = vendored_stdlib_declaration(first) else {
                return Ok(None);
            };
            Some(file)
        };
        if self.project_shadows_native_package(first)
            || (first == "quopri"
                || imre_ast::providers::is_native_fallback(first)
                || (!self.frozen_modules && imre_ast::providers::is_optional_frozen(first)))
                && self.has_project_source(first)
        {
            return Ok(None);
        }
        if IntrinsicModule::named(first).is_none() && imre_ast::providers::provider(first).is_none()
        {
            if self.has_installed_source(first) {
                return Ok(None);
            }
            if !ruff_python_stdlib::sys::is_known_standard_library(
                crate::version::LANGUAGE.minor,
                first,
            ) && declaration.is_some()
            {
                return Ok(declaration);
            }
        }
        if self.module_operand.is_some() {
            self.import_failure = Some(EntryImportFailure::RuntimeProvider);
        }
        Err(format!(
            "standard-library imports and shadowing are unsupported: {first:?}"
        ))
    }

    fn add_declaration_module(&mut self, name: &str) -> Result<(), String> {
        let index = if let Some((package, _)) = name.rsplit_once('.') {
            // A loaded physical package owns its child's search path.
            let Some(&parent) = self.by_name.get(package) else {
                return Ok(());
            };
            let module = &self.modules[parent];
            let SearchRoots::Single(index) = module.search_roots else {
                return Ok(());
            };
            if !module.is_package || module.bundled || module.archive.is_some() {
                return Ok(());
            }
            index
        } else {
            // Shipped declarations already have their own finder and shadowing rules.
            if ruff_python_stdlib::sys::is_known_standard_library(
                crate::version::LANGUAGE.minor,
                name,
            ) {
                return Ok(());
            }
            0
        };
        let root = Self::root_at(&self.root, &self.search_roots, index);
        if self.zip_sources.contains_key(root) || (index == 0 && self.skip_current_root) {
            return Ok(());
        }
        let base = root.join(name.replace('.', "/"));
        // A competing stub owns type lookup. Keep it as a physical input even
        // when the checker cannot use its declaration for a fixed result.
        let stub = [base.join("__init__.pyi"), base.with_extension("pyi")]
            .into_iter()
            .find(|path| {
                let present = self.fs.is_file(path);
                self.inputs.file_checks.insert(path.clone(), present);
                present
            });
        let (path, identity) = if let Some(path) = stub {
            let identity = self.fs.canonicalize(&path).unwrap_or_else(|_| path.clone());
            (path, identity)
        } else if let Ok(Some((path, identity, _, _, None))) = self.module_file(name, index, &base)
        {
            (path, identity)
        } else {
            return Ok(());
        };
        let root = Self::root_at(&self.root, &self.search_roots, index);
        let bytes = if identity.starts_with(root) {
            self.fs.read_source(&identity).ok()
        } else {
            None
        };
        let digest = bytes.as_deref().map_or_else(String::new, |bytes| {
            blake3::hash(bytes).to_hex().to_string()
        });
        if path.extension().is_some_and(|ext| ext == "py")
            && let Some(bytes) = bytes
            && let Ok((source, _)) = imre_ast::source_encoding::decode_disk_source(bytes, &path)
            && let Ok(source) = crate::normalize_source(&source, crate::SourceKind::DiskFile)
        {
            self.session.add_declaration_source(&path, &source)?;
        }
        self.inputs.declaration_files.push((path, identity, digest));
        Ok(())
    }

    fn root_at<'r>(entry: &'r PathBuf, search: &'r [PathBuf], index: usize) -> &'r PathBuf {
        if index == 0 {
            entry
        } else {
            &search[index - 1]
        }
    }

    /// Choose a package, native archive, or Python source in one physical search root.
    fn module_file(
        &mut self,
        name: &str,
        root_index: usize,
        base: &Path,
    ) -> Result<Option<ModuleFile>, String> {
        let root = Self::root_at(&self.root, &self.search_roots, root_index);
        let package = base.join("__init__.py");
        let source = base.with_extension("py");
        let stub = base.with_extension("pyi");
        let (path, is_package, archive) = if self.fs.is_file(&package) {
            (package, true, None)
        } else {
            self.inputs.missing.push(package);
            let installed = self.search_roots[..self.archive_roots].contains(root);
            let mut archives = imre_ast::NATIVE_ARCHIVE_EXTENSIONS
                .iter()
                .map(|ext| base.with_extension(ext));
            let archive = if installed {
                archives.find(|path| {
                    let exists = self.fs.is_file(path);
                    if !exists {
                        self.inputs.missing.push(path.clone());
                    }
                    exists
                })
            } else {
                self.site_probes
                    .as_mut()
                    .filter(|(sites, _)| sites.contains(root))
                    .and_then(|(_, probes)| {
                        archives.find(|path| probes.paired(&mut self.fs, path, &stub))
                    })
            };
            if let Some(archive) = archive {
                if installed && !self.fs.is_file(&stub) {
                    return Err(format!(
                        "native module {name:?} requires a typed declaration at {}",
                        stub.display()
                    ));
                }
                let identity = self.fs.canonicalize(&archive).map_err(|e| e.to_string())?;
                if !identity.starts_with(root) {
                    return Err(format!(
                        "archive for native module {name:?} resolves outside its extension path"
                    ));
                }
                (
                    stub,
                    false,
                    Some(NativeArchive {
                        path: archive,
                        identity,
                    }),
                )
            } else if self.fs.is_file(&source) {
                (source, false, None)
            } else if installed && self.fs.is_file(&stub) {
                return Err(format!(
                    "native module {name:?} requires a rebuilt archive at {}",
                    base.with_extension(crate::NATIVE_ARCHIVE_EXTENSION)
                        .display()
                ));
            } else {
                self.inputs.missing.push(source);
                if installed {
                    self.inputs.missing.push(stub);
                }
                return Ok(None);
            }
        };
        let identity = self
            .fs
            .canonicalize(&path)
            .map_err(|error| error.to_string())?;
        if !identity.starts_with(root) {
            self.inputs
                .declared_file_resolutions
                .push((path, Some(identity)));
            return Err(format!(
                "module {name:?} resolves outside the first-party module root or extension path"
            ));
        }
        Ok(Some((path, identity, is_package, root_index, archive)))
    }

    fn load_new_module(&mut self, name: &str) -> Result<ModuleId, String> {
        let first = name.split('.').next().unwrap_or("");
        let directory_main = name == "__main__" && self.module_operand.as_deref() == Some(name);
        let declaration = if directory_main {
            // runpy removes the startup __main__ from sys.modules for a path operand.
            None
        } else {
            self.checked_import_declaration(first)?
        };
        let parent = self.package_parent(name)?;
        // Parent initialization can have loaded this child while exposing re-exports.
        if let Some(&id) = self.by_name.get(name) {
            if self.loading.contains(&id) {
                return Err(format!("cyclic import through {name:?}"));
            }
            return Ok(id);
        }
        // A regular package owns its subtree. A namespace searches its portions in path order.
        let relative = name.replace('.', "/");
        let mut resolved = None;
        let mut portions = Vec::new();
        let mut zip_import = false;
        let mut member_source = None;
        for root_index in 0..=self.search_roots.len() {
            if root_index == 0 && self.skip_current_root {
                continue;
            }
            if parent.is_some_and(|(id, _)| !self.modules[id].search_roots.contains(root_index)) {
                continue;
            }
            let root = Self::root_at(&self.root, &self.search_roots, root_index);
            if let Some(archive) = self.zip_sources.get_mut(root) {
                // The path-entry finder supplies the physical member used as __main__
                // and the source of imports from this same first-party archive.
                if root_index == 0
                    && self.module_operand.is_some()
                    && let Some(member) = archive.module(name)
                {
                    member_source = Some(member.source);
                    resolved = Some((
                        member.path.clone(),
                        member.path,
                        member.package,
                        root_index,
                        None,
                    ));
                    break;
                }
                if archive.package(name).is_some() {
                    zip_import = true;
                    break;
                }
                zip_import |= archive.contains_namespace(name);
                continue;
            }
            let base = root.join(&relative);
            resolved = self.module_file(name, root_index, &base)?;
            if resolved.is_some() {
                break;
            }
            if self.fs.is_dir(&base) {
                portions.push((
                    root_index,
                    Self::root_at(&self.root, &self.search_roots, root_index).clone(),
                    base,
                ));
            } else {
                self.inputs.missing.push(base);
            }
        }
        if let Some(declaration) = declaration {
            if resolved.is_some() || (!zip_import && !portions.is_empty()) {
                self.has_checked_only_import = true;
            } else if let Some(file) = if name == first {
                Some(declaration)
            } else {
                vendored_stdlib_declaration(&relative)
            } {
                return Err(format!(
                    "no checked runtime provider selected for {name:?} (vendored type declaration {file:?})"
                ));
            }
        }
        let Some((path, identity, is_package, root_index, archive)) = resolved else {
            if zip_import {
                self.import_failure = Some(EntryImportFailure::ZipImport);
                return Err(format!("module {name:?} requires the ZIP importer"));
            }
            if portions.is_empty() {
                self.import_failure = Some(EntryImportFailure::MissingModule);
                let hint = if name == first {
                    "\n  help: for an installed dependency, pass --extension-path DIRECTORY where DIRECTORY contains the package or module"
                } else {
                    ""
                };
                return Err(format!(
                    "cannot resolve first-party module {name:?} under {}{hint}",
                    self.root.display()
                ));
            }
            let path = portions[0].2.clone();
            let mut indices = Vec::with_capacity(portions.len());
            for (index, root, dir) in portions {
                let identity = self.fs.canonicalize(&dir).map_err(|e| e.to_string())?;
                if !identity.starts_with(&root) {
                    return Err(format!(
                        "namespace {name:?} resolves outside its module root"
                    ));
                }
                self.inputs.directories.push((dir, identity));
                indices.push(index);
            }
            // Give the semantic database an empty module without creating an initializer
            // that would hide portions from later roots.
            let id = self.add(path.join("<namespace>.py"), path, name.into(), true, "")?;
            self.modules[id].search_roots = SearchRoots::Namespace(indices);
            self.scan(id)?;
            self.bind_child(name, id, parent)?;
            return Ok(id);
        };
        if archive.is_some() {
            let leaf = name.rsplit('.').next().unwrap();
            if !leaf.is_ascii() {
                return Err(format!(
                    "native module {name:?} requires an ASCII initialization symbol"
                ));
            }
            if self.modules.iter().any(|module| {
                module.archive.is_some() && module.name.rsplit('.').next() == Some(leaf)
            }) {
                return Err(format!(
                    "native modules share the initialization symbol PyInit_{leaf}"
                ));
            }
        }
        if let Some(&previous) = self.by_path.get(&identity) {
            return Err(format!(
                "{} is already loaded as {:?}; a source file cannot have multiple module identities",
                path.display(),
                self.modules[previous].name
            ));
        }
        let input = if let Some(source) = member_source {
            ModuleText::Text(source.into(), crate::SourceKind::DecodedFile)
        } else {
            let bytes = self
                .fs
                .read_source(&identity)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            ModuleText::disk(&path, bytes)?
        };
        // ty resolves logical import paths; canonical identities enforce containment
        // and uniqueness without breaking a single internal symlink's import name.
        let id = self.add_source(path, identity, name.to_owned(), is_package, input)?;
        self.modules[id].search_roots = SearchRoots::Single(root_index);
        self.modules[id].archive = archive;
        self.scan(id)?;
        self.bind_child(name, id, parent)?;
        Ok(id)
    }

    fn bind_child(
        &mut self,
        name: &str,
        id: ModuleId,
        parent: Option<(ModuleId, &str)>,
    ) -> Result<(), String> {
        if let Some((parent, child)) = parent {
            let binding = Binding::Module(id);
            if let Some(previous) = self.modules[parent].bindings.get(child)
                && *previous != binding
            {
                return Err(format!(
                    "importing {name:?} would overwrite a fixed package binding"
                ));
            }
            self.modules[parent]
                .bindings
                .insert(child.to_owned(), binding);
        }
        Ok(())
    }
}

pub(crate) fn errno_import(statement: &ast::Stmt) -> Option<&ast::StmtImportFrom> {
    let ast::Stmt::ImportFrom(import) = statement else {
        return None;
    };
    (import.level == 0
        && import
            .module
            .as_ref()
            .is_some_and(|name| name.as_str() == "errno"))
    .then_some(import)
}

fn stdlib_top_level(name: &str) -> bool {
    INTRINSIC_MODULES.contains(&name) || vendored_stdlib_declaration(name).is_some()
}

fn vendored_stdlib_declaration(name: &str) -> Option<String> {
    let vendored = ty_vendored::file_system();
    [".pyi", "/__init__.pyi"]
        .into_iter()
        .map(|suffix| format!("stdlib/{name}{suffix}"))
        .find(|path| vendored.is_file(path.as_str()))
}

/// Recognize an optional import and any bindings initialized by its probe.
/// The object importer executes the original try, including partial bindings on failure.
fn optional_stdlib_import<'a>(
    modules: &[SourceModule],
    module: ModuleId,
    guard: &'a ast::StmtTry,
) -> Option<(&'a str, Vec<&'a str>)> {
    let (first, rest) = guard.body.split_first()?;
    let module_name = match first {
        ast::Stmt::Import(import) if rest.is_empty() || !import.is_lazy => {
            let [alias] = import.names.as_slice() else {
                return None;
            };
            if alias.name.contains('.') {
                return None;
            }
            alias.name.as_str()
        }
        ast::Stmt::ImportFrom(import) if import.level == 0 && rest.is_empty() => {
            import.module.as_ref()?.as_str()
        }
        _ => return None,
    };
    let primary = single_import_binding(first)?;
    let fallback = import_error_fallback(modules, module, guard, !rest.is_empty())?;
    let top = module_name.split('.').next()?;
    if rest.is_empty() {
        let [assignment] = fallback else { return None };
        return (stdlib_top_level(top)
            && matches!(assignment, ast::Stmt::Assign(_))
            && value_name(assignment) == Some(primary)
            && matches!(module_value(assignment), Some(ast::Expr::NoneLiteral(_))))
        .then_some((module_name, vec![primary]));
    }
    // Optional extension modules can belong to the standard library without a typeshed stub.
    if !stdlib_top_level(top)
        && !ruff_python_stdlib::sys::is_known_standard_library(crate::version::LANGUAGE.minor, top)
    {
        return None;
    }
    let mut names = vec![primary];
    match rest {
        [assignment @ ast::Stmt::Assign(probe)] => {
            let result = value_name(assignment)?;
            if !crate::narrowing::expression_assignments(&probe.value).is_empty() {
                return None;
            }
            let [otherwise] = fallback else { return None };
            if !matches!(otherwise, ast::Stmt::Assign(_))
                || value_name(otherwise) != Some(result)
                || !module_value(otherwise).is_some_and(literal_export)
            {
                return None;
            }
            names.push(result);
        }
        [ast::Stmt::ImportFrom(import)]
            if !import.is_lazy
                && import.level == 0
                && import.module.as_ref()?.as_str() == module_name =>
        {
            for alias in &import.names {
                if alias.name.as_str() == "*" {
                    return None;
                }
                names.push(alias.asname.as_ref().unwrap_or(&alias.name).as_str());
            }
            for stmt in fallback {
                let ast::Stmt::Assign(assignment) = stmt else {
                    return None;
                };
                for target in &assignment.targets {
                    if !names.contains(&target.as_name_expr()?.id.as_str()) {
                        return None;
                    }
                }
                let value = &*assignment.value;
                if !literal_export(value)
                    && !value.is_name_expr()
                    && !matches!(value, ast::Expr::Attribute(attr) if attr.value.is_name_expr())
                {
                    return None;
                }
            }
        }
        _ => return None,
    }
    Some((module_name, names))
}

fn import_error_fallback<'a>(
    modules: &[SourceModule],
    module: ModuleId,
    guard: &'a ast::StmtTry,
    allow_attribute_error: bool,
) -> Option<&'a [ast::Stmt]> {
    let [ast::ExceptHandler::ExceptHandler(handler)] = guard.handlers.as_slice() else {
        return None;
    };
    let exception = handler.type_.as_deref()?;
    let builtin = |expr: &ast::Expr, name: &str| {
        matches!(expr, ast::Expr::Name(n) if n.id.as_str() == name)
            && resolve_binding(modules, module, expr).is_none()
    };
    let catches_import = match exception {
        ast::Expr::Tuple(tuple) if allow_attribute_error && tuple.elts.len() == 2 => {
            ["ImportError", "AttributeError"]
                .iter()
                .all(|name| tuple.elts.iter().any(|expr| builtin(expr, name)))
        }
        _ => builtin(exception, "ImportError"),
    };
    (!guard.is_star
        && guard.orelse.is_empty()
        && guard.finalbody.is_empty()
        && handler.name.is_none()
        && catches_import)
        .then_some(&handler.body)
}

/// Names supplied on both normal exits. The initializer executes the imports
/// and the handler in source order, including after a partially successful import.
fn conditional_imports<'a>(
    modules: &[SourceModule],
    module: ModuleId,
    statement: &'a ast::Stmt,
) -> Option<Vec<(&'a str, &'a ast::Stmt)>> {
    if let ast::Stmt::Try(guard) = statement {
        let fallback = import_error_fallback(modules, module, guard, false)?;
        let imports = guard
            .body
            .iter()
            .map(single_import_binding)
            .collect::<Option<Vec<_>>>()?;
        let mut names = Vec::new();
        for stmt in fallback {
            let name = single_import_binding(stmt).or_else(|| {
                module_value(stmt).filter(|value| literal_export(value))?;
                value_name(stmt)
            })?;
            if imports.contains(&name) {
                names.push((name, stmt));
            }
        }
        return (!names.is_empty()).then_some(names);
    }
    let guard = statement.as_if_stmt()?;
    let test = guard.test.as_compare_expr()?;
    let [left, right] = test.operands.as_ref() else {
        return None;
    };
    if resolve_binding(modules, module, left) != Some(Binding::Intrinsic(Intrinsic::SysPlatform)) {
        return None;
    }
    let strings = |items: &[ast::Expr]| {
        items
            .iter()
            .all(|item| matches!(item, ast::Expr::StringLiteral(_)))
    };
    match (test.ops.as_ref(), right) {
        ([ast::CmpOp::Eq | ast::CmpOp::NotEq], ast::Expr::StringLiteral(_)) => {}
        ([ast::CmpOp::In | ast::CmpOp::NotIn], ast::Expr::Set(set)) if strings(&set.elts) => {}
        ([ast::CmpOp::In | ast::CmpOp::NotIn], ast::Expr::Tuple(tuple)) if strings(&tuple.elts) => {
        }
        _ => return None,
    }
    let [ast::Stmt::ImportFrom(import)] = guard.body.as_slice() else {
        return None;
    };
    let [alias] = import.names.as_slice() else {
        return None;
    };
    let [otherwise] = guard.elif_else_clauses.as_slice() else {
        return None;
    };
    let [fallback] = otherwise.body.as_slice() else {
        return None;
    };
    let name = alias.asname.as_ref().unwrap_or(&alias.name).as_str();
    (otherwise.test.is_none()
        && value_name(fallback) == Some(name)
        && module_value(fallback).is_some_and(literal_export))
    .then(|| vec![(name, &guard.body[0])])
}

fn single_import_binding(stmt: &ast::Stmt) -> Option<&str> {
    let aliases = match stmt {
        ast::Stmt::Import(import) => &import.names,
        ast::Stmt::ImportFrom(import) => &import.names,
        _ => return None,
    };
    let [alias] = aliases.as_slice() else {
        return None;
    };
    if let Some(name) = &alias.asname {
        return Some(name.as_str());
    }
    // An unaliased dotted import binds only the first component.
    let name = alias.name.as_str().split('.').next()?;
    (name != "*").then_some(name)
}

/// Allow an identity check with a closed builtin raise before importing a dependency.
/// Keep it in the initializer so even a failing guard runs in source order.
fn module_name_guard(statement: &ast::Stmt) -> bool {
    let ast::Stmt::If(guard) = statement else {
        return false;
    };
    let ast::Expr::Compare(test) = &*guard.test else {
        return false;
    };
    let [ast::Stmt::Raise(raise)] = guard.body.as_slice() else {
        return false;
    };
    if !guard.elif_else_clauses.is_empty()
        || !matches!(test.ops.as_ref(), [ast::CmpOp::Eq | ast::CmpOp::NotEq])
        || !matches!(test.operands.as_ref(),
            [ast::Expr::Name(n), ast::Expr::StringLiteral(_)] |
            [ast::Expr::StringLiteral(_), ast::Expr::Name(n)] if n.id == "__name__")
        || raise.cause.is_some()
    {
        return false;
    }
    let exception = match raise.exc.as_deref() {
        Some(ast::Expr::Name(name)) => name,
        Some(ast::Expr::Call(call))
            if call.arguments.keywords.is_empty()
                && call
                    .arguments
                    .args
                    .iter()
                    .all(|a| matches!(a, ast::Expr::StringLiteral(_))) =>
        {
            let ast::Expr::Name(name) = &*call.func else {
                return false;
            };
            name
        }
        _ => return false,
    };
    crate::frontend::exception_kind(exception.id.as_str()).is_some()
}

/// A closed literal tree cannot read a name introduced by a later import.
fn literal_export(expression: &ast::Expr) -> bool {
    match expression {
        ast::Expr::NoneLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::BytesLiteral(_) => true,
        ast::Expr::List(seq) => seq.elts.iter().all(literal_export),
        ast::Expr::Tuple(seq) => seq.elts.iter().all(literal_export),
        _ => false,
    }
}

/// The rebuilt C interface supports METH_VARARGS. Its stub contains signatures
/// and their type imports; no Python statements in the stub run at import time.
fn native_declaration(statement: &ast::Stmt) -> bool {
    match statement {
        ast::Stmt::Import(import) => import
            .names
            .iter()
            .all(|a| matches!(a.name.as_str(), "typing" | "collections.abc")),
        ast::Stmt::ImportFrom(import) => {
            import.level == 0
                && import.names.iter().all(|a| {
                    matches!(
                        (import.module.as_ref().map(|m| m.as_str()), a.name.as_str()),
                        (Some("typing" | "collections.abc"), "Callable")
                            | (Some("__future__"), "annotations")
                    )
                })
        }
        ast::Stmt::Expr(doc) => matches!(&*doc.value, ast::Expr::StringLiteral(_)),
        ast::Stmt::FunctionDef(f) => {
            !f.is_async
                && f.type_params.is_none()
                && f.decorator_list.is_empty()
                && f.returns.is_some()
                && f.parameters.args.is_empty()
                && f.parameters.kwonlyargs.is_empty()
                && f.parameters.vararg.is_none()
                && f.parameters.kwarg.is_none()
                && f.parameters
                    .posonlyargs
                    .iter()
                    .all(|p| p.annotation().is_some() && p.default.is_none())
                && matches!(f.body.as_slice(), [ast::Stmt::Expr(s)] if matches!(&*s.value, ast::Expr::EllipsisLiteral(_)))
        }
        _ => false,
    }
}

/// A single named assignment can initialize or replace a module value.
pub(crate) fn value_name(statement: &ast::Stmt) -> Option<&str> {
    Some(value_target(statement)?.as_name_expr()?.id.as_str())
}

pub(crate) fn value_target(statement: &ast::Stmt) -> Option<&ast::Expr> {
    match statement {
        ast::Stmt::Assign(assignment) if assignment.targets.len() == 1 => {
            Some(&assignment.targets[0])
        }
        ast::Stmt::AnnAssign(assignment) => Some(&assignment.target),
        _ => None,
    }
}

fn module_value(statement: &ast::Stmt) -> Option<&ast::Expr> {
    match statement {
        ast::Stmt::Assign(assignment) => Some(&assignment.value),
        ast::Stmt::AnnAssign(assignment) => assignment.value.as_deref(),
        _ => None,
    }
}

/// A runtime binding introduced directly in a module-level conditional branch.
fn conditional_export_name(statement: &ast::Stmt) -> Option<&str> {
    if let ast::Stmt::FunctionDef(function) = statement {
        Some(function.name.as_str())
    } else {
        module_value(statement)?;
        value_name(statement)
    }
}

#[cfg(test)]
mod native_tests {
    use super::SourceGraph;
    use std::fs;

    #[test]
    fn ordinary_dotted_import_keeps_its_checked_source() {
        let mut loader = super::Loader::new(
            std::path::PathBuf::from("/"),
            &[],
            &[],
            None,
            super::FileSystem::default(),
        )
        .unwrap();
        let child = loader.load_module("email.encoders").unwrap();
        assert!(!loader.modules[child].registered_namespace);
    }

    #[test]
    fn guarded_from_imports_bind_the_names_available_on_both_exits() {
        let directory = tempfile::tempdir().unwrap();
        let package = directory.path().join("settings");
        fs::create_dir(&package).unwrap();
        fs::write(
            package.join("values.py"),
            "label = 'ready'\nparts = (1,)\nextra = 2\n",
        )
        .unwrap();
        fs::write(
            package.join("__init__.py"),
            r#"__all__ = ["missing"]
previous = 0
try:
    from .values import label as _label
    from .values import parts
    from .values import extra
    from .values import extra as previous
except ImportError:
    _label = "fallback"
    parts = ()
    previous = 1
"#,
        )
        .unwrap();

        let entry = directory.path().join("main.py");
        fs::write(&entry, "from settings import _label, parts, previous\n").unwrap();
        let graph = SourceGraph::load(&entry, Some(directory.path()), &[]).unwrap();
        let module = graph
            .modules
            .iter()
            .find(|module| module.name == "settings")
            .unwrap();
        assert!(
            matches!(module.export("previous"), Some(super::Binding::Value(def))
            if module.rebound_values.contains(&def.statement))
        );
        assert_eq!(
            module.export("_label"),
            Some(super::Binding::ConditionalValue)
        );
        assert_eq!(
            module.export("parts"),
            Some(super::Binding::ConditionalValue)
        );
        assert_eq!(module.export("extra"), None);
        assert_eq!(module.export("missing"), None);
    }

    #[test]
    fn only_the_importing_initializer_needs_the_registered_namespace() {
        use super::{FileSystem, Loader, MEMORY_SOURCE_PATH, PathBuf};

        for (prefix, imported) in [("", false), ("from email import errors\n", true)] {
            let filename = PathBuf::from(MEMORY_SOURCE_PATH);
            let mut loader =
                Loader::new(PathBuf::from("/"), &[], &[], None, FileSystem::default()).unwrap();
            loader.allow_imports = false;
            let source =
                format!("{prefix}def score(x: int) -> int: return x * 3\nprint(score(14))\n");
            let entry = loader
                .add(
                    filename.clone(),
                    filename.clone(),
                    "__main__".into(),
                    false,
                    &source,
                )
                .unwrap();
            // Both graphs contain the same prepared child. Only one initializer imports it.
            let child = loader.load_source("email.errors", true).unwrap();
            assert!(loader.modules[child].registered_namespace);
            loader.scan(entry).unwrap();
            let mut graph = loader.finish(entry, filename, false, None).unwrap();
            let program = crate::frontend::lower_with_caller_cache(&mut graph, None, None).unwrap();
            assert!(match program {
                crate::CheckedProgram::Registered(Some(reason)) => {
                    imported
                        && reason
                            .to_string()
                            .contains("email.errors requires runtime import initialization")
                }
                crate::CheckedProgram::Native(_) => !imported,
                _ => false,
            });
        }
    }

    #[test]
    fn later_package_imports_are_visible_with_a_separate_type_declaration() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        fs::write(
            root.join("typing_extensions.py"),
            "def get_origin(value: int) -> int:\n    return value + 1\n",
        )
        .unwrap();
        fs::create_dir(root.join("helpers")).unwrap();
        fs::write(root.join("helpers/__init__.py"), "").unwrap();
        fs::write(
            root.join("helpers/score.py"),
            "def twice(value: int) -> int:\n    return value * 2\n",
        )
        .unwrap();
        let entry = root.join("main.py");
        let external = "from typing_extensions import get_origin";
        let local = "from helpers.score import twice";
        for (first, second) in [(external, local), (local, external)] {
            fs::write(
                &entry,
                format!("{first}\n{second}\nprint(get_origin(3), twice(3))\n"),
            )
            .unwrap();
            crate::check_file(&entry, Some(root)).unwrap();
        }
    }

    #[test]
    fn a_local_package_owns_its_bundled_child_name() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let package = root.join("email");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("__init__.py"), "").unwrap();
        let child = package.join("errors.py");
        fs::write(&child, "value: int = 42\n").unwrap();
        let entry = root.join("main.py");
        fs::write(&entry, "from email.errors import value\nprint(value)\n").unwrap();

        let graph = SourceGraph::load(&entry, Some(&root), &[]).unwrap();
        let selected = graph
            .modules
            .iter()
            .find(|module| module.name == "email.errors")
            .unwrap();
        assert_eq!(selected.path, child);
        assert!(selected.is_program_source());
    }

    #[test]
    fn native_declarations_follow_package_ownership_and_preserve_initializers() {
        let workspace = tempfile::tempdir().unwrap();
        let app = workspace.path().join("app");
        let wheel = workspace.path().join("wheel");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(wheel.join("pkg")).unwrap();
        let entry = app.join("program.py");
        fs::write(&entry, "from pkg import combine\nprint(combine(b'a'))\n").unwrap();
        fs::write(
            wheel.join("pkg/__init__.py"),
            "from .native import combine\n",
        )
        .unwrap();
        let stub = wheel.join("pkg/native.pyi");
        let archive = wheel.join("pkg/native.imre.a");
        fs::write(&stub, "def combine(data: bytes, /) -> int: ...\n").unwrap();
        let options = crate::CompileOptions {
            extension_paths: vec![wheel.clone()],
            ..Default::default()
        };
        assert!(
            crate::compile_file(&entry, None, &options)
                .err()
                .unwrap()
                .contains("requires a rebuilt archive")
        );
        fs::write(&archive, []).unwrap();
        let compiled = crate::compile_file(&entry, None, &options).unwrap();
        assert_eq!(
            compiled.extension_archives,
            [archive.canonicalize().unwrap()]
        );
        assert!(compiled.source_inputs.fresh());
        fs::write(&archive, b"updated archive").unwrap();
        assert!(!compiled.source_inputs.fresh());
        fs::write(&archive, []).unwrap();
        assert_eq!(
            compiled
                .llvm_ir
                .matches("call void @imre_extension_import(ptr @PyInit_native)")
                .count(),
            1
        );
        assert_eq!(
            compiled
                .llvm_ir
                .matches("call void @imre_extension_clear()")
                .count(),
            1
        );

        fs::write(&stub, "def combine(data: str, /) -> int: ...\n").unwrap();
        assert!(
            crate::compile_file(&entry, None, &options)
                .err()
                .unwrap()
                .contains("native declarations currently support")
        );

        for invalid in [
            "def combine(data: bytes) -> int: ...",
            "def combine(data: bytes = b'', /) -> int: ...",
            "def combine(*data: bytes) -> int: ...",
            "async def combine(data: bytes, /) -> int: ...",
            "@staticmethod\ndef combine(data: bytes, /) -> int: ...",
            "from elsewhere import value as combine",
        ] {
            fs::write(&stub, invalid).unwrap();
            let error =
                crate::check_file_with_extensions(&entry, None, std::slice::from_ref(&wheel))
                    .unwrap_err();
            assert!(
                error.contains("native stubs allow type-only imports"),
                "{error}"
            );
        }
        fs::write(
            &stub,
            "from typing import Callable as Callback\ndef combine(data: bytes, /) -> int: ...\n",
        )
        .unwrap();
        fs::write(&entry, "from pkg.native import Callback\n").unwrap();
        let error = crate::check_file_with_extensions(&entry, None, std::slice::from_ref(&wheel))
            .unwrap_err();
        assert!(error.contains("no static binding"), "{error}");

        fs::create_dir(app.join("pkg")).unwrap();
        fs::write(app.join("pkg/__init__.py"), "").unwrap();
        fs::write(&entry, "from pkg.native import combine\n").unwrap();
        assert!(
            crate::compile_file(&entry, None, &options)
                .err()
                .unwrap()
                .contains("cannot resolve first-party module")
        );
    }

    #[cfg(unix)]
    #[test]
    fn source_ownership_follows_the_winning_search_entry() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let installed = root.join("installed");
        fs::create_dir(&installed).unwrap();
        let entry = root.join("main.py");
        let dependency = installed.join("helper.py");
        fs::write(&entry, "import helper\n").unwrap();
        fs::write(&dependency, "value = 7\n").unwrap();
        let load =
            || SourceGraph::load(&entry, Some(&root), std::slice::from_ref(&installed)).unwrap();
        let graph = load();
        assert!(graph.modules[graph.entry].is_program_source());
        let helper = graph.modules.iter().find(|m| m.name == "helper").unwrap();
        assert!(!helper.is_program_source());

        let local = root.join("helper.py");
        std::os::unix::fs::symlink(&dependency, &local).unwrap();
        let graph = load();
        let helper = graph.modules.iter().find(|m| m.name == "helper").unwrap();
        assert!(helper.is_program_source());
        assert_eq!(helper.path, local);
        assert_eq!(fs::canonicalize(&helper.path).unwrap(), dependency);
    }

    #[cfg(unix)]
    #[test]
    fn changing_an_extension_root_invalidates_the_compiled_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let entry = root.join("main.py");
        fs::write(&entry, "from helper import value\nprint(value)\n").unwrap();
        for (name, value) in [("first", 23), ("second", 41)] {
            let directory = root.join(name);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("helper.py"), format!("value = {value}\n")).unwrap();
        }
        let selected = root.join("selected");
        std::os::unix::fs::symlink(root.join("first"), &selected).unwrap();
        let options = crate::CompileOptions {
            extension_paths: vec![selected.clone()],
            ..Default::default()
        };
        let compilation = crate::compile_file(&entry, Some(&root), &options).unwrap();
        assert!(compilation.source_inputs.fresh());
        fs::remove_file(&selected).unwrap();
        std::os::unix::fs::symlink(root.join("second"), &selected).unwrap();
        assert!(!compilation.source_inputs.fresh());
    }

    #[test]
    fn namespace_portions_track_directory_changes_and_concrete_module_precedence() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let extension = root.join("extension");
        let namespace = root.join("space");
        let other_portion = extension.join("space");
        fs::create_dir(&namespace).unwrap();
        fs::create_dir_all(&other_portion).unwrap();
        let entry = root.join("program.py");
        fs::write(&entry, "import space\n").unwrap();
        let load =
            || SourceGraph::load(&entry, Some(&root), std::slice::from_ref(&extension)).unwrap();

        let mut graph = load();
        let (inputs, _) = graph.disk_inputs(None);
        assert!(inputs.fresh());
        fs::remove_dir(&other_portion).unwrap();
        assert!(!inputs.fresh());
        fs::create_dir(&other_portion).unwrap();
        assert!(inputs.fresh());

        let stub = extension.join("space.pyi");
        fs::write(&stub, "def read() -> int: ...\n").unwrap();
        assert!(!inputs.fresh());
        fs::remove_file(stub).unwrap();
        assert!(inputs.fresh());

        let module = extension.join("space.py");
        fs::write(&module, "value = 42\n").unwrap();
        assert!(!inputs.fresh());
        let mut graph = load();
        let (_, paths) = graph.disk_inputs(None);
        assert!(paths.contains(&module));
        assert!(!paths.contains(&namespace.join("__init__.py")));
    }

    #[test]
    fn checked_discovery_sources_are_independent_of_search_roots() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let entry = root.join("main.py");
        fs::write(
            &entry,
            "from importlib.metadata import version\nfrom pkgutil import iter_modules\n",
        )
        .unwrap();
        for dirname in ["first", "quote'Ω"] {
            let extension = root.join(dirname);
            fs::create_dir(&extension).unwrap();
            let graph = SourceGraph::load(&entry, Some(root), &[extension]).unwrap();
            for (name, source) in [
                (
                    super::METADATA_VERSION,
                    concat!(
                        include_str!("../stdlib/importlib/metadata_typed.py"),
                        "\n",
                        include_str!("../stdlib/importlib/metadata_version.py"),
                    ),
                ),
                (
                    "pkgutil",
                    concat!(
                        include_str!("../stdlib/pkgutil.py"),
                        "\n",
                        include_str!("../stdlib/pkgutil/iter_modules.py"),
                    ),
                ),
            ] {
                let module = graph
                    .modules
                    .iter()
                    .find(|module| module.is_bundled(name))
                    .unwrap();
                assert_eq!(
                    module.original_source(),
                    source,
                    "physical source for {name}"
                );
                assert_eq!(
                    module.source,
                    crate::normalize_source(source, crate::SourceKind::File).unwrap(),
                    "checked source for {name}",
                );
            }
        }
    }

    #[test]
    fn project_import_config_uses_its_own_declarations() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        fs::create_dir(root.join("imre")).unwrap();
        fs::write(root.join("imre/__init__.py"), "").unwrap();
        fs::write(
            root.join("imre/_import_config.py"),
            "def filesystem_roots(n: int) -> int: return n + 1\n",
        )
        .unwrap();
        let entry = root.join("main.py");
        fs::write(
            &entry,
            concat!(
                "from imre._import_config import filesystem_roots\n",
                "import imre._import_config as config\n",
                "from importlib.metadata import version\n",
                "print(filesystem_roots(config.filesystem_roots(40)))\n",
            ),
        )
        .unwrap();
        crate::check_file(&entry, Some(root)).unwrap();
    }

    #[test]
    fn distinct_packages_cannot_supply_the_same_static_module_initializer() {
        let workspace = tempfile::tempdir().unwrap();
        let wheel = workspace.path().join("wheel");
        for package in ["left", "right"] {
            let path = wheel.join(package);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("__init__.py"), "").unwrap();
            fs::write(path.join("_core.pyi"), "def read() -> int: ...\n").unwrap();
            fs::write(path.join("_core.imre.a"), []).unwrap();
        }
        let entry = workspace.path().join("program.py");
        fs::write(&entry, "import left._core\nimport right._core\n").unwrap();
        let error = crate::check_file_with_extensions(&entry, None, &[wheel]).unwrap_err();
        assert!(
            error.contains("share the initialization symbol PyInit__core"),
            "{error}"
        );
    }
}
