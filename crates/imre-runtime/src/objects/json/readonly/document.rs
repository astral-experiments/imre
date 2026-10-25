//! Mutable TOML/JSON views reuse native ordered dictionaries and list storage.
//! A subscript or shallow spread retains the same nested container owner.

use crate::ObjectOwner;
use crate::StrOwner;
use ::toml::de::{DeTable, DeValue};
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
    String(StrOwner),
    Datetime(Str),
    Array(ObjectOwner),
    Table(ObjectOwner),
}

impl Value {
    fn object(self) -> ObjectOwner {
        ObjectOwner::new(Object(StorageCell::new(Storage::Document(self))))
    }
    fn word(self) -> usize {
        ObjectOwner::into_raw(self.object()) as usize
    }
    fn array(
        values: impl ExactSizeIterator<Item = Result<ObjectOwner, Error>>,
    ) -> Result<Self, Error> {
        let mut storage = Words::new(5);
        storage
            .values
            .try_reserve_exact(values.len())
            .map_err(|_| Error::allocation())?;
        for value in values {
            storage.values.push(ObjectOwner::into_raw(value?) as usize);
        }
        Ok(Self::Array(storage.into_object()))
    }
    fn table(
        pairs: impl Iterator<Item = Result<(StrOwner, ObjectOwner), Error>>,
    ) -> Result<Self, Error> {
        // SAFETY: a fresh string/object dictionary transfers one owner.
        let dict = unsafe { ObjectOwner::from_raw(imre_dict_new(3, 5)) };
        for pair in pairs {
            let (key, value) = pair?;
            // SAFETY: the three owners root the exact mapping, string, and document.
            unsafe {
                dict_set(
                    ObjectOwner::as_ptr(&dict),
                    StrOwner::as_ptr(&key) as usize,
                    ObjectOwner::as_ptr(&value) as usize,
                )?
            };
        }
        Ok(Self::Table(dict))
    }
    fn items(source: &Object) -> Vec<(StrOwner, ObjectOwner)> {
        // SAFETY: this operation copies all owners before traversing child values.
        unsafe { source.0.with(|v| v.dictionary().document_pairs()) }
    }
    fn elements(source: &Object) -> Vec<ObjectOwner> {
        // SAFETY: only native list[document value] owners enter this path.
        unsafe {
            source.0.with(|s| {
                s.sequence()
                    .values
                    .iter()
                    .map(|&word| ObjectOwner::acquire_word(word))
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
                    StrOwner::new(key.clone()),
                    Self::from_json(view, *index)?.object(),
                ))
            }))?,
        })
    }
    fn from_toml(node: &DeValue<'_>) -> Result<Self, Error> {
        Ok(match node {
            DeValue::String(value) => Self::String(StrOwner::new(Str::new(value.as_ref()))),
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
                    StrOwner::new(Str::new(k.get_ref())),
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
    fn list(&self) -> Result<ObjectOwner, Error> {
        if let Self::Array(v) = self {
            Ok(v.clone())
        } else {
            Err(Error::new(Kind::TypeError, "expected a document list"))
        }
    }
    fn map(&self) -> Result<ObjectOwner, Error> {
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
    fn reaches(&self, target: &ObjectOwner) -> bool {
        match self {
            Self::Array(v) => {
                ObjectOwner::ptr_eq(v, target)
                    || Self::elements(v)
                        .iter()
                        .any(|child| Self::read(child).reaches(target))
            }
            Self::Table(v) => {
                ObjectOwner::ptr_eq(v, target)
                    || Self::items(v)
                        .iter()
                        .any(|(_, child)| Self::read(child).reaches(target))
            }
            _ => false,
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
        let ptr = |v: ObjectOwner| ObjectOwner::into_raw(v) as usize;
        Ok(match op {
            0 => Value::Null.word(),
            1 => Value::table(std::iter::empty())?.word(),
            2 => Value::array(std::iter::empty())?.word(),
            3 => Value::String(StrOwner::acquire_word(a)).word(),
            4 => Value::Bool(a != 0).word(),
            5 => Value::Integer((*Int::borrow(a)).clone()).word(),
            6 => Value::Float(f64::from_bits(a as u64)).word(),
            7 => object(a).0.with(|s| {
                let Storage::Json(view) = s else {
                    unreachable!()
                };
                Value::from_json(view, view.index).map(Value::word)
            })?,
            8 => Value::Array(ObjectOwner::acquire_word(a)).word(),
            9 => Value::Table(ObjectOwner::acquire_word(a)).word(),
            10 => ptr(doc(a).map()?),
            11 => ptr(doc(a).list()?),
            12 => {
                let Value::String(s) = doc(a) else {
                    return Err(Error::new(Kind::TypeError, "expected a document string"));
                };
                StrOwner::into_raw(s) as usize
            }
            13 | 14 => {
                if op == 13 {
                    doc(a).map()?;
                } else {
                    doc(a).list()?;
                }
                ptr(ObjectOwner::acquire_word(a))
            }
            15 => usize::from(matches!(doc(a), Value::Null)),
            16 => usize::from(matches!(doc(a), Value::Table(_))),
            17 => usize::from(matches!(doc(a), Value::Array(_))),
            18 => usize::from(matches!(doc(a), Value::String(_))),
            19 => {
                let mut repr = String::new();
                doc(a).view().write_repr(&mut repr);
                StrOwner::into_raw(StrOwner::new(Str::new(repr))) as usize
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
                    StrOwner::into_raw(StrOwner::new(Str::new(text))) as usize
                } else {
                    0
                }
            }
            23 => {
                let target = doc(a).map()?;
                let source = doc(b).map()?;
                for (key, val) in Value::items(&source) {
                    dict_set(
                        ObjectOwner::as_ptr(&target),
                        StrOwner::as_ptr(&key) as usize,
                        ObjectOwner::as_ptr(&val) as usize,
                    )?;
                }
                ptr(ObjectOwner::acquire_word(a))
            }
            24 => {
                let target = doc(a).map()?;
                if imre_dict_contains(ObjectOwner::as_ptr(&target), b) != 0 {
                    let old = imre_dict_get(ObjectOwner::as_ptr(&target), b);
                    imre_dict_delete(ObjectOwner::as_ptr(&target), b);
                    old
                } else {
                    ptr(ObjectOwner::acquire_word(c))
                }
            }
            25 => {
                let target = doc(b).list()?;
                let values = Value::elements(object(a));
                if values.iter().any(|v| Value::read(v).reaches(&target)) {
                    return Err(Error::new(
                        Kind::ValueError,
                        "cyclic document mutation is unsupported",
                    ));
                }
                let old = target.0.with_mut(|s| {
                    let words = s.sequence_mut();
                    let new = values
                        .into_iter()
                        .map(|v| ObjectOwner::into_raw(v) as usize)
                        .collect();
                    let old = std::mem::replace(&mut words.values, new);
                    words.update_header();
                    old
                });
                for word in old {
                    drop(ObjectOwner::from_raw(word as *const Object));
                }
                0
            }
            26 => {
                let target = doc(b).map()?;
                if doc(a).reaches(&target) {
                    return Err(Error::new(
                        Kind::ValueError,
                        "cyclic document mutation is unsupported",
                    ));
                }
                dict_set(ObjectOwner::as_ptr(&target), c, a)?;
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
