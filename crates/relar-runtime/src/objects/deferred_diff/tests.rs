use super::*;
use crate::objects::values::relar_sequence_equal;
use crate::objects::{
    relar_seq_append, relar_seq_borrow_index_proven, relar_seq_data_readonly, relar_seq_len,
};
use std::ptr;

fn string(value: &str) -> Rc<Str> {
    Rc::new(Str::new(value))
}

fn list(values: &[&str]) -> Rc<Object> {
    let mut words = Words::new(3);
    for &s in values {
        words.values.push(Rc::into_raw(string(s)) as usize);
    }
    words.into_object()
}

fn rows(values: &Rc<Object>) -> Vec<String> {
    // SAFETY: all test sequence elements are rooted strings.
    unsafe {
        values.0.with(|s| {
            s.sequence()
                .values
                .iter()
                .map(|&word| (&*(word as *const Str)).to_string())
                .collect()
        })
    }
}

fn filter_rows(
    list: &Rc<Object>,
    a: &str,
    b: Option<&str>,
    exclude: Option<&str>,
    strip: u32,
) -> Rc<Object> {
    let (first, second, exclude) = (string(a), b.map(string), exclude.map(string));
    // SAFETY: each non-null argument remains rooted and the result transfers an owner.
    unsafe {
        Rc::from_raw(relar_str_list_filter(
            Rc::as_ptr(list),
            Rc::as_ptr(&first),
            second.as_ref().map_or(ptr::null(), Rc::as_ptr),
            exclude.as_ref().map_or(ptr::null(), Rc::as_ptr),
            strip,
        ))
    }
}

#[test]
fn filters_and_suffix_sort_keep_raw_guides_lazy() {
    let left = list(&["anchor", "same text abc", "same text def"]);
    let right = list(&["anchor", "same text abX", "same text deX"]);
    // SAFETY: functions read rooted exact lists and transfer all returned owners.
    unsafe {
        let diff = Rc::from_raw(relar_ndiff_deferred(Rc::as_ptr(&left), Rc::as_ptr(&right)));
        let copied = Rc::from_raw(relar_str_list_copy(Rc::as_ptr(&diff)));
        let signed = filter_rows(&copied, "-", Some("+"), None, 0);
        assert_eq!(relar_str_list_nonempty(Rc::as_ptr(&diff)), 1);
        assert_eq!(relar_str_list_nonempty(Rc::as_ptr(&signed)), 1);
        assert_eq!(
            rows(&filter_rows(&diff, "-", None, Some("- same text d"), 2)),
            ["same text abc"]
        );
        let sorted = Rc::from_raw(relar_str_list_sorted_suffix(Rc::as_ptr(&signed), 2));
        assert_eq!(
            rows(&sorted),
            [
                "+ same text abX",
                "- same text abc",
                "+ same text deX",
                "- same text def"
            ]
        );
        diff.0.with(|s| {
            assert!(
                s.sequence_storage()
                    .plan()
                    .unwrap()
                    .source
                    .full
                    .get()
                    .is_none()
            )
        });
        assert_eq!(
            rows(&signed),
            [
                "- same text abc",
                "+ same text abX",
                "- same text def",
                "+ same text deX"
            ]
        );
        assert_eq!(
            rows(&filter_rows(&diff, "?", None, None, 0)),
            vec!["?             ^\n"; 4]
        );
        assert_eq!(rows(&diff)[0], "  anchor");
        assert_eq!(
            relar_sequence_equal(Rc::as_ptr(&diff), Rc::as_ptr(&copied)),
            1
        );
    }
}

