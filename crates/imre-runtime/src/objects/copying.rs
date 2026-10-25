//! Copy supported container graphs by identity. A source owner stays live until
//! its entire copy is complete, so addresses in the memo cannot be reused.
use std::collections::HashMap;

use super::records::{allocate_record, descriptor};
use super::words::WordKind;
use super::{Error, List, OwnedWord, Storage, Words};

struct Graph {
    memo: HashMap<usize, OwnedWord>,
    pending: Vec<(crate::ObjectOwner, crate::ObjectOwner)>,
}

fn push<T>(values: &mut Vec<T>, value: T) -> Result<(), Error> {
    values.try_reserve(1).map_err(|_| Error::allocation())?;
    values.push(value);
    Ok(())
}

impl Graph {
    /// An empty object stands for a mutable result until all edges are ready.
    /// Tuple contents can immediately point to it without descending back through it.
    fn copy(&mut self, source: OwnedWord) -> Result<OwnedWord, Error> {
        enum Task {
            Visit(OwnedWord),
            Tuple(OwnedWord, usize),
        }
        let mut work = vec![Task::Visit(source)];
        let mut results = Vec::<OwnedWord>::new();
        while let Some(task) = work.pop() {
            let source = match task {
                Task::Visit(source) => source,
                Task::Tuple(source, len) => {
                    let start = results.len() - len;
                    let record = source.word as *const usize;
                    // SAFETY: the rooted plain tuple has len initialized fields.
                    let metadata = unsafe { descriptor(record) };
                    let changed = results[start..].iter().enumerate().any(|(i, value)| {
                        // SAFETY: the output and input field have this exact kind.
                        value.word != unsafe { value.kind.read_record_field(record.add(i)) }
                    });
                    if !changed {
                        results.truncate(start);
                        // This memo is private: remembering an unchanged tuple
                        // avoids exponential rescans of a shared immutable DAG.
                        self.memo.try_reserve(1).map_err(|_| Error::allocation())?;
                        self.memo.insert(source.word, source.clone());
                        push(&mut results, source)?;
                        continue;
                    }
                    // SAFETY: the same immutable descriptor outlives both tuples.
                    let output = unsafe { allocate_record(metadata) };
                    if output.is_null() {
                        return Err(Error::allocation());
                    }
                    for (i, value) in results.drain(start..).enumerate() {
                        // SAFETY: fresh, aligned slots have the original field kinds.
                        unsafe { value.initialize_record_field(output.add(i)) };
                    }
                    self.remember(&mut results, source, output as usize)?;
                    continue;
                }
            };
            let kind = source.kind.base();
            if source.word == 0 || !matches!(kind, 4..=6) {
                push(&mut results, source)?;
                continue;
            }
            if let Some(previous) = self.memo.get(&source.word) {
                // SAFETY: a memo hit roots the same result, even when one view
                // uses a nullable rather than a nonnullable reference descriptor.
                push(&mut results, unsafe { source.kind.acquire(previous.word) })?;
                continue;
            }
            let copy = match kind {
                4 => {
                    // SAFETY: kind 4 identifies an integer list, whose elements are atomic.
                    let list = unsafe { &*(source.word as *const List) };
                    // SAFETY: the local owner roots this list; cloning integers cannot reenter.
                    let values = unsafe {
                        list.0.with(|values| {
                            let mut output = Vec::new();
                            output
                                .try_reserve_exact(values.len())
                                .map_err(|_| Error::allocation())?;
                            output.extend(values.iter().cloned());
                            Ok::<_, Error>(output)
                        })
                    }?;
                    crate::new_list(values) as usize
                }
                5 => {
                    // Do not inspect the source cell here; copy() may have been
                    // called while the very same container is being traversed.
                    let output = Words::new(0).into_object();
                    // SAFETY: kind 5 roots an Object for the pending traversal.
                    let input = unsafe { crate::ObjectOwner::acquire_word(source.word) };
                    push(&mut self.pending, (input, output.clone()))?;
                    crate::ObjectOwner::into_raw(output) as usize
                }
                6 => {
                    let record = source.word as *const usize;
                    // SAFETY: admission allows only scalar structs and plain tuples.
                    let metadata = unsafe { descriptor(record) };
                    if metadata.is_value_struct() {
                        push(&mut results, source)?;
                        continue;
                    }
                    let len = metadata.length;
                    work.try_reserve(len.checked_add(1).ok_or_else(Error::allocation)?)
                        .map_err(|_| Error::allocation())?;
                    work.push(Task::Tuple(source, len));
                    for i in (0..len).rev() {
                        // SAFETY: Task::Tuple holds the source throughout this traversal.
                        let field = unsafe {
                            let kind = WordKind::new(metadata.kinds.add(i).read());
                            kind.acquire(kind.read_record_field(record.add(i)))
                        };
                        work.push(Task::Visit(field));
                    }
                    continue;
                }
                _ => unreachable!(),
            };
            self.remember(&mut results, source, copy)?;
        }
        Ok(results.pop().expect("one result for the input word"))
    }

