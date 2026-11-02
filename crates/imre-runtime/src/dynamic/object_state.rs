//! The default object state shared by Python classes with dictionaries and slots.
use super::{Context, Error, Id, NONE, Result, Value, builtins};

impl Context {
    pub(super) fn object_getstate(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&object, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "unbound method object.__getstate__() needs an argument",
            ));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "object.__getstate__() takes no keyword arguments",
            ));
        }
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "object.__getstate__() takes no arguments ({} given)",
                    rest.len()
                ),
            ));
        }
        let state = self.pickle_instance_state_dict(object)?;
        self.heap.keep(state);
        let class = self.exact_type(object)?;
        let cached = self.own_class_attr(class, "__slotnames__");
        let names = match cached {
            Some(names) => names,
            None => {
                let copyreg = self.module("copyreg")?;
                let helper = self.lookup_attr(copyreg, "_slotnames")?;
                self.call(helper, vec![class], Vec::new())?
            }
        };
        if names == NONE {
            return Ok(state);
        }
        let Some(storage) = self.list_storage(names) else {
            let message = if cached.is_some() {
                format!(
                    "{}.__slotnames__ should be a list or None, not {}",
                    self.class_name(class),
                    self.type_name(names)
                )
            } else {
                "copyreg._slotnames didn't return a list or None".to_owned()
            };
            return Err(Error::new("TypeError", message));
        };
        self.heap.keep(names);
        let Value::List(entries) = self.heap.get(storage) else {
            unreachable!()
        };
        let count = entries.len();
        if count == 0 {
            return Ok(state);
        }
        let slots = self.dict(Vec::new());
        for index in 0..count {
            let Value::List(entries) = self.heap.get(storage) else {
                unreachable!()
            };
            let key = entries[index];
            self.heap.keep(key);
            let name = builtins::attribute_name(self, key)?;
            match self.lookup_named_attr(object, &name, Some(key)) {
                Ok(value) => self.put_item(slots, key, value)?,
                Err(error) if self.is_attribute_error(&error) => {}
                Err(error) => return Err(error),
            }
            if !matches!(self.heap.get(storage), Value::List(items) if items.len() == count) {
                return Err(Error::new(
                    "RuntimeError",
                    "__slotnames__ changed size during iteration",
                ));
            }
        }
        if self.dict_entries(slots).unwrap().is_empty() {
            Ok(state)
        } else {
            Ok(self.alloc(Value::Tuple(vec![state, slots])))
        }
    }
}
