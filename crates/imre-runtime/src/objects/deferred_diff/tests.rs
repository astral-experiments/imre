use super::*;
use crate::StrOwner;
use crate::objects::values::imre_sequence_equal;
use crate::objects::{
    imre_seq_append, imre_seq_borrow_index_proven, imre_seq_data_readonly, imre_seq_len,
};
use std::ptr;

fn string(value: &str) -> StrOwner {
    StrOwner::new(Str::new(value))
}

fn list(values: &[&str]) -> crate::ObjectOwner {
    let mut words = Words::new(3);
    for &s in values {
        words.values.push(StrOwner::into_raw(string(s)) as usize);
    }
    words.into_object()
}

fn rows(values: &crate::ObjectOwner) -> Vec<String> {
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
    list: &crate::ObjectOwner,
    a: &str,
    b: Option<&str>,
    exclude: Option<&str>,
    strip: u32,
) -> crate::ObjectOwner {
    let (first, second, exclude) = (string(a), b.map(string), exclude.map(string));
    // SAFETY: each non-null argument remains rooted and the result transfers an owner.
    unsafe {
        crate::ObjectOwner::from_raw(imre_str_list_filter(
            crate::ObjectOwner::as_ptr(list),
            StrOwner::as_ptr(&first),
            second.as_ref().map_or(ptr::null(), StrOwner::as_ptr),
            exclude.as_ref().map_or(ptr::null(), StrOwner::as_ptr),
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
        let diff = crate::ObjectOwner::from_raw(imre_ndiff_deferred(
            crate::ObjectOwner::as_ptr(&left),
            crate::ObjectOwner::as_ptr(&right),
        ));
        let copied =
            crate::ObjectOwner::from_raw(imre_str_list_copy(crate::ObjectOwner::as_ptr(&diff)));
        let signed = filter_rows(&copied, "-", Some("+"), None, 0);
        assert_eq!(imre_str_list_nonempty(crate::ObjectOwner::as_ptr(&diff)), 1);
        assert_eq!(
            imre_str_list_nonempty(crate::ObjectOwner::as_ptr(&signed)),
            1
        );
        assert_eq!(
            rows(&filter_rows(&diff, "-", None, Some("- same text d"), 2)),
            ["same text abc"]
        );
        let sorted = crate::ObjectOwner::from_raw(imre_str_list_sorted_suffix(
            crate::ObjectOwner::as_ptr(&signed),
            2,
        ));
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
            let Some(Deferred(View::Full(source))) = s.sequence_storage().plan() else {
                unreachable!()
            };
            assert!(source.full.get().is_none());
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
            imre_sequence_equal(
                crate::ObjectOwner::as_ptr(&diff),
                crate::ObjectOwner::as_ptr(&copied)
            ),
            1
        );
    }
}

#[test]
fn sparse_projection_releases_inputs_and_restores_raw_guides() {
    let mut common = vec![
        "".to_owned(),
        "".to_owned(),
        "café".to_owned(),
        "cafê".to_owned(),
    ];
    common.extend((10000..11100).map(|n| format!("row {n} 雪\0")));
    common.push("x".repeat(300));
    let mut left: Vec<_> = common.iter().map(String::as_str).collect();
    let mut right = left.clone();
    left.extend(["z abcdefgh", "z duplicate", "z duplicate"]);
    right.extend(["z abcdEfgh", "z duplicate"]);
    let (left, right) = (list(&left), list(&right));
    // SAFETY: both input lists are rooted for each independent snapshot.
    let make_diff = || unsafe {
        crate::ObjectOwner::from_raw(imre_ndiff_deferred(
            crate::ObjectOwner::as_ptr(&left),
            crate::ObjectOwner::as_ptr(&right),
        ))
    };
    let diff = make_diff();
    let expected = rows(&make_diff());
    // SAFETY: the rooted diff cannot mutate during this inspection.
    let source = unsafe {
        diff.0.with(|storage| {
            let Some(Deferred(View::Full(source))) = storage.sequence_storage().plan() else {
                unreachable!()
            };
            Rc::clone(source)
        })
    };
    assert!(matches!(*source.inputs.borrow(), Inputs::Original(..)));
    assert_eq!(
        rows(&filter_rows(&diff, "+", None, None, 2)),
        ["z abcdEfgh"]
    );
    {
        let inputs = source.inputs.borrow();
        let Inputs::Packed(packed) = &*inputs else {
            panic!("sparse projection should pack")
        };
        let (left, right) = packed.unpack();
        let last = left.values.len() - 1;
        assert_eq!(left.values[last - 1], left.values[last]);
        assert_eq!(left.values[last], *right.values.last().unwrap());
    }
    assert!(source.full.get().is_none());
    drop((left, right));
    assert_eq!(rows(&diff), expected);
    assert!(matches!(*source.inputs.borrow(), Inputs::Materialized));
    assert_eq!(
        rows(&filter_rows(&diff, "-", None, None, 2)),
        ["z abcdefgh", "z duplicate"]
    );
}

#[test]
fn raw_reads_and_writes_respect_snapshots_and_copy_identity() {
    let input = list(&["same"]);
    let extra = string("later");
    // SAFETY: mutations finish before subsequent reads of the rooted buffers.
    // Stored words and the readonly pointer remain rooted until their last use.
    unsafe {
        let diff = crate::ObjectOwner::from_raw(imre_ndiff_deferred(
            crate::ObjectOwner::as_ptr(&input),
            crate::ObjectOwner::as_ptr(&input),
        ));
        let copy =
            crate::ObjectOwner::from_raw(imre_str_list_copy(crate::ObjectOwner::as_ptr(&diff)));
        let alias = crate::ObjectOwner::clone(&diff);
        let signed = filter_rows(&diff, "+ ", Some("- "), None, 0);
        assert_eq!(
            imre_str_list_nonempty(crate::ObjectOwner::as_ptr(&signed)),
            0
        );
        imre_seq_append(
            crate::ObjectOwner::as_ptr(&input),
            StrOwner::as_ptr(&extra) as usize,
            3,
        );
        drop(input);
        assert_eq!(
            imre_seq_resolve_header(crate::ObjectOwner::as_ptr(&diff)),
            2
        );
        assert_eq!(imre_seq_len(crate::ObjectOwner::as_ptr(&diff)), 3);
        let word = imre_seq_borrow_index_proven(crate::ObjectOwner::as_ptr(&diff), 0);
        assert_eq!(
            imre_seq_data_readonly(crate::ObjectOwner::as_ptr(&diff)).read(),
            word
        );
        assert_eq!((&*(word as *const Str)).as_str(), "  same");
        imre_seq_append(
            crate::ObjectOwner::as_ptr(&alias),
            StrOwner::as_ptr(&extra) as usize,
            3,
        );
        diff.0
            .with(|s| assert!(s.sequence_storage().plan().is_none()));
        assert_eq!(rows(&alias), ["  same", "later"]);
        assert_eq!(rows(&copy), ["  same"]);
        assert_eq!(
            imre_sequence_equal(
                crate::ObjectOwner::as_ptr(&diff),
                crate::ObjectOwner::as_ptr(&copy)
            ),
            0
        );
        imre_seq_append(
            crate::ObjectOwner::as_ptr(&signed),
            StrOwner::as_ptr(&extra) as usize,
            3,
        );
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
    let sorted = unsafe {
        crate::ObjectOwner::from_raw(imre_str_list_sorted_suffix(
            crate::ObjectOwner::as_ptr(&plain),
            2,
        ))
    };
    assert_eq!(rows(&sorted), ["🐍", "", "Øäa", "ßab", "🐍éz", "abz"]);
}

#[test]
fn suffix_sort_is_stable_beyond_the_stack_scratch_size() {
    let values = (0..1025)
        .map(|index| format!("{index:04}:{}", index % 7))
        .collect::<Vec<_>>();
    let borrowed = values
        .iter()
        .map(|value| value.as_str())
        .collect::<Vec<_>>();
    let plain = list(&borrowed);
    // SAFETY: the ordinary string list roots all elements through the sort.
    let sorted = unsafe {
        crate::ObjectOwner::from_raw(imre_str_list_sorted_suffix(
            crate::ObjectOwner::as_ptr(&plain),
            5,
        ))
    };
    let expected = (0..7)
        .flat_map(|remainder| {
            (remainder..1025)
                .step_by(7)
                .map(move |index| format!("{index:04}:{remainder}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(rows(&sorted), expected);
}

#[test]
fn shared_strings_preserve_duplicate_counts_order_and_slice_ownership() {
    let left = list(&["b", "b", "common", "z"]);
    let right = list(&["a", "a", "common", "z"]);
    // SAFETY: all accessed handles remain rooted, including the retained
    // untagged output strings after both the inputs and their diff are dropped.
    unsafe {
        let diff = crate::ObjectOwner::from_raw(imre_ndiff_deferred(
            crate::ObjectOwner::as_ptr(&left),
            crate::ObjectOwner::as_ptr(&right),
        ));
        let removed = filter_rows(&diff, "- b", None, None, 2);
        let added = filter_rows(&diff, "+", None, None, 2);
        diff.0.with(|storage| {
            let Some(Deferred(View::Full(source))) = storage.sequence_storage().plan() else {
                unreachable!()
            };
            let (left, right) = (source.side("-").unwrap(), source.side("+").unwrap());
            assert_eq!(left.values[2..], right.values[2..]);
            removed
                .0
                .with(|s| assert_eq!(s.sequence().values[..], left.values[..2]));
            added
                .0
                .with(|s| assert_eq!(s.sequence().values[..], right.values[..2]));
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
    let diff = unsafe {
        crate::ObjectOwner::from_raw(imre_ndiff_deferred(
            crate::ObjectOwner::as_ptr(&left),
            crate::ObjectOwner::as_ptr(&right),
        ))
    };
    assert_eq!(rows(&filter_rows(&diff, "+", None, Some("+ x"), 2)), ["w"]);
    assert_eq!(
        rows(&filter_rows(&diff, "-", None, Some("- z"), 2)),
        ["a", "y"]
    );
}

#[test]
fn sorted_rows_reuse_only_live_ordered_sources_and_detach_on_write() {
    let left = list(&["c", "c", "unchanged"]);
    let right = list(&["b", "a", "unchanged"]);
    let edit = string("- edited");
    // SAFETY: all handles are rooted for each call and every returned reference
    // is adopted once. The weak source is observed only through a strong owner.
    unsafe {
        let diff = crate::ObjectOwner::from_raw(imre_ndiff_deferred(
            crate::ObjectOwner::as_ptr(&left),
            crate::ObjectOwner::as_ptr(&right),
        ));
        let signed = filter_rows(&diff, "+", Some("-"), None, 0);
        let sorted = crate::ObjectOwner::from_raw(imre_str_list_sorted_suffix(
            crate::ObjectOwner::as_ptr(&signed),
            2,
        ));
        let copy =
            crate::ObjectOwner::from_raw(imre_str_list_copy(crate::ObjectOwner::as_ptr(&sorted)));
        let removed = filter_rows(&copy, "-", None, None, 2);
        let added = filter_rows(&copy, "+", None, None, 2);
        assert_eq!(rows(&removed), ["c", "c"]);
        assert_eq!(rows(&added), ["a", "b"]);
        let weak = sorted.0.with(|s| {
            let Some(Deferred(View::Sorted { origin, .. })) = s.sequence_storage().plan() else {
                unreachable!()
            };
            let source = origin.upgrade().unwrap();
            removed.0.with(|s| {
                assert_eq!(
                    s.sequence().values[..],
                    source.side("-").unwrap().values[..2]
                )
            });
            Weak::clone(origin)
        });
        imre_seq_append(
            crate::ObjectOwner::as_ptr(&sorted),
            StrOwner::as_ptr(&edit) as usize,
            3,
        );
        assert_eq!(
            rows(&filter_rows(&sorted, "-", None, None, 2)),
            ["c", "c", "edited"]
        );
        drop(diff);
        drop(signed);
        assert!(weak.upgrade().is_none());
        assert_eq!(rows(&filter_rows(&copy, "-", None, None, 2)), ["c", "c"]);
        assert_eq!(rows(&copy), ["+ a", "+ b", "- c", "- c"]);
    }
}

#[test]
fn split_ranges_preserve_unicode_lines_filters_and_independent_mutation() {
    use crate::objects::{imre_seq_get_index, imre_str_splitlines, stdlib::imre_str_join};

    let source = StrOwner::new(Str::new(
        "+🦀hello\u{2028}+++excluded\r\n-β\u{85}plain\n".repeat(64),
    ));
    // SAFETY: the input, separator and lists root every handle; each owned
    // result is taken exactly once and no storage borrow spans a mutation.
    unsafe {
        let split = crate::ObjectOwner::from_raw(imre_str_splitlines(StrOwner::as_ptr(&source)));
        let copy =
            crate::ObjectOwner::from_raw(imre_str_list_copy(crate::ObjectOwner::as_ptr(&split)));
        let added = filter_rows(&split, "+", None, Some("+++"), 2);
        let separator = Str::new("|");
        let joined = StrOwner::from_raw(imre_str_join(
            &separator,
            crate::ObjectOwner::as_ptr(&added),
        ));
        assert_eq!(joined.as_str(), vec!["hello"; 64].join("|"));
        assert_eq!(
            imre_seq_len(crate::ObjectOwner::as_ptr(&split)),
            2 * 256 + 1
        );
        let original = StrOwner::from_raw(
            imre_seq_get_index(crate::ObjectOwner::as_ptr(&copy), 0) as *const Str
        );
        assert_eq!(original.as_str(), "+🦀hello");
        let extra = StrOwner::new(Str::new("detached"));
        imre_seq_append(
            crate::ObjectOwner::as_ptr(&split),
            StrOwner::as_ptr(&extra) as usize,
            3,
        );
        assert_eq!(
            imre_seq_len(crate::ObjectOwner::as_ptr(&split)),
            2 * 257 + 1
        );
        assert_eq!(imre_seq_len(crate::ObjectOwner::as_ptr(&copy)), 2 * 256 + 1);
        assert_eq!(imre_seq_len(crate::ObjectOwner::as_ptr(&added)), 2 * 64 + 1);
    }
}
