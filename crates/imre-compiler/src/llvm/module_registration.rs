//! Link checked module registrations to their native initializers.

use super::registered::Output;
use crate::ir::{ModuleLoader, ModuleRegistration, PhysicalModuleKind};
use std::fmt::Write as _;

/// Name and file constants precede the initializer's constants in each output.
pub(crate) struct ModuleOperands<'a> {
    name: String,
    pub(crate) file: String,
    loader: &'a ModuleLoader<'a>,
}

impl ModuleOperands<'_> {
    pub(crate) fn write_call(self, output: &mut Output, main: &mut String, code: &str) {
        let (function, data, argument) = match self.loader {
            ModuleLoader::Source { builtin, text } => {
                let function = if *builtin {
                    "imre_dyn_register_builtin_source"
                } else {
                    "imre_dyn_register_source"
                };
                let data = text.map_or_else(|| "null".into(), |text| output.data(text));
                (
                    function,
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
                    PhysicalModuleKind::File => "imre_dyn_register_file_code",
                    PhysicalModuleKind::Zip => "imre_dyn_register_zip_source",
                };
                (function, output.data(digest), format!("i32 {future_flags}"))
            }
        };
        writeln!(
            main,
            "  call void @{function}(ptr %ctx, ptr {}, ptr {}, ptr {data}, {argument}, ptr {code})",
            self.name, self.file
        )
        .unwrap();
    }
}

impl Output {
    pub(crate) fn module_registration<'a>(
        &mut self,
        registration: &'a ModuleRegistration<'_>,
    ) -> ModuleOperands<'a> {
        ModuleOperands {
            name: self.data(registration.name.as_bytes()),
            file: self.data(registration.file.as_os_str().as_encoded_bytes()),
            loader: &registration.loader,
        }
    }
}
