//! Link checked module registrations to their native initializers.

use super::body::{begin_main, end_main};
use super::registered::{ABI, Output};
use crate::ir::{ModuleLoader, ModuleRegistration, PhysicalModuleKind};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

const SOURCE_ARGS: &str = "ptr, ptr, ptr, ptr, i64, ptr";
const PHYSICAL_ARGS: &str = "ptr, ptr, ptr, ptr, i32, ptr";

/// Root constants, registration calls and declarations for one output layout.
#[derive(Clone)]
pub(crate) struct ModuleLauncher {
    pub(crate) output: Output,
    main: String,
    declarations: HashSet<String>,
}

impl ModuleLauncher {
    pub(crate) fn new(config: &str, keep_docstrings: bool) -> Self {
        let mut output = Output {
            keep_docstrings,
            ..Output::default()
        };
        let config = output.data(config.as_bytes());
        let mut main = String::from("\n");
        begin_main(&mut main, true, "");
        writeln!(
            main,
            "entry:\n  %ctx = call ptr @imre_dyn_new(i32 %argc, ptr %argv, ptr {config})"
        )
        .unwrap();
        Self {
            output,
            main,
            declarations: HashSet::new(),
        }
    }

    pub(crate) fn declares(&self, symbol: &str) -> bool {
        self.declarations.contains(symbol)
    }

    // The name and file precede every initializer's constants, including a stored error.
    fn module_operands(&mut self, name: &str, file: &Path) -> (String, String) {
        (
            self.output.data(name.as_bytes()),
            self.output.data(file.as_os_str().as_encoded_bytes()),
        )
    }

    pub(crate) fn module_registration<'a>(
        &mut self,
        registration: &'a ModuleRegistration<'_>,
    ) -> ModuleOperands<'a> {
        let (name, file) = self.module_operands(registration.name, registration.file);
        ModuleOperands {
            name,
            file,
            loader: &registration.loader,
        }
    }

    pub(crate) fn external(&mut self, symbol: &str) -> String {
        if self.declarations.insert(symbol.to_owned()) {
            writeln!(self.output.functions, "declare i64 @{symbol}(ptr, i64)").unwrap();
        }
        format!("@{symbol}")
    }

    fn register(&mut self, function: &str, parameters: &str, arguments: std::fmt::Arguments<'_>) {
        if !self.declares(function) {
            self.declarations.insert(function.to_owned());
            writeln!(
                self.output.functions,
                "declare void @{function}({parameters})"
            )
            .unwrap();
        }
        writeln!(self.main, "  call void @{function}({arguments})").unwrap();
    }

    /// Attach one checked input to the module registered immediately before its inputs.
    pub(crate) fn register_single(
        &mut self,
        owner: &str,
        filename: &str,
        source: &str,
        flags: u32,
        code: &str,
    ) {
        let text = self.output.data(source.as_bytes());
        self.register(
            "imre_dyn_register_module_single",
            "ptr, ptr, ptr, ptr, i64, i32, ptr",
            format_args!(
                "ptr %ctx, ptr {owner}, ptr {filename}, ptr {text}, i64 {}, i32 {flags}, ptr {code}",
                source.len(),
            ),
        );
    }

    pub(crate) fn register_error(
        &mut self,
        (name, file, source): (&str, &Path, &str),
        message: &str,
        position: Option<[usize; 4]>,
        in_args: bool,
        parse_error: bool,
        digest: Option<&[u8; 32]>,
    ) {
        let (name, file) = self.module_operands(name, file);
        let message = self.output.data(message.as_bytes());
        let position = self.output.position(position);
        let text = self.output.data(source.as_bytes());
        let hash = digest.map_or_else(|| "null".into(), |digest| self.output.data(digest));
        let error_kind = if parse_error { 2 } else { u8::from(in_args) };
        self.register(
            "imre_dyn_register_module_error",
            "ptr, ptr, ptr, ptr, i64, ptr, ptr, i8, ptr",
            format_args!(
                "ptr %ctx, ptr {name}, ptr {file}, ptr {text}, i64 {}, ptr {message}, ptr {position}, i8 {error_kind}, ptr {hash}",
                source.len(),
            ),
        );
    }

    pub(crate) fn finish(mut self) -> String {
        self.output.integer_prelude();
        end_main(&mut self.main, "@imre_dyn_finish(ptr %ctx)");
        module_parts(&self.output.constants, &self.output.functions, &self.main).concat()
    }
}

/// The ABI, constants and functions precede either the executable entry or a provider alias.
fn module_parts<'a>(constants: &'a str, functions: &'a str, entry: &'a str) -> [&'a str; 6] {
    [ABI, "\n", constants, "\n", functions, entry]
}

/// An initializer whose content determines its link name before its alias is written.
pub(crate) struct ProviderBody {
    constants: String,
    functions: String,
    entry: String,
}

impl ProviderBody {
    pub(crate) fn new(mut output: Output, entry: String) -> Self {
        output.integer_prelude();
        Self {
            constants: output.constants,
            functions: output.functions,
            entry,
        }
    }

    pub(crate) fn parts<'a>(&'a self, alias: &'a str) -> [&'a str; 6] {
        module_parts(&self.constants, &self.functions, alias)
    }

    pub(crate) fn llvm_ir(&self, symbol: &str, seal: &str) -> String {
        let alias = format!("@{symbol} = alias i64 (ptr, i64), ptr {}\n{seal}", self.entry);
        self.parts(&alias).concat()
    }
}

/// The loader payload follows the initializer's constants in each output.
pub(crate) struct ModuleOperands<'a> {
    pub(crate) name: String,
    pub(crate) file: String,
    loader: &'a ModuleLoader<'a>,
}

impl ModuleOperands<'_> {
    pub(crate) fn write_call(&self, launcher: &mut ModuleLauncher, code: &str) {
        let name = &self.name;
        let file = &self.file;
        let mut declared_path = String::new();
        let ((function, parameters), data, argument) = match self.loader {
            ModuleLoader::Source { builtin, text } => {
                let function = if *builtin {
                    "imre_dyn_register_builtin_source"
                } else {
                    "imre_dyn_register_source"
                };
                let data = text.map_or_else(|| "null".into(), |text| launcher.output.data(text));
                (
                    (function, SOURCE_ARGS),
                    data,
                    format!("i64 {}", text.map_or(0, <[u8]>::len)),
                )
            }
            ModuleLoader::Physical {
                kind,
                digest,
                future_flags,
            } => {
                let function = match kind {
                    PhysicalModuleKind::File => ("imre_dyn_register_file_code", PHYSICAL_ARGS),
                    PhysicalModuleKind::DeclaredFile(path) => {
                        let origin = launcher.output.data(path.as_os_str().as_encoded_bytes());
                        write!(declared_path, ", ptr {origin}").unwrap();
                        (
                            "imre_dyn_register_declared_file_code_at",
                            "ptr, ptr, ptr, ptr, i32, ptr, ptr",
                        )
                    }
                    PhysicalModuleKind::Zip => ("imre_dyn_register_zip_source", PHYSICAL_ARGS),
                };
                (
                    function,
                    launcher.output.data(digest),
                    format!("i32 {future_flags}"),
                )
            }
        };
        launcher.register(
            function,
            parameters,
            format_args!(
                "ptr %ctx, ptr {name}, ptr {file}, ptr {data}, {argument}, ptr {code}{declared_path}"
            ),
        );
    }
}
