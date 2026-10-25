//! Mutable TOML/JSON views reuse native ordered dictionaries and list storage.
//! A subscript or shallow spread retains the same nested container owner.

use ::toml::de::{DeTable, DeValue};
use std::collections::HashSet;
use std::rc::Rc;

use super::{Node, Nodes, View};
use crate::objects::mappings::{
    dict_set, imre_dict_contains, imre_dict_delete, imre_dict_get, imre_dict_new,
};
use crate::objects::{
    Error, Int, Kind, Object, Storage, StorageCell, Str, Words, acquire_rc, object,
};

#[derive(Clone)]
pub(in crate::objects) enum Value {
    Null,
    Bool(bool),
    Integer(Int),
    Float(f64),
    String(Rc<Str>),
    Datetime(Str),
    Array(Rc<Object>),
    Table(Rc<Object>),
}

impl Value {
    fn object(self) -> Rc<Object> {
        Rc::new(Object(StorageCell::new(Storage::Document(self))))
    }
    fn word(self) -> usize {
        Rc::into_raw(self.object()) as usize
    }
    fn array(
        values: impl ExactSizeIterator<Item = Result<Rc<Object>, Error>>,
    ) -> Result<Self, Error> {
        let mut storage = Words::new(5);
        storage
            .values
            .try_reserve_exact(values.len())
            .map_err(|_| Error::allocation())?;
        for value in values {
            storage.values.push(Rc::into_raw(value?) as usize);
        }
        Ok(Self::Array(storage.into_object()))
    }
    fn table(
        pairs: impl Iterator<Item = Result<(Rc<Str>, Rc<Object>), Error>>,
    ) -> Result<Self, Error> {
        // SAFETY: a fresh string/object dictionary transfers one owner.
        let dict = unsafe { Rc::from_raw(imre_dict_new(3, 5)) };
        for pair in pairs {
            let (key, value) = pair?;
            // SAFETY: the three owners root the exact mapping, string, and document.
            unsafe {
                dict_set(
                    Rc::as_ptr(&dict),
                    Rc::as_ptr(&key) as usize,
                    Rc::as_ptr(&value) as usize,
                )?
            };
        }
        Ok(Self::Table(dict))
    }
    fn items(source: &Object) -> Vec<(Rc<Str>, Rc<Object>)> {
        // SAFETY: this operation copies all owners before traversing child values.
        unsafe { source.0.with(|v| v.dictionary().document_pairs()) }
    }
    fn elements(source: &Object) -> Vec<Rc<Object>> {
        // SAFETY: only native list[document value] owners enter this path.
        unsafe {
            source.0.with(|s| {
                s.sequence()
                    .values
                    .iter()
                    .map(|&word| acquire_rc(word))
                    .collect()
            })
        }
    }
    fn snapshot(&self, nodes: &mut Vec<Node>) -> usize {
        let node = match self {
            Self::Null => Node::Null,
            Self::Bool(x) => Node::Bool(*x),
            Self::Integer(x) => Node::Integer(x.clone()),
            Self::Float(x) => Node::Float(*x),
            Self::String(x) => Node::String(x.clone()),
            Self::Datetime(x) => Node::Datetime(x.clone()),
            Self::Array(a) => Node::Array(
                Self::elements(a)
                    .iter()
                    .map(|value| Self::read(value).snapshot(nodes))
                    .collect(),
            ),
            Self::Table(t) => Node::Object(
                Self::items(t)
                    .iter()
                    .map(|(key, value)| ((**key).clone(), Self::read(value).snapshot(nodes)))
                    .collect(),
            ),
        };
        let index = nodes.len();
        nodes.push(node);
        index
    }
    pub(in crate::objects) fn view(&self) -> View {
        let mut nodes = Vec::new();
        let index = self.snapshot(&mut nodes);
        View {
            nodes: Nodes::Document(Rc::new(nodes)),
            index,
        }
    }
    fn from_json(view: &View, index: usize) -> Result<Self, Error> {
        if index == view.index
            && let Some(root) = view.snapshot_root()
        {
            return Self::from_json(&root, root.index);
        }
        Ok(match &view.nodes[index] {
            Node::Deferred(array) => Self::from_json(array.view(), array.view().index)?,
            Node::Null => Self::Null,
            Node::Bool(x) => Self::Bool(*x),
            Node::Integer(x) => Self::Integer(x.clone()),
            Node::Float(x) => Self::Float(*x),
            Node::String(x) => Self::String(x.clone()),
            Node::Datetime(x) => Self::Datetime(x.clone()),
            Node::Array(a) => Self::array(
                a.iter()
                    .map(|&index| Self::from_json(view, index).map(Self::object)),
            )?,
            Node::Object(t) => Self::table(t.iter().map(|(key, index)| {
                Ok((
                    Rc::new(key.clone()),
                    Self::from_json(view, *index)?.object(),
                ))
            }))?,
        })
    }
    fn from_toml(node: &DeValue<'_>) -> Result<Self, Error> {
        Ok(match node {
            DeValue::String(value) => Self::String(Rc::new(Str::new(value.as_ref()))),
            DeValue::Integer(value) => Self::Integer(Int::new(
                num_bigint::BigInt::parse_bytes(value.as_str().as_bytes(), value.radix())
                    .ok_or_else(|| Error::new(Kind::TomlDecodeError, "invalid integer"))?,
            )),
            DeValue::Float(value) => Self::Float(
                value
                    .as_str()
                    .parse()
                    .map_err(|_| Error::new(Kind::TomlDecodeError, "invalid float"))?,
            ),
            DeValue::Boolean(value) => Self::Bool(*value),
            DeValue::Datetime(value) => {
                use std::fmt::Write;
                let mut iso = String::new();
                if let Some(date) = value.date {
                    write!(iso, "{date}").unwrap();
                }
                if let Some(time) = value.time {
                    if value.date.is_some() {
                        iso.push(' ');
                    }
                    let second = time.second.unwrap_or(0);
                    if second > 59 {
                        return Err(Error::new(Kind::TomlDecodeError, "second must be in 0..59"));
                    }
                    write!(iso, "{:02}:{:02}:{second:02}", time.hour, time.minute).unwrap();
                    let micros = time.nanosecond.unwrap_or(0) / 1000;
                    if micros != 0 {
                        write!(iso, ".{micros:06}").unwrap();
                    }
                }
                if let Some(offset) = value.offset {
                    let offset = offset.to_string();
                    iso.push_str(if offset == "Z" { "+00:00" } else { &offset });
                }
                Self::Datetime(Str::new(iso))
            }
            DeValue::Array(values) => Self::array(
                values
                    .iter()
                    .map(|v| Self::from_toml(v.get_ref()).map(Self::object)),
            )?,
            DeValue::Table(table) => Self::table(table.iter().map(|(k, v)| {
                Ok((
                    Rc::new(Str::new(k.get_ref())),
                    Self::from_toml(v.get_ref())?.object(),
                ))
            }))?,
        })
    }
    fn read(obj: &Object) -> Self {
        // SAFETY: callers hold a typed document owner. No storage borrow escapes.
        unsafe {
            obj.0.with(|storage| match storage {
                Storage::Document(node) => node.clone(),
                _ => unreachable!("expected document"),
            })
        }
    }
    fn list(&self) -> Result<Rc<Object>, Error> {
        if let Self::Array(v) = self {
            Ok(v.clone())
        } else {
            Err(Error::new(Kind::TypeError, "expected a document list"))
        }
    }
    fn map(&self) -> Result<Rc<Object>, Error> {
        if let Self::Table(v) = self {
            Ok(v.clone())
        } else {
            Err(Error::new(
                Kind::TypeError,
                "expected a document dictionary",
            ))
        }
    }
    fn truth(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Integer(i) => i.as_small() != Some(0),
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::Datetime(_) => true,
            // SAFETY: these owners keep the list or table alive for the length read.
            Self::Array(a) => unsafe { a.0.with(|s| !s.sequence().values.is_empty()) },
            // SAFETY: this owner pins a native dictionary with its original storage.
            Self::Table(t) => unsafe { t.0.with(|s| !s.dictionary().is_empty()) },
        }
    }
    fn check_acyclic(&self, target: &Rc<Object>) -> Result<(), Error> {
        let (Self::Array(root) | Self::Table(root)) = self else {
            return Ok(());
        };
        let mut current = root.clone();
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        loop {
            if Rc::ptr_eq(&current, target) {
                return Err(Error::new(
                    Kind::ValueError,
                    "cyclic document mutation is unsupported",
                ));
            }
            // SAFETY: the source graph roots every child throughout this read-only
            // traversal. Its array and table words are exact document owners.
            unsafe {
                current.0.with(|storage| {
                    let visit = |word| {
                        let (Self::Array(child) | Self::Table(child)) =
                            Self::read(object(word as *const Object))
                        else {
                            return Ok(());
                        };
                        let id = Rc::as_ptr(&child);
                        if !seen.contains(&id) {
                            seen.try_reserve(1).map_err(|_| Error::allocation())?;
                            pending.try_reserve(1).map_err(|_| Error::allocation())?;
                            seen.insert(id);
                            pending.push(child);
                        }
                        Ok(())
                    };
                    match storage {
                        Storage::Sequence(items) => {
                            items.values.iter().copied().try_for_each(visit)
                        }
                        Storage::Dictionary(map) => map.document_values().try_for_each(visit),
                        _ => unreachable!("expected a document container"),
                    }
                })?;
            }
            let Some(next) = pending.pop() else {
                return Ok(());
            };
            current = next;
        }
    }
}

