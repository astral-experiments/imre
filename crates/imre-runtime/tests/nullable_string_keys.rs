use std::ffi::c_void;

unsafe extern "C" {
    fn imre_dict_new(key_kind: u32, value_kind: u32) -> *const c_void;
    fn imre_dict_set(dictionary: *const c_void, key: usize, value: usize);
    fn imre_dict_get(dictionary: *const c_void, key: usize) -> usize;
    fn imre_dict_contains(dictionary: *const c_void, key: usize) -> u32;
    fn imre_dict_iter(dictionary: *const c_void, mode: u32) -> *const c_void;
    fn imre_mapping_iter_next(cursor: *const c_void, key: *mut usize, value: *mut usize) -> u32;
    fn imre_object_release(value: *const c_void);
}

#[test]
fn nullable_string_dictionary_keeps_none_and_string_distinct_and_iterates_in_order() {
    // SAFETY: the words below exactly match the nullable-str and bool descriptors.
    unsafe {
        let text = imre_runtime::imre_str_literal(b"alpha".as_ptr(), 5);
        let dictionary = imre_dict_new(256 | 3, 1);
        imre_dict_set(dictionary, 0, 1);
        imre_dict_set(dictionary, text as usize, 0);
        assert_eq!(imre_dict_contains(dictionary, 0), 1);
        assert_eq!(imre_dict_contains(dictionary, text as usize), 1);
        assert_eq!(imre_dict_get(dictionary, 0), 1);
        assert_eq!(imre_dict_get(dictionary, text as usize), 0);

        let cursor = imre_dict_iter(dictionary, 1);
        let (mut key, mut value) = (usize::MAX, usize::MAX);
        assert_eq!(imre_mapping_iter_next(cursor, &mut key, &mut value), 1);
        assert_eq!((key, value), (0, 1));
        assert_eq!(imre_mapping_iter_next(cursor, &mut key, &mut value), 1);
        assert_eq!((key, value), (text as usize, 0));
        imre_runtime::imre_str_release(key as *const imre_runtime::Str);
        assert_eq!(imre_mapping_iter_next(cursor, &mut key, &mut value), 0);
        imre_object_release(cursor);
        imre_object_release(dictionary);
        imre_runtime::imre_str_release(text);
    }
}