#[test]
fn raw_reads_and_writes_respect_snapshots_and_copy_identity() {
    let input = list(&["same"]);
    let extra = string("later");
    // SAFETY: mutations finish before subsequent reads of the rooted buffers.
    // Stored words and the readonly pointer remain rooted until their last use.
    unsafe {
        let diff = Rc::from_raw(relar_ndiff_deferred(Rc::as_ptr(&input), Rc::as_ptr(&input)));
        let copy = Rc::from_raw(relar_str_list_copy(Rc::as_ptr(&diff)));
        let alias = Rc::clone(&diff);
        let signed = filter_rows(&diff, "+ ", Some("- "), None, 0);
        assert_eq!(relar_str_list_nonempty(Rc::as_ptr(&signed)), 0);
        relar_seq_append(Rc::as_ptr(&input), Rc::as_ptr(&extra) as usize, 3);
        drop(input);
        assert_eq!(relar_seq_resolve_header(Rc::as_ptr(&diff)), 2);
        assert_eq!(relar_seq_len(Rc::as_ptr(&diff)), 3);
        let word = relar_seq_borrow_index_proven(Rc::as_ptr(&diff), 0);
        assert_eq!(relar_seq_data_readonly(Rc::as_ptr(&diff)).read(), word);
        assert_eq!((&*(word as *const Str)).as_str(), "  same");
        relar_seq_append(Rc::as_ptr(&alias), Rc::as_ptr(&extra) as usize, 3);
        diff.0
            .with(|s| assert!(s.sequence_storage().plan().is_none()));
        assert_eq!(rows(&alias), ["  same", "later"]);
        assert_eq!(rows(&copy), ["  same"]);
        assert_eq!(
            relar_sequence_equal(Rc::as_ptr(&diff), Rc::as_ptr(&copy)),
            0
        );
        relar_seq_append(Rc::as_ptr(&signed), Rc::as_ptr(&extra) as usize, 3);
        assert_eq!(rows(&signed), ["later"]);
        assert_eq!(rows(&copy), ["  same"]);
    }
}

#[test]
fn ordinary_filters_and_sort_slice_by_unicode_code_points() {
    let plain = list(&["🐍éz", "ßab", "abz", "Øäa", "🐍", ""]);
    assert_eq!(
        rows(&filter_rows(&plain, "🐍", Some("ab"), None, 2)),
        ["z", "z", ""]
    );
    // SAFETY: the ordinary string list roots all elements through the sort.
    let sorted = unsafe { Rc::from_raw(relar_str_list_sorted_suffix(Rc::as_ptr(&plain), 2)) };
    assert_eq!(rows(&sorted), ["🐍", "", "Øäa", "ßab", "🐍éz", "abz"]);
}

#[test]
fn shared_strings_preserve_duplicate_counts_order_and_slice_ownership() {
    let left = list(&["b", "b", "common", "z"]);
    let right = list(&["a", "a", "common", "z"]);
    // SAFETY: all accessed handles remain rooted, including the retained
    // untagged output strings after both the inputs and their diff are dropped.
    unsafe {
        let diff = Rc::from_raw(relar_ndiff_deferred(Rc::as_ptr(&left), Rc::as_ptr(&right)));
        let removed = filter_rows(&diff, "- b", None, None, 2);
        let added = filter_rows(&diff, "+", None, None, 2);
        diff.0.with(|storage| {
            let source = &storage.sequence_storage().plan().unwrap().source;
            assert_eq!(source.left.values[2..], source.right.values[2..]);
            removed
                .0
                .with(|s| assert_eq!(s.sequence().values[..], source.left.values[..2]));
            added
                .0
                .with(|s| assert_eq!(s.sequence().values[..], source.right.values[..2]));
        });
        drop(diff);
        drop(left);
        drop(right);
        assert_eq!(rows(&added), ["a", "a"]);
        assert_eq!(rows(&removed), ["b", "b"]);
    }
    // Unsorted inputs are only shared on exact comparisons. Each side can
    // also skip unrelated and repeated payloads when finding a filtered row.
    let left = list(&["z", "a", "a", "a", "y"]);
    let right = list(&["a", "x", "a", "w"]);
    // SAFETY: both sequences independently own their exact strings.
    let diff = unsafe { Rc::from_raw(relar_ndiff_deferred(Rc::as_ptr(&left), Rc::as_ptr(&right))) };
    assert_eq!(rows(&filter_rows(&diff, "+", None, Some("+ x"), 2)), ["w"]);
    assert_eq!(
        rows(&filter_rows(&diff, "-", None, Some("- z"), 2)),
        ["a", "y"]
    );
}
