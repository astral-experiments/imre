use super::*;

impl Context {
    /// Keep Enum's display methods when the float data type would otherwise supply its slots.
    pub(in crate::dynamic) fn install_float_enum_display(
        &mut self,
        class: Id,
        member_type: Id,
        first_enum: Id,
    ) -> Result<()> {
        let Value::Class { scope, .. } = *self.heap.get(class) else {
            unreachable!("enum class")
        };
        let float_repr = self.builtin("float.__repr__");
        let object = self.builtin("object");
        let bases = self.lookup_attr(class, "__bases__")?;
        let Value::Tuple(bases) = self.heap.get(bases) else {
            unreachable!("enum class bases")
        };
        let value_repr = bases.iter().find_map(|&base| {
            self.find_in_mro(base, |ancestor| {
                if ancestor == object {
                    None
                } else if ancestor == first_enum || self.enum_values(ancestor).is_some() {
                    Some(self.class_attr(ancestor, "_value_repr_").unwrap_or(NONE))
                } else {
                    self.own_class_attr(ancestor, "__repr__")
                }
            })
        });
        self.bind(scope, "_value_repr_", value_repr.unwrap_or(float_repr));
        for name in ["__repr__", "__str__", "__format__"] {
            if self.own_class_attr(class, name).is_some() {
                continue;
            }
            let found = self.class_attr(class, name);
            if found == self.class_attr(member_type, name) || found == self.class_attr(object, name)
            {
                let method = self
                    .find_in_mro(first_enum, |ancestor| {
                        (ancestor != object)
                            .then(|| self.own_class_attr(ancestor, name))
                            .flatten()
                    })
                    .unwrap_or_else(|| self.alloc(Value::Native(format!("enum.Enum.{name}"))));
                self.bind(scope, name, method);
            }
        }
        Ok(())
    }

    /// Render from the member name and data type's stored repr, independently of the float payload.
    pub(in crate::dynamic) fn float_enum_display(
        &mut self,
        method: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        if method == "enum.Enum.__format__" {
            let [member, spec] = builtins::bind_builtin_arguments(
                "__format__",
                args,
                kwargs,
                ["self", "format_spec"],
                2,
            )?;
            let text = builtins::call(self, "str", &[member.unwrap()], &[])?;
            return builtins::call(self, "str.__format__", &[text, spec.unwrap()], &[]);
        }
        let [member] = builtins::bind_builtin_arguments(method, args, kwargs, ["self"], 1)?;
        let member = member.unwrap();
        // Attribute hooks may delete fields that the following operations still need.
        let attr = |ctx: &mut Self, obj, field| -> Result<Id> {
            let value = ctx.lookup_attr(obj, field)?;
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(value);
            Ok(value)
        };
        let renderer = if method == "enum.Enum.__repr__" {
            let class = attr(self, member, "__class__")?;
            let renderer = attr(self, class, "_value_repr_")?;
            Some(if self.truth(renderer)? {
                renderer
            } else {
                self.builtin("repr")
            })
        } else {
            None
        };
        let class = attr(self, member, "__class__")?;
        let class_name = attr(self, class, "__name__")?;
        let name = attr(self, member, "_name_")?;
        let rendered = if let Some(renderer) = renderer {
            let value = attr(self, member, "_value_")?;
            let rendered = self.call(renderer, vec![value], Vec::new())?;
            self.heap.reserve_temporaries(1)?;
            self.heap.keep(rendered);
            Some(rendered)
        } else {
            None
        };
        let class_name = builtins::call(self, "str", &[class_name], &[])?;
        let dot = self.text(".");
        let name = builtins::call(self, "str", &[name], &[])?;
        let parts = if let Some(rendered) = rendered {
            let value = builtins::call(self, "str", &[rendered], &[])?;
            vec![
                self.text("<"),
                class_name,
                dot,
                name,
                self.text(": "),
                value,
                self.text(">"),
            ]
        } else {
            vec![class_name, dot, name]
        };
        let empty = self.text("");
        let parts = self.alloc(Value::Tuple(parts));
        builtins::call(self, "str.join", &[empty, parts], &[])
    }
}
