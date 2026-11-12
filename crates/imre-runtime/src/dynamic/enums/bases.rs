use super::*;

impl Context {
    /// Give the native enum constructors the writable class namespaces of their Python counterparts.
    pub(in crate::dynamic) fn install_enum_base(&mut self, class: Id, kind: &str) {
        let scope = self.scope("enum", None);
        self.enum_scopes.insert(class, scope);
        let module = self.text("enum");
        self.bind(scope, "__module__", module);
        let members = self.dict(Vec::new());
        self.bind(scope, "_member_map_", members);

        let (member_type, display, string) = match kind {
            "enum.Flag" => ("object", "enum.Flag.__repr__", "enum.Enum.__str__"),
            "enum.IntFlag" => ("int", "enum.Flag.__repr__", "int.__repr__"),
            "enum.IntEnum" => ("int", "enum.Enum.__repr__", "int.__repr__"),
            "enum.StrEnum" => ("str", "enum.Enum.__repr__", "str.__str__"),
            _ => ("object", "enum.Enum.__repr__", "enum.Enum.__str__"),
        };
        // Enum introduces member dictionaries; a scalar mixin introduces its own getset.
        if kind == "enum.Enum" || member_type != "object" {
            let descriptor = self.alloc(Value::LayoutDescriptor(class, "__dict__"));
            self.bind(scope, "__dict__", descriptor);
        }
        let base = self.builtin(member_type);
        self.bind(scope, "_member_type_", base);
        let value_repr = if member_type == "object" {
            NONE
        } else {
            self.builtin(&format!("{member_type}.__repr__"))
        };
        self.bind(scope, "_value_repr_", value_repr);
        let repr = self.builtin(display);
        self.bind(scope, "__repr__", repr);
        let string = self.builtin(string);
        self.bind(scope, "__str__", string);
        let format = if member_type == "object" {
            self.builtin("enum.Enum.__format__")
        } else {
            self.builtin(&format!("{member_type}.__format__"))
        };
        self.bind(scope, "__format__", format);
        if kind == "enum.Enum" {
            for name in ["__copy__", "__deepcopy__", "__reduce_ex__"] {
                let method = self.builtin(&format!("enum.Enum.{name}"));
                self.bind(scope, name, method);
            }
        } else {
            self.builtin("enum.Enum");
        }
        if kind == "enum.IntFlag" {
            self.builtin("enum.Flag");
        } else if kind == "enum.Flag" {
            let callable = self.builtin("enum.Flag._missing_");
            let dictionary = self.dict(Vec::new());
            let missing = self.alloc(Value::ClassMethod {
                callable,
                dictionary,
            });
            self.bind(scope, "_missing_", missing);
            let repr = self.builtin("repr");
            self.bind(scope, "_numeric_repr_", repr);
        }
    }

    pub(in crate::dynamic) fn enum_base_attribute(
        &mut self,
        class: Id,
        scope: Id,
        name: &str,
    ) -> Result<Option<Id>> {
        let attribute = match name {
            "__dict__" => self.class_dictionary(scope),
            "__members__" => {
                let members = self
                    .class_attr(class, "_member_map_")
                    .ok_or_else(|| self.missing_attribute(class, "_member_map_"))?;
                self.mapping_proxy(members)?
            }
            "__name__" | "__qualname__" => {
                let short = self
                    .class_name(class)
                    .rsplit('.')
                    .next()
                    .unwrap()
                    .to_owned();
                self.text(short)
            }
            _ => {
                let Some(value) = self.class_attr(class, name) else {
                    return Ok(None);
                };
                self.bind_attribute(value, NONE, class)?
            }
        };
        Ok(Some(attribute))
    }

    pub(in crate::dynamic) fn update_enum_base(
        &mut self,
        scope: Id,
        name: &str,
        value: Option<Id>,
    ) -> Result<()> {
        if matches!(name, "__dict__" | "__mro__" | "__base__" | "__members__") {
            return Err(Error::new(
                "AttributeError",
                format!("attribute '{name}' is not writable"),
            ));
        }
        // Native layouts have fixed base lineages and immutable internal type names.
        if matches!(
            name,
            "__bases__" | "__name__" | "__qualname__" | "__class__"
        ) {
            return Err(Error::new(
                "TypeError",
                format!("cannot change '{name}' of a native type"),
            ));
        }
        if let Some(value) = value {
            self.bind(scope, name, value);
            if name == "__del__" {
                self.register_class_finalizers();
            }
        } else {
            let key = self.text(name);
            if !self.remove_binding(scope, name, key)? {
                return Err(Error::new("AttributeError", name));
            }
        }
        Ok(())
    }
}
