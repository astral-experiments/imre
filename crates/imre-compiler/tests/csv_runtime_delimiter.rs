use imre_compiler::CompileOptions;

fn assert_typed_delimiter(source: &str) {
    imre_compiler::check(source).unwrap();
    let ir = imre_compiler::compile(source, &CompileOptions::default()).unwrap();
    assert!(
        ir.lines()
            .any(|line| line.contains("call ptr @imre_csv_reader_delimiter("))
    );
    assert!(
        !ir.lines()
            .any(|line| line.contains("call ") && line.contains("@imre_dyn_"))
    );
}

#[test]
fn runtime_csv_delimiter_and_invalid_lengths_use_the_checked_reader() {
    assert_typed_delimiter(
        "import csv\nimport io\n\ndef read(delimiter: str) -> list[list[str]]:\n    return list(csv.reader(io.StringIO('a;b\\n'), delimiter=delimiter))\n\nprint(read(';'))\n",
    );
    assert_typed_delimiter(
        "import csv\n\ndef read(delimiter: str | None) -> None:\n    csv.reader([], delimiter=delimiter)\n\nread(None)\n",
    );
}

#[test]
fn runtime_csv_delimiter_and_terminator_keep_both_keyword_orders() {
    for call in [
        "csv.reader([], delimiter=delimiter, lineterminator=terminator)",
        "csv.reader([], lineterminator=terminator, delimiter=delimiter)",
    ] {
        assert_typed_delimiter(&format!(
            "import csv\n\ndef read(delimiter: str, terminator: str) -> None:\n    {call}\n\nread(';', '\\r\\n')\n"
        ));
    }
}
