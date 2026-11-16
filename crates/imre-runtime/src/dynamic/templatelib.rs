//! Immutable objects shared by t-string literals and string.templatelib's constructors.

use super::{Context, Error, Id, NONE, Result, Value, builtins, value};

const INTERPOLATION_FIELDS: [&str; 4] = ["value", "expression", "conversion", "format_spec"];

pub(super) fn class_name(name: &str) -> Option<&'static str> {
    [
        value::TEMPLATE_TYPE,
        value::INTERPOLATION_TYPE,
        value::TEMPLATE_ITER_TYPE,
    ]
    .into_iter()
    .find(|&class| class == name)
}

pub(super) fn construct(
    ctx: &mut Context,
    class: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if class == value::TEMPLATE_TYPE {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "Template.__new__ only accepts *args arguments",
            ));
        }
        return ctx.template_from_parts(args);
    }
    let [value, expression, conversion, format_spec] =
        builtins::bind_builtin_arguments("Interpolation", args, kwargs, INTERPOLATION_FIELDS, 1)?;
    let empty = ctx.heap.string_literal("")?;
    ctx.template_interpolation(
        value.expect("required interpolation value"),
        expression.unwrap_or(empty),
        conversion.unwrap_or(NONE),
        format_spec.unwrap_or(empty),
    )
}

