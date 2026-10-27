//! One Python attribute dictionary per local object and native thread.

use std::sync::{Arc, Weak};

use super::{Context, Error, Id, Result, Value};

pub(super) const CLASS: &str = "_thread._local";

thread_local! { static THREAD: Arc<()> = Arc::new(()); }

/// A thread owns its token. Once it exits, its dictionary may be collected.
#[derive(Clone, Default)]
pub(super) struct Dictionaries(Vec<(Weak<()>, Id)>);

impl Dictionaries {
    fn current(&self) -> Option<Id> {
        THREAD.with(|thread| {
            self.0
                .iter()
                .find(|(owner, _)| owner.as_ptr() == Arc::as_ptr(thread))
                .map(|(_, dict)| *dict)
        })
    }

    fn insert(&mut self, dict: Id) {
        self.0.retain(|(owner, _)| owner.strong_count() != 0);
        self.0.push((THREAD.with(Arc::downgrade), dict));
    }

    pub(super) fn live(&self) -> impl Iterator<Item = Id> {
        self.0
            .iter()
            .filter(|(owner, _)| owner.strong_count() != 0)
            .map(|(_, dict)| *dict)
    }
}

impl Context {
    fn local_dict(&mut self, obj: Id) -> Id {
        let Value::ThreadLocal(local) = self.heap.get(obj) else {
            unreachable!()
        };
        if let Some(dict) = local.current() {
            return dict;
        }
        let dict = self.dict(Vec::new());
        let Value::ThreadLocal(local) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        local.insert(dict);
        dict
    }

    pub(super) fn local_attribute(&mut self, obj: Id, name: &str) -> Result<Id> {
        if name == "__class__" {
            return Ok(self.builtin(CLASS));
        }
        let dict = self.local_dict(obj);
        if name == "__dict__" {
            return Ok(dict);
        }
        let key = self.text(name);
        match self.get_item(dict, key) {
            Err(error) if error.kind == "KeyError" => Err(self.missing_attribute(obj, name)),
            result => result,
        }
    }

    pub(super) fn update_local_attribute(
        &mut self,
        obj: Id,
        name: &str,
        value: Option<Id>,
    ) -> Result<()> {
        if name == "__dict__" {
            return Err(Error::new(
                "AttributeError",
                "'_thread._local' object attribute '__dict__' is read-only",
            ));
        }
        if name == "__class__" {
            let message = match value {
                None => "can't delete __class__ attribute".to_owned(),
                Some(value) if self.builtin_type_name(value) != Some("type") => {
                    format!(
                        "__class__ must be set to a class, not '{}' object",
                        self.type_name(value)
                    )
                }
                Some(_) => {
                    "__class__ assignment only supported for mutable types or ModuleType subclasses"
                        .to_owned()
                }
            };
            return Err(Error::new("TypeError", message));
        }
        let dict = self.local_dict(obj);
        if let Some(value) = value {
            let key = self.text(name);
            self.put_item(dict, key, value)
        } else {
            self.delete_dict_attribute(obj, dict, name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Dictionaries;
    use crate::dynamic::value::{Heap, Location, NONE, Value};
    use std::rc::Rc;
    use std::sync::{Barrier, Mutex};

    #[test]
    fn dictionaries_are_private_and_unrooted_after_thread_exit() {
        let mut heap = Heap::new();
        let mut dictionary = || {
            let payload: Rc<[Vec<String>]> = Rc::from([]);
            let probe = Rc::downgrade(&payload);
            let value = heap.alloc(Value::AttrGetter(payload));
            (heap.alloc(Value::Dict(vec![(NONE, value)])), probe)
        };
        let (parent, parent_probe) = dictionary();
        let (worker, worker_probe) = dictionary();
        let local = Mutex::new(Dictionaries::default());
        let ready = Barrier::new(2);
        local.lock().unwrap().insert(parent);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                {
                    let mut local = local.lock().unwrap();
                    assert_eq!(local.current(), None);
                    local.insert(worker);
                }
                ready.wait();
                assert_eq!(local.lock().unwrap().current(), Some(worker));
                ready.wait();
            });
            ready.wait();
            {
                let local = local.lock().unwrap();
                assert_eq!(local.current(), Some(parent));
                assert_eq!(local.live().collect::<Vec<_>>(), [parent, worker]);
            }
            ready.wait();
        });
        let object = heap.alloc(Value::ThreadLocal(local.into_inner().unwrap()));
        // Cross the ordinary statement collection threshold with only the local rooted.
        for _ in 0..4096 {
            heap.alloc(Value::None);
        }
        let location = Location {
            file: "local.py".into(),
            line: 1,
            span: None,
        };
        heap.statement(std::iter::once(object), &[], location, NONE);
        assert!(parent_probe.upgrade().is_some());
        assert!(worker_probe.upgrade().is_none());
    }
}