/// # Safety
/// The compiler pairs each opcode with three rooted words in its exact representations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_document(op: u32, a: usize, b: usize, c: usize) -> usize {
    // SAFETY: the opcode determines the concrete ABI types for raw inputs.
    let run = || unsafe {
        let object = |word: usize| object(word as *const Object);
        let doc = |word| Value::read(object(word));
        let str_ = |word| &*(word as *const Str);
        let ptr = |v: Rc<Object>| Rc::into_raw(v) as usize;
        Ok(match op {
            0 => Value::Null.word(),
            1 => Value::table(std::iter::empty())?.word(),
            2 => Value::array(std::iter::empty())?.word(),
            3 => Value::String(acquire_rc(a)).word(),
            4 => Value::Bool(a != 0).word(),
            5 => Value::Integer((*Int::borrow(a)).clone()).word(),
            6 => Value::Float(f64::from_bits(a as u64)).word(),
            7 => object(a).0.with(|s| {
                let Storage::Json(view) = s else {
                    unreachable!()
                };
                Value::from_json(view, view.index).map(Value::word)
            })?,
            8 => Value::Array(acquire_rc(a)).word(),
            9 => Value::Table(acquire_rc(a)).word(),
            10 => ptr(doc(a).map()?),
            11 => ptr(doc(a).list()?),
            12 => {
                let Value::String(s) = doc(a) else {
                    return Err(Error::new(Kind::TypeError, "expected a document string"));
                };
                Rc::into_raw(s) as usize
            }
            13 | 14 => {
                if op == 13 {
                    doc(a).map()?;
                } else {
                    doc(a).list()?;
                }
                ptr(acquire_rc(a))
            }
            15 => usize::from(matches!(doc(a), Value::Null)),
            16 => usize::from(matches!(doc(a), Value::Table(_))),
            17 => usize::from(matches!(doc(a), Value::Array(_))),
            18 => usize::from(matches!(doc(a), Value::String(_))),
            19 => {
                let mut repr = String::new();
                doc(a).view().write_repr(&mut repr);
                Rc::into_raw(Rc::new(Str::new(repr))) as usize
            }
            20 => usize::from(doc(a).truth()),
            21 => {
                let parsed = DeTable::parse(str_(a))
                    .map_err(|e| Error::new(Kind::TomlDecodeError, e.to_string()))?;
                Value::from_toml(&DeValue::Table(parsed.into_inner()))?.word()
            }
            22 | 27 => {
                let view = doc(a).view();
                let mut text = String::new();
                let result = view.table(view.index, "", false, &mut text);
                if op == 27 {
                    object(b).0.with_mut(|s| {
                        let Storage::File(f) = s else { unreachable!() };
                        f.write_bytes(text.as_bytes())
                    })?;
                }
                result?;
                if op == 22 {
                    Rc::into_raw(Rc::new(Str::new(text))) as usize
                } else {
                    0
                }
            }
            23 => {
                let target = doc(a).map()?;
                let source = doc(b).map()?;
                for (key, val) in Value::items(&source) {
                    dict_set(
                        Rc::as_ptr(&target),
                        Rc::as_ptr(&key) as usize,
                        Rc::as_ptr(&val) as usize,
                    )?;
                }
                ptr(acquire_rc(a))
            }
            24 => {
                let target = doc(a).map()?;
                if imre_dict_contains(Rc::as_ptr(&target), b) != 0 {
                    let old = imre_dict_get(Rc::as_ptr(&target), b);
                    imre_dict_delete(Rc::as_ptr(&target), b);
                    old
                } else {
                    ptr(acquire_rc(c))
                }
            }
            25 => {
                let target = doc(b).list()?;
                let values = Value::elements(object(a));
                for value in &values {
                    Value::read(value).check_acyclic(&target)?;
                }
                let old = target.0.with_mut(|s| {
                    let words = s.sequence_mut();
                    let new = values
                        .into_iter()
                        .map(|v| Rc::into_raw(v) as usize)
                        .collect();
                    let old = std::mem::replace(&mut words.values, new);
                    words.update_header();
                    old
                });
                for word in old {
                    drop(Rc::from_raw(word as *const Object));
                }
                0
            }
            26 => {
                let target = doc(b).map()?;
                doc(a).check_acyclic(&target)?;
                dict_set(Rc::as_ptr(&target), c, a)?;
                0
            }
            28 => Value::table(
                object(a)
                    .0
                    .with(|s| s.dictionary().document_null_keys())
                    .into_iter()
                    .map(|s| Ok((s, Value::Null.object()))),
            )?
            .word(),
            29 => doc(a).view().into_handle() as usize,
            _ => unreachable!("valid document opcode"),
        })
    };
    crate::finish(run(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_check_traverses_deep_shared_children_once() {
        let mut owners = vec![Words::new(5).into_object()];
        for _ in 0..8192 {
            let child = Value::Array(owners.last().unwrap().clone()).object();
            let mut parent = Words::new(5);
            parent.values.push(Rc::into_raw(child.clone()) as usize);
            parent.values.push(Rc::into_raw(child) as usize);
            owners.push(parent.into_object());
        }
        let root = Value::Array(owners.last().unwrap().clone());
        assert!(root.check_acyclic(&Words::new(5).into_object()).is_ok());
        assert!(root.check_acyclic(&owners[0]).is_err());
        drop(root);
        // Release parents before their independently rooted children.
        while owners.pop().is_some() {}
    }
}