impl Context {
    pub(super) fn template_interpolation(
        &mut self,
        value: Id,
        expression: Id,
        conversion: Id,
        format_spec: Id,
    ) -> Result<Id> {
        for (field, part) in [
            ("expression", expression),
            ("conversion", conversion),
            ("format_spec", format_spec),
        ] {
            if field == "conversion" && part == NONE {
                continue;
            }
            let Some(storage) = self.string_storage(part) else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "Interpolation() argument '{field}' must be str, not {}",
                        self.type_name(part)
                    ),
                ));
            };
            if field == "conversion"
                && !matches!(self.heap.get(storage), Value::Str(text) if matches!(text.as_str(), "a" | "r" | "s"))
            {
                return Err(Error::new(
                    "ValueError",
                    "Interpolation() argument 'conversion' must be one of 's', 'a' or 'r'",
                ));
            }
        }
        Ok(self.alloc(Value::Interpolation {
            value,
            expression,
            conversion,
            format_spec,
        }))
    }

    pub(super) fn concat_template_strings(&mut self, left: Id, right: Id) -> Result<Id> {
        // PyUnicode_Concat can retain an exact string operand, but must copy a subclass.
        match (self.heap.get(left), self.heap.get(right)) {
            (Value::Str(s), Value::Str(_) | Value::SurrogateStr(_)) if s.is_empty() => {
                return Ok(right);
            }
            (Value::Str(_) | Value::SurrogateStr(_), Value::Str(s)) if s.is_empty() => {
                return Ok(left);
            }
            _ => {}
        }
        self.concat_strings(left, right)
    }

    fn template_from_parts(&mut self, parts: &[Id]) -> Result<Id> {
        let empty = self.heap.string_literal("")?;
        let mut strings = Vec::new();
        let mut interpolations = Vec::new();
        let count = parts
            .iter()
            .filter(|&&id| matches!(self.heap.get(id), Value::Interpolation { .. }))
            .count();
        strings
            .try_reserve_exact(count + 1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        interpolations
            .try_reserve_exact(count)
            .map_err(|_| Error::new("MemoryError", ""))?;
        let mut pending = None;
        for &part in parts {
            if self.string_storage(part).is_some() {
                pending = Some(match pending {
                    Some(prefix) => self.concat_template_strings(prefix, part)?,
                    None => part,
                });
            } else if matches!(self.heap.get(part), Value::Interpolation { .. }) {
                strings.push(pending.take().unwrap_or(empty));
                interpolations.push(part);
            } else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "Template.__new__ *args need to be of type 'str' or 'Interpolation', got {}",
                        self.type_name(part)
                    ),
                ));
            }
        }
        strings.push(pending.unwrap_or(empty));
        let strings = self.heap.packed_tuple(strings, false);
        let interpolations = self.heap.packed_tuple(interpolations, false);
        Ok(self.make_template(strings, interpolations))
    }

    pub(super) fn template_from_literal(&mut self, strings: Id, interpolations: Id) -> Result<Id> {
        // Older linked providers supply the fields in a list.
        let interpolations = if matches!(self.heap.get(interpolations), Value::Tuple(_)) {
            interpolations
        } else {
            let fields = self.items(interpolations)?;
            self.heap.packed_tuple(fields, false)
        };
        // CPython folds BUILD_TUPLE into a code constant up to its stack-use guideline.
        let strings = match self.heap.get(strings) {
            Value::Tuple(items) if items.len() > 30 => self.heap.packed_tuple(items.clone(), false),
            _ => strings,
        };
        Ok(self.make_template(strings, interpolations))
    }

    fn make_template(&mut self, strings: Id, interpolations: Id) -> Id {
        self.alloc(Value::Template {
            strings,
            interpolations,
        })
    }

    pub(super) fn template_values(&mut self, interpolations: Id) -> Result<Id> {
        let Value::Tuple(items) = self.heap.get(interpolations) else {
            unreachable!("template interpolations tuple");
        };
        let mut values = Vec::new();
        values
            .try_reserve_exact(items.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        values.extend(items.iter().map(|&item| match self.heap.get(item) {
            Value::Interpolation { value, .. } => *value,
            _ => unreachable!("template interpolation"),
        }));
        Ok(self.heap.packed_tuple(values, false))
    }

    pub(super) fn template_class_attr(&mut self, class: &'static str, name: &str) -> Option<Id> {
        let short = class.rsplit('.').next().unwrap();
        let field = if class == value::TEMPLATE_ITER_TYPE {
            None
        } else if class == value::TEMPLATE_TYPE {
            ["strings", "interpolations"]
                .into_iter()
                .find(|&field| field == name)
        } else {
            INTERPOLATION_FIELDS
                .into_iter()
                .find(|&field| field == name)
        };
        if let Some(field) = field {
            return Some(self.readonly_member(class, field));
        }
        let value = match name {
            "__module__" => self.text("string.templatelib"),
            "__name__" | "__qualname__" => self.text(short),
            "__doc__" if class == value::TEMPLATE_ITER_TYPE => {
                self.text("Template iterator object")
            }
            "__doc__" => self.text(format!("{short} object")),
            "__base__" => self.builtin("object"),
            "__match_args__" if class == value::INTERPOLATION_TYPE => {
                let key = format!("{class}.__match_args__");
                if let Some(&cached) = self.builtins.get(&key) {
                    return Some(cached);
                }
                let names = INTERPOLATION_FIELDS
                    .iter()
                    .map(|field| self.heap.code_name(field))
                    .collect();
                let tuple = self.heap.packed_tuple(names, true);
                self.builtins.insert(key, tuple);
                tuple
            }
            "values" if class == value::TEMPLATE_TYPE => {
                let key = format!("{class}.values");
                if let Some(&cached) = self.builtins.get(&key) {
                    return Some(cached);
                }
                let get = self.builtin("string.templatelib.Template.get_values");
                let descriptor = self.alloc(Value::GetSetDescriptor {
                    owner: class,
                    name: "values",
                    get,
                    set: None,
                });
                self.builtins.insert(key, descriptor);
                descriptor
            }
            _ => return None,
        };
        Some(value)
    }

    pub(super) fn check_template_write(&self, obj: Id, name: &str) -> Result<()> {
        let class = match self.heap.get(obj) {
            Value::Native(class) if class_name(class).is_some() => {
                return Err(Error::new(
                    "TypeError",
                    format!("cannot set '{name}' attribute of immutable type '{class}'"),
                ));
            }
            Value::Template { .. } => value::TEMPLATE_TYPE,
            Value::TemplateIter { .. } => value::TEMPLATE_ITER_TYPE,
            Value::Interpolation { .. } => value::INTERPOLATION_TYPE,
            _ => return Ok(()),
        };
        let member = (class == value::TEMPLATE_TYPE
            && matches!(name, "strings" | "interpolations"))
            || (class == value::INTERPOLATION_TYPE && INTERPOLATION_FIELDS.contains(&name));
        if member {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        let message = if class == value::TEMPLATE_TYPE && name == "values" {
            format!("attribute 'values' of '{class}' objects is not writable")
        } else {
            format!(
                "'{class}' object has no attribute '{name}' and no __dict__ for setting new attributes"
            )
        };
        Err(Error::new("AttributeError", message))
    }
}