    fn remember(
        &mut self,
        results: &mut Vec<OwnedWord>,
        source: OwnedWord,
        copy: usize,
    ) -> Result<(), Error> {
        // SAFETY: the allocation transfers exactly one owner of this source kind.
        let result = unsafe { OwnedWord::from_owned(copy, source.kind) };
        self.memo.try_reserve(1).map_err(|_| Error::allocation())?;
        self.memo.insert(source.word, result.clone());
        push(results, result)
    }

    fn run(mut self, source: OwnedWord) -> Result<OwnedWord, Error> {
        let result = self.copy(source.clone())?;
        let mut prepared = Vec::new();
        while let Some((source, output)) = self.pending.pop() {
            // SAFETY: copy() only queues generic objects; it never borrows their
            // cells. Only copied fields, owned words and private storage escape.
            let contents = unsafe {
                source.0.with(|storage| match storage {
                    Storage::Sequence(words) => {
                        if let Some(rows) = words.rows() {
                            return Ok(Storage::Sequence(rows.repeat(1)?));
                        }
                        let words = words.resolved();
                        let mut out = Words::new(words.kind.raw());
                        out.values
                            .try_reserve_exact(words.values.len())
                            .map_err(|_| Error::allocation())?;
                        for &word in &words.values {
                            out.values
                                .push(self.copy(words.kind.acquire(word))?.into_word());
                        }
                        out.update_header();
                        Ok(Storage::Sequence(out))
                    }
                    Storage::Dictionary(map) => {
                        Ok(Storage::Dictionary(super::mappings::TableOwner::new(
                            map.copy_values(|value| self.copy(value))?,
                        )))
                    }
                    Storage::Set(set) => Ok(Storage::Set(super::mappings::TableOwner::new(
                        set.copy_keys()?,
                    ))),
                    _ => unreachable!("deep-copy admission requires an exact builtin container"),
                })
            }?;
            push(&mut prepared, (output, contents))?;
        }
        // Publish edges only after every fallible allocation succeeded. On error
        // all shells are still empty, so even a partly staged cycle cleans up.
        for (output, contents) in prepared {
            // SAFETY: the shell has not escaped and its old storage is empty.
            unsafe { output.0.with_mut(|slot| *slot = contents) };
        }
        Ok(result)
    }
}

