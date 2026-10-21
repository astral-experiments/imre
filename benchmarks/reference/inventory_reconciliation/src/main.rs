//! Direct BigInt port of the inventory program, retaining eager source order.

use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::os::fd::IntoRawFd;
use std::process::ExitCode;

type Result<T> = std::result::Result<T, Error>;

struct Error {
    kind: &'static str,
    message: String,
}

impl Error {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

unsafe extern "C" {
    fn close(fd: std::ffi::c_int) -> std::ffi::c_int;
}

/// Consume the sole descriptor owner; Linux close must not be retried on error.
fn checked_close(file: File, message: &str) -> Result<()> {
    let fd = file.into_raw_fd();
    // SAFETY: into_raw_fd transferred exclusive ownership of this live descriptor.
    if unsafe { close(fd) } == 0 {
        Ok(())
    } else {
        Err(Error::new("OSError", message))
    }
}

fn splitlines(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((position, character)) = characters.next() {
        if matches!(
            character,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            result.push(text[start..position].to_owned());
            start = position + character.len_utf8();
            if character == '\r' && characters.peek().is_some_and(|(_, next)| *next == '\n') {
                characters.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        result.push(text[start..].to_owned());
    }
    result
}

/// Closing an input overrides an earlier read/decode failure, as Python with does.
fn load(path: &str, label: &str) -> Result<Vec<String>> {
    let message = format!("{label}: cannot read");
    if path.as_bytes().contains(&0) {
        return Err(Error::new("ValueError", format!("{label}: invalid path")));
    }
    let mut file = File::open(path).map_err(|_| Error::new("OSError", &message))?;
    let work = (|| {
        if file
            .metadata()
            .map_err(|_| Error::new("OSError", &message))?
            .is_dir()
        {
            return Err(Error::new("OSError", &message));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| Error::new("OSError", &message))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| Error::new("ValueError", format!("{label}: invalid UTF-8")))?;
        Ok(splitlines(&text))
    })();
    checked_close(file, &message)?;
    work
}

struct Reader {
    lines: Vec<String>,
    position: usize,
    label: &'static str,
}

impl Reader {
    fn new(lines: Vec<String>, label: &'static str) -> Self {
        Self {
            lines,
            position: 0,
            label,
        }
    }

    fn error(&self, message: &str) -> Error {
        Error::new("ValueError", format!("{}: {message}", self.label))
    }

    fn next(&mut self) -> Result<Vec<String>> {
        let line = self
            .lines
            .get(self.position)
            .ok_or_else(|| self.error("truncated"))?;
        self.position += 1;
        Ok(line
            .split(|character: char| {
                character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
            })
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn number(&self, token: &str) -> Result<BigInt> {
        let digits = token.strip_prefix(['+', '-']).unwrap_or(token);
        if digits.is_empty()
            || digits.starts_with('_')
            || digits.ends_with('_')
            || digits.contains("__")
            || !digits
                .bytes()
                .all(|value| value.is_ascii_digit() || value == b'_')
        {
            return Err(self.error("invalid integer"));
        }
        BigInt::parse_bytes(token.replace('_', "").as_bytes(), 10)
            .ok_or_else(|| self.error("invalid integer"))
    }

    fn count(&mut self) -> Result<usize> {
        let fields = self.next()?;
        if fields.len() != 1 {
            return Err(self.error("expected count"));
        }
        let value = self.number(&fields[0])?;
        if value < BigInt::zero() {
            return Err(self.error("negative count"));
        }
        // An unaddressable frame still reads available rows before reporting EOF.
        Ok(value.to_usize().unwrap_or(usize::MAX))
    }

    fn item(&mut self) -> Result<(String, BigInt)> {
        let fields = self.next()?;
        if fields.len() != 2 {
            return Err(self.error("expected SKU and quantity"));
        }
        Ok((fields[0].clone(), self.number(&fields[1])?))
    }

    fn finish(&self) -> Result<()> {
        if self.position == self.lines.len() {
            Ok(())
        } else {
            Err(self.error("trailing data"))
        }
    }
}

struct Stock {
    sku: String,
    quantity: BigInt,
}

struct Request {
    batch: usize,
    row: usize,
    sku: String,
    quantity: BigInt,
}

fn read_stock(lines: Vec<String>) -> Result<Vec<Stock>> {
    let mut reader = Reader::new(lines, "stock");
    let count = reader.count()?;
    let mut stocks = Vec::new();
    for _ in 0..count {
        let (sku, quantity) = reader.item()?;
        if quantity < BigInt::zero() {
            return Err(reader.error("negative quantity"));
        }
        stocks.push(Stock { sku, quantity });
    }
    reader.finish()?;
    Ok(stocks)
}

fn read_orders(lines: Vec<String>) -> Result<Vec<Vec<Request>>> {
    let mut reader = Reader::new(lines, "orders");
    let count = reader.count()?;
    let mut batches = Vec::new();
    for batch in 0..count {
        let rows = reader.count()?;
        let mut requests = Vec::new();
        for row in 0..rows {
            let (sku, quantity) = reader.item()?;
            requests.push(Request {
                batch,
                row,
                sku,
                quantity,
            });
        }
        batches.push(requests);
    }
    reader.finish()?;
    Ok(batches)
}

/// Hash lookup plus insertion-order traversal, matching the Python dictionaries.
struct Amounts<'a> {
    entries: Vec<(&'a str, BigInt)>,
    indexes: HashMap<&'a str, usize>,
}

impl<'a> Amounts<'a> {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            indexes: HashMap::new(),
        }
    }

    fn insert(&mut self, sku: &'a str, quantity: BigInt) {
        if let Some(&index) = self.indexes.get(sku) {
            self.entries[index].1 = quantity;
        } else {
            self.indexes.insert(sku, self.entries.len());
            self.entries.push((sku, quantity));
        }
    }
}

struct Fulfillment<'a> {
    request: &'a Request,
    filled: BigInt,
    shortage: BigInt,
}

fn fulfill<'a>(request: &'a Request, remaining: &mut Amounts<'_>) -> Fulfillment<'a> {
    let mut filled = BigInt::zero();
    if let Some(&index) = remaining.indexes.get(request.sku.as_str()) {
        let available = &mut remaining.entries[index].1;
        filled = request.quantity.clone().min(available.clone());
        *available -= &filled;
    }
    let shortage = &request.quantity - &filled;
    Fulfillment {
        request,
        filled,
        shortage,
    }
}

struct Report<'a> {
    stock_rows: usize,
    batches: usize,
    request_rows: usize,
    results: Vec<Fulfillment<'a>>,
    requested: Amounts<'a>,
    requested_skus: HashSet<&'a str>,
    remaining: Amounts<'a>,
}

