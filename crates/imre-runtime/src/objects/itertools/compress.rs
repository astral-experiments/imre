//! Each selector is requested after its corresponding data item, including
//! the pull which discovers that there are no more selectors.

use super::*;

pub(in crate::objects) struct Compress {
    pub(super) data: Option<Rc<Object>>,
    pub(super) selectors: Option<Rc<Object>>,
}

impl Compress {
    /// Keep the pending data item on the current invocation, so reentry can
    /// consume a different pair. A selector error discards only that item.
    pub(super) fn advance(&self, frame: &mut PullFrame, item: Option<ItemResult>) -> Advance {
        if let Some(result) = item {
            let value = match result {
                Ok(Some(value)) => value,
                ended => return Advance::Return(ended),
            };
            match std::mem::take(frame) {
                PullFrame::Empty => {
                    *frame = PullFrame::CompressSelector(value);
                    return Advance::Pull(Rc::clone(self.selectors.as_ref().unwrap()));
                }
                PullFrame::CompressSelector(data) => {
                    if truth(&value) {
                        return Advance::Return(Ok(Some(data)));
                    }
                }
                _ => unreachable!(),
            }
        }
        Advance::Pull(Rc::clone(self.data.as_ref().unwrap()))
    }
}

fn truth(value: &OwnedWord) -> bool {
    let word = value.word;
    let kind = value.kind.raw();
    let nullable = kind & words::NULLABLE != 0;
    if nullable && word == 0 {
        return false;
    }
    // SAFETY: the owner roots the exact representation. The checker admits
    // only values with builtin truthiness; it never bypasses a Python __bool__.
    unsafe {
        match kind & !words::NULLABLE {
            0 => false,
            1 => word == if nullable { 2 } else { 1 },
            2 => word != 1,
            3 => crate::imre_str_truthy(word as *const Str) != 0,
            4 => (&*(word as *const List)).0.with(|items| !items.is_empty()),
            5 => object(word as *const Object).0.with(|state| match state {
                Storage::Sequence(seq) => seq.resolved_header() >> 1 != 0,
                Storage::Dictionary(_) => mappings::imre_dict_len(word as *const Object) != 1,
                Storage::Set(_) => mappings::imre_set_len(word as *const Object) != 1,
                // Other admitted object selectors are iterators or streams: their
                // truth does not inspect or advance the underlying source.
                _ => true,
            }),
            6 => records::descriptor(word as *const usize).length != 0,
            7 if nullable => *(word as *const f64) != 0.0,
            7 => f64::from_bits(word as u64) != 0.0,
            8 => bytes::imre_bytes_truthy(word as *const Bytes) != 0,
            9 => unions::imre_union_truthy(word) != 0,
            _ => unreachable!("checked selector"),
        }
    }
}

/// # Safety
/// Both source words are rooted and each mode describes the matching operand.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_compress(
    dm: u32,
    data: usize,
    sm: u32,
    selectors: usize,
) -> *const Object {
    // SAFETY: the caller has evaluated both operands before the first acquisition.
    let data = match unsafe { sources::try_acquire(dm, data) } {
        Ok(it) => it,
        Err(error) => return error.returning(std::ptr::null()),
    };
    // SAFETY: selectors are independently rooted even when the data source aliases them.
    let selectors = match unsafe { sources::try_acquire(sm, selectors) } {
        Ok(it) => it,
        Err(error) => return error.returning(std::ptr::null()),
    };
    Rc::into_raw(adapter(Adapter::Compress(Compress {
        data: Some(data),
        selectors: Some(selectors),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repeat(value: i64, times: i64) -> Rc<Object> {
        // SAFETY: both integers are immediate and the new cursor transfers an owner.
        unsafe {
            Rc::from_raw(imre_repeat(
                2,
                Int::small(value).unwrap().0,
                Int::small(times).unwrap().0,
            ))
        }
    }

    fn filter(data: &Rc<Object>, selectors: &Rc<Object>) -> Rc<Object> {
        // SAFETY: mode zero shares each rooted cursor in its current position.
        unsafe {
            Rc::from_raw(imre_compress(
                0,
                Rc::as_ptr(data) as usize,
                0,
                Rc::as_ptr(selectors) as usize,
            ))
        }
    }

    #[test]
    fn reentry_before_the_selector_preserves_both_data_items() {
        // SAFETY: the progression acquires its immediate integer arguments.
        let data = unsafe {
            Rc::from_raw(sources::imre_progression(
                Int::small(0).unwrap().0,
                0,
                Int::small(1).unwrap().0,
            ))
        };
        let selectors = repeat(1, 2);
        let cursor = filter(&data, &selectors);
        // SAFETY: the constructor installed a compress adapter; only its owner escapes.
        let state = unsafe {
            cursor.0.with(|s| match s {
                Storage::IteratorAdapter(a) => Rc::clone(a),
                _ => unreachable!(),
            })
        };
        let mut frame = PullFrame::Empty;
        let Advance::Pull(data) = state.advance(&mut frame, None) else {
            panic!("expected data");
        };
        let Advance::Pull(selector) = state.advance(&mut frame, Some(next_item(&data))) else {
            panic!("expected selector");
        };
        assert!(
            matches!(next_item(&cursor), Ok(Some(item)) if item.word == Int::small(1).unwrap().0)
        );
        assert!(
            matches!(state.advance(&mut frame, Some(next_item(&selector))), Advance::Return(Ok(Some(item))) if item.word == Int::small(0).unwrap().0)
        );
        // The exhausted selector still costs data item 2, even though nothing is returned.
        assert!(matches!(next_item(&cursor), Ok(None)));
        assert!(
            matches!(next_item(&data), Ok(Some(item)) if item.word == Int::small(3).unwrap().0)
        );
    }

    #[test]
    fn deep_selector_pulls_preserve_the_shared_data_cursor() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let n = 16_384;
                let data = repeat(1, n + 2);
                let mut top = repeat(1, 1);
                for _ in 0..n {
                    top = filter(&data, &top);
                }
                assert!(matches!(next_item(&top), Ok(Some(_))));
                drop(top);
                assert_eq!(Rc::strong_count(&data), 1);
                assert!(matches!(next_item(&data), Ok(Some(_))));
                assert!(matches!(next_item(&data), Ok(Some(_))));
                assert!(matches!(next_item(&data), Ok(None)));
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