/// Deep copies the statically admitted container graph with one private memo.
///
/// # Safety
/// The word is a live value of the supplied representation. Generic objects
/// must be closed lists, mappings or sets; records must be plain tuples or scalar structs.
/// Dictionary and set keys must be recursively immutable under deepcopy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deep_copy(source: usize, kind: u32) -> usize {
    // SAFETY: the caller has checked both the root and every reachable type.
    let input = unsafe { WordKind::new(kind).acquire(source) };
    let graph = Graph {
        memo: HashMap::new(),
        pending: Vec::new(),
    };
    super::finish(graph.run(input).map(OwnedWord::into_word), 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Int;
    use crate::objects::Object;
    use crate::objects::mappings::{imre_dict_delete, imre_dict_get, imre_dict_new, imre_dict_set};
    use crate::objects::records::{
        RecordDescriptor, imre_record_new, imre_record_release, imre_record_retain,
    };
    use crate::objects::{imre_seq_append, imre_seq_get_index, imre_seq_new, imre_seq_pop};

    #[test]
    fn list_dict_cycle_keeps_aliases_and_targets_the_copy() {
        // Source programs cannot currently express these recursive storage types.
        // SAFETY: every edge has the declared object kind. We keep both roots
        // and explicitly break the cycles before their final owners drop.
        unsafe {
            let input = crate::ObjectOwner::from_raw(imre_seq_new(5));
            let map = crate::ObjectOwner::from_raw(imre_dict_new(2, 5));
            let key = Int::small(1).unwrap().0;
            imre_seq_append(
                crate::ObjectOwner::as_ptr(&input),
                crate::ObjectOwner::as_ptr(&map) as usize,
                5,
            );
            imre_seq_append(
                crate::ObjectOwner::as_ptr(&input),
                crate::ObjectOwner::as_ptr(&map) as usize,
                5,
            );
            imre_dict_set(
                crate::ObjectOwner::as_ptr(&map),
                key,
                crate::ObjectOwner::as_ptr(&input) as usize,
            );
            let copy = crate::ObjectOwner::from_raw(imre_deep_copy(
                crate::ObjectOwner::as_ptr(&input) as usize,
                5,
            ) as *const Object);
            let first = crate::ObjectOwner::from_raw(imre_seq_get_index(
                crate::ObjectOwner::as_ptr(&copy),
                0,
            ) as *const Object);
            let second = crate::ObjectOwner::from_raw(imre_seq_get_index(
                crate::ObjectOwner::as_ptr(&copy),
                1,
            ) as *const Object);
            let back = crate::ObjectOwner::from_raw(imre_dict_get(
                crate::ObjectOwner::as_ptr(&first),
                key,
            ) as *const Object);
            assert!(!crate::ObjectOwner::ptr_eq(&copy, &input));
            assert!(!crate::ObjectOwner::ptr_eq(&first, &map));
            assert!(crate::ObjectOwner::ptr_eq(&first, &second));
            assert!(crate::ObjectOwner::ptr_eq(&copy, &back));
            imre_dict_delete(crate::ObjectOwner::as_ptr(&first), key);
            imre_dict_delete(crate::ObjectOwner::as_ptr(&map), key);
            drop(back);
            assert_eq!(crate::ObjectOwner::strong_count(&copy), 1);
            assert_eq!(crate::ObjectOwner::strong_count(&input), 1);
        }
    }

    #[test]
    fn tuple_cycle_reuses_the_new_tuple_for_the_back_edge() {
        let kinds = [5];
        let descriptor = RecordDescriptor {
            length: 1,
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        // SAFETY: the descriptor and roots outlive both explicitly broken cycles.
        unsafe {
            let list = crate::ObjectOwner::from_raw(imre_seq_new(6));
            let tuple = imre_record_new(&descriptor);
            tuple
                .cast::<*const Object>()
                .write(crate::ObjectOwner::into_raw(list.clone()));
            imre_seq_append(crate::ObjectOwner::as_ptr(&list), tuple as usize, 6);
            let copied_tuple = imre_deep_copy(tuple as usize, 6) as *const usize;
            assert_ne!(copied_tuple, tuple);
            let copied_list = copied_tuple.cast::<*const Object>().read();
            assert_ne!(copied_list, crate::ObjectOwner::as_ptr(&list));
            let back = imre_seq_pop(copied_list) as *const usize;
            assert_eq!(back, copied_tuple);
            imre_record_release(back);
            imre_record_release(copied_tuple);
            imre_record_release(imre_seq_pop(crate::ObjectOwner::as_ptr(&list)) as *const usize);
            imre_record_release(tuple);
            assert_eq!(crate::ObjectOwner::strong_count(&list), 1);
        }
    }

    #[test]
    fn deeply_nested_immutable_tuples_keep_the_original_identity() {
        let kinds = [6, 6];
        let parent = RecordDescriptor {
            length: 2,
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        let empty = RecordDescriptor {
            length: 0,
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        // SAFETY: the two layout descriptors outlive all records; each field owns
        // a separately retained child, including where both pointers are equal.
        unsafe {
            let mut root = imre_record_new(&empty);
            for _ in 0..20_000 {
                let wrapper = imre_record_new(&parent);
                wrapper.cast::<*const usize>().write(root);
                imre_record_retain(root);
                wrapper.add(1).cast::<*const usize>().write(root);
                root = wrapper;
            }
            let copy = imre_deep_copy(root as usize, 6) as *const usize;
            assert_eq!(root.cast_const(), copy);
            imre_record_release(copy);
            imre_record_release(root);
        }
    }
}