fn reconcile<'a>(stocks: &'a [Stock], batches: &'a [Vec<Request>]) -> Report<'a> {
    let mut remaining = Amounts::new();
    for stock in stocks {
        remaining.insert(&stock.sku, stock.quantity.clone());
    }
    let positive: Vec<_> = batches
        .iter()
        .flatten()
        .filter(|request| request.quantity > BigInt::zero())
        .collect();
    let requested_skus = positive
        .iter()
        .map(|request| request.sku.as_str())
        .collect();
    let mut requested = Amounts::new();
    for request in &positive {
        requested.insert(&request.sku, BigInt::zero());
    }
    for request in &positive {
        requested.entries[requested.indexes[request.sku.as_str()]].1 += &request.quantity;
    }
    let results = positive
        .into_iter()
        .map(|request| fulfill(request, &mut remaining))
        .collect();
    Report {
        stock_rows: stocks.len(),
        batches: batches.len(),
        request_rows: batches.iter().map(Vec::len).sum(),
        results,
        requested,
        requested_skus,
        remaining,
    }
}

fn show(report: &Report<'_>, output: &mut impl Write) -> io::Result<()> {
    let shortages: Vec<_> = report
        .results
        .iter()
        .filter(|result| result.shortage > BigInt::zero())
        .collect();
    let untouched: Vec<_> = report
        .remaining
        .entries
        .iter()
        .filter(|(sku, _)| !report.requested_skus.contains(sku))
        .collect();
    let mut requested = BigInt::zero();
    let mut filled = BigInt::zero();
    let mut shortage = BigInt::zero();
    for result in &report.results {
        requested += &result.request.quantity;
        filled += &result.filled;
        shortage += &result.shortage;
    }
    writeln!(output, "INVENTORY")?;
    writeln!(output, "STOCK_ROWS {}", report.stock_rows)?;
    writeln!(output, "STOCK_SKUS {}", report.remaining.entries.len())?;
    writeln!(output, "BATCHES {}", report.batches)?;
    writeln!(output, "REQUEST_ROWS {}", report.request_rows)?;
    writeln!(output, "POSITIVE_REQUESTS {}", report.results.len())?;
    writeln!(output, "REQUESTED_SKUS {}", report.requested_skus.len())?;
    writeln!(output, "TOTAL_REQUESTED {requested}")?;
    writeln!(output, "TOTAL_FILLED {filled}")?;
    writeln!(output, "TOTAL_SHORTAGE {shortage}")?;
    writeln!(output, "FULFILLMENTS {}", report.results.len())?;
    for result in &report.results {
        let request = result.request;
        writeln!(
            output,
            "FULFILL {} {} {} {} {} {}",
            request.batch,
            request.row,
            request.sku,
            request.quantity,
            result.filled,
            result.shortage
        )?;
    }
    writeln!(output, "SHORTAGES {}", shortages.len())?;
    for result in shortages {
        let request = result.request;
        writeln!(
            output,
            "SHORTAGE {} {} {} {}",
            request.batch, request.row, request.sku, result.shortage
        )?;
    }
    writeln!(output, "REQUESTED {}", report.requested.entries.len())?;
    for (sku, quantity) in &report.requested.entries {
        writeln!(output, "REQUEST {sku} {quantity}")?;
    }
    writeln!(output, "REMAINING {}", report.remaining.entries.len())?;
    for (sku, quantity) in &report.remaining.entries {
        writeln!(output, "STOCK {sku} {quantity}")?;
    }
    writeln!(output, "UNTOUCHED {}", untouched.len())?;
    for (sku, quantity) in untouched {
        writeln!(output, "STOCK {sku} {quantity}")?;
    }
    writeln!(output, "END")
}

fn write_report(path: &str, report: &Report<'_>) -> Result<()> {
    if path.as_bytes().contains(&0) {
        return Err(Error::new("ValueError", "report: invalid path"));
    }
    let file = File::create(path).map_err(|_| Error::new("OSError", "report: cannot write"))?;
    let mut output = BufWriter::with_capacity(8192, file);
    let rendered = show(report, &mut output);
    let flushed = output.flush();
    let (file, unwritten) = output.into_parts();
    drop(unwritten);
    checked_close(file, "report: cannot write")?;
    rendered
        .and(flushed)
        .map_err(|_| Error::new("OSError", "report: cannot write"))
}

fn run(arguments: &[String]) -> Result<()> {
    if arguments.len() != 4 {
        return Err(Error::new(
            "ValueError",
            "usage: inventory_reconciliation STOCK ORDERS REPORT",
        ));
    }
    let stocks = read_stock(load(&arguments[1], "stock")?)?;
    let batches = read_orders(load(&arguments[2], "orders")?)?;
    write_report(&arguments[3], &reconcile(&stocks, &batches))
}

fn main() -> ExitCode {
    match run(&std::env::args().collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{}: {}", error.kind, error.message);
            ExitCode::FAILURE
        }
    }
}
