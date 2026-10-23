//! Turn a DBC file into a `tel-can` message table (Rust source).
//!
//! The generator is strict: anything `tel-can` cannot decode correctly
//! (multiplexed signals, 64-bit floats, duplicate identifiers, signals that
//! overrun their frame) is an error rather than being silently skipped.

use can_dbc::{ByteOrder, Dbc, MessageId, MultiplexIndicator, SignalExtendedValueType, ValueType};
use std::fmt::{self, Write};

/// Interpretation of a signal's bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindModel {
    /// Unsigned integer.
    Unsigned,
    /// Signed integer.
    Signed,
    /// IEEE binary32.
    Float32,
}

/// One signal.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalModel {
    /// Name.
    pub name: String,
    /// DBC start bit.
    pub start_bit: u16,
    /// Width in bits.
    pub len: u8,
    /// True for big endian (`@0`).
    pub big_endian: bool,
    /// Interpretation.
    pub kind: KindModel,
    /// Scale factor.
    pub factor: f64,
    /// Offset.
    pub offset: f64,
    /// Unit.
    pub unit: String,
}

/// One message.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageModel {
    /// Identifier without the extended flag.
    pub id: u32,
    /// 29-bit identifier.
    pub extended: bool,
    /// Name.
    pub name: String,
    /// Payload length in bytes.
    pub dlc: u8,
    /// Signals in file order.
    pub signals: Vec<SignalModel>,
}

/// Generator options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Message names to leave out of the table. Each must exist in the DBC, so
    /// a stale skip list is an error rather than a silent no-op.
    pub skip: Vec<String>,
}

impl Options {
    /// Parse a skip file: one message name per line, `#` starts a comment.
    pub fn from_skip_file(text: &str) -> Self {
        let skip = text
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").trim())
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        Self { skip }
    }
}

/// Generator failure.
#[derive(Debug, PartialEq)]
pub enum GenError {
    /// A signal does not fit inside the message's declared length.
    SignalOutsideFrame(String),
    /// A name in the skip list is not in the DBC.
    UnknownSkip(String),
    /// The DBC could not be parsed.
    Parse(String),
    /// A construct that `tel-can` cannot decode.
    Unsupported(String),
    /// Two messages share an identifier.
    DuplicateId(u32, bool),
    /// A signal or message value is out of range.
    OutOfRange(String),
}

impl fmt::Display for GenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignalOutsideFrame(e) => write!(
                f,
                "signal does not fit in its message: {e} (add the message to the skip file if it is not needed)"
            ),
            Self::UnknownSkip(e) => write!(f, "skip list names a message that is not in the DBC: {e}"),
            Self::Parse(e) => write!(f, "DBC parse error: {e}"),
            Self::Unsupported(e) => write!(f, "unsupported DBC feature: {e}"),
            Self::DuplicateId(id, ext) => write!(f, "duplicate message id {id:#x} (extended: {ext})"),
            Self::OutOfRange(e) => write!(f, "value out of range: {e}"),
        }
    }
}

impl std::error::Error for GenError {}

/// Rewrite signal ranges that have a single value, such as `[4294970000]`, to
/// `[x|x]`. SBG's published DBC contains one such line and the parser requires
/// `[min|max]`. Minimum and maximum play no part in decoding, so this cannot
/// change a decoded value. Returns the new text and the number of lines fixed.
pub fn sanitize(text: &str) -> (String, usize) {
    fn fix_range_line(line: &str) -> Option<String> {
        if !line.trim_start().starts_with("SG_ ") {
            return None;
        }
        let (head, rest) = line.split_once('[')?;
        let (range, tail) = rest.split_once(']')?;
        if range.contains('|') {
            return None;
        }
        Some(format!("{head}[{range}|{range}]{tail}"))
    }
    let mut fixed = 0_usize;
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match fix_range_line(line) {
            Some(new) => {
                fixed = fixed.saturating_add(1);
                out.push_str(&new);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    (out, fixed)
}

/// Parse a DBC into the generator's model, sorted by `(extended, id)`.
pub fn model_from_dbc(text: &str, opts: &Options) -> Result<Vec<MessageModel>, GenError> {
    let (clean, _) = sanitize(text);
    let dbc = Dbc::try_from(clean.as_str()).map_err(|e| GenError::Parse(format!("{e:?}")))?;
    for name in &opts.skip {
        if !dbc.messages.iter().any(|m| &m.name == name) {
            return Err(GenError::UnknownSkip(name.clone()));
        }
    }
    let mut out: Vec<MessageModel> = Vec::new();
    for m in &dbc.messages {
        if opts.skip.iter().any(|s| s == &m.name) {
            continue;
        }
        let (id, extended) = match m.id {
            MessageId::Standard(id) => (u32::from(id), false),
            MessageId::Extended(id) => (id, true),
        };
        let max_id: u32 = if extended { 0x1FFF_FFFF } else { 0x7FF };
        if id > max_id {
            return Err(GenError::OutOfRange(format!("message {} id {id:#x}", m.name)));
        }
        let dlc = u8::try_from(m.size)
            .ok()
            .filter(|d| *d <= 8)
            .ok_or_else(|| GenError::OutOfRange(format!("message {} size {}", m.name, m.size)))?;
        let mut signals = Vec::new();
        for s in &m.signals {
            if !matches!(s.multiplexer_indicator, MultiplexIndicator::Plain) {
                return Err(GenError::Unsupported(format!("multiplexed signal {}.{}", m.name, s.name)));
            }
            let len = u8::try_from(s.size)
                .ok()
                .filter(|l| (1..=64).contains(l))
                .ok_or_else(|| GenError::OutOfRange(format!("{}.{} width {}", m.name, s.name, s.size)))?;
            let start_bit = u16::try_from(s.start_bit)
                .ok()
                .filter(|b| *b < 64)
                .ok_or_else(|| GenError::OutOfRange(format!("{}.{} start bit {}", m.name, s.name, s.start_bit)))?;
            let kind = match dbc.extended_value_type_for_signal(m.id, &s.name) {
                Some(SignalExtendedValueType::IEEEfloat32Bit) => {
                    if len != 32 {
                        return Err(GenError::OutOfRange(format!("{}.{} float32 with width {len}", m.name, s.name)));
                    }
                    KindModel::Float32
                }
                Some(SignalExtendedValueType::IEEEdouble64bit) => {
                    return Err(GenError::Unsupported(format!("64-bit float signal {}.{}", m.name, s.name)));
                }
                _ => match s.value_type {
                    ValueType::Signed => KindModel::Signed,
                    ValueType::Unsigned => KindModel::Unsigned,
                },
            };
            if !s.factor.is_finite() || !s.offset.is_finite() {
                return Err(GenError::OutOfRange(format!("{}.{} factor/offset not finite", m.name, s.name)));
            }
            let order = if matches!(s.byte_order, ByteOrder::BigEndian) {
                tel_can::ByteOrder::BigEndian
            } else {
                tel_can::ByteOrder::LittleEndian
            };
            let zero = [0_u8; 8];
            let frame = zero.get(..usize::from(dlc)).unwrap_or(&[]);
            if tel_can::extract(frame, start_bit, len, order).is_none() {
                return Err(GenError::SignalOutsideFrame(format!("{}.{} (DLC {dlc})", m.name, s.name)));
            }
            signals.push(SignalModel {
                name: s.name.clone(),
                start_bit,
                len,
                big_endian: matches!(s.byte_order, ByteOrder::BigEndian),
                kind,
                factor: s.factor,
                offset: s.offset,
                unit: s.unit.clone(),
            });
        }
        out.push(MessageModel { id, extended, name: m.name.clone(), dlc, signals });
    }
    finish(out)
}

/// Sort by `(extended, id)` and reject duplicate identifiers.
fn finish(mut out: Vec<MessageModel>) -> Result<Vec<MessageModel>, GenError> {
    out.sort_by_key(|m| (m.extended, m.id));
    for pair in out.windows(2) {
        if let [a, b] = pair {
            if (a.extended, a.id) == (b.extended, b.id) {
                return Err(GenError::DuplicateId(a.id, a.extended));
            }
        }
    }
    Ok(out)
}

/// Check one message the same way the runtime validator will.
fn check_message(m: &MessageModel) -> Result<(), GenError> {
    let max_id: u32 = if m.extended { 0x1FFF_FFFF } else { 0x7FF };
    if m.id > max_id {
        return Err(GenError::OutOfRange(format!("message {} id {:#x}", m.name, m.id)));
    }
    if m.dlc > 8 {
        return Err(GenError::OutOfRange(format!("message {} size {}", m.name, m.dlc)));
    }
    for s in &m.signals {
        if !(1..=64).contains(&s.len) {
            return Err(GenError::OutOfRange(format!("{}.{} width {}", m.name, s.name, s.len)));
        }
        if s.kind == KindModel::Float32 && s.len != 32 {
            return Err(GenError::OutOfRange(format!("{}.{} float32 with width {}", m.name, s.name, s.len)));
        }
        if !s.factor.is_finite() || !s.offset.is_finite() {
            return Err(GenError::OutOfRange(format!("{}.{} factor/offset not finite", m.name, s.name)));
        }
        let order = if s.big_endian { tel_can::ByteOrder::BigEndian } else { tel_can::ByteOrder::LittleEndian };
        let zero = [0_u8; 8];
        let frame = zero.get(..usize::from(m.dlc)).unwrap_or(&[]);
        if tel_can::extract(frame, s.start_bit, s.len, order).is_none() {
            return Err(GenError::SignalOutsideFrame(format!("{}.{} (DLC {})", m.name, s.name, m.dlc)));
        }
    }
    Ok(())
}

/// Header row required in a CSV signal list.
pub const CSV_HEADER: &str = "id,extended,message,dlc,signal,start_bit,len,order,kind,factor,offset,unit";

fn parse_bool(s: &str, what: &str, line: usize) -> Result<bool, GenError> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(GenError::Parse(format!("line {line}: {what} must be 0 or 1, got {s:?}"))),
    }
}

fn parse_num<T: std::str::FromStr>(s: &str, what: &str, line: usize) -> Result<T, GenError> {
    s.parse::<T>().map_err(|_| GenError::Parse(format!("line {line}: bad {what} {s:?}")))
}

/// Parse a hand-written signal list.
///
/// One row per signal, columns as in [`CSV_HEADER`]. `id` is decimal or
/// `0x` hexadecimal, `extended` is 0 or 1, `order` is `le` or `be`, `kind` is
/// `u`, `s` or `f32`. Rows of the same message must repeat the same message
/// name and `dlc`. Blank lines and lines starting with `#` are ignored; fields
/// cannot contain commas. Use this when no DBC exists, for example for the
/// MoTeC streams.
pub fn model_from_csv(text: &str) -> Result<Vec<MessageModel>, GenError> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i.saturating_add(1), l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'));
    match lines.next() {
        Some((_, h)) if h.replace(' ', "") == CSV_HEADER => {}
        Some((n, h)) => return Err(GenError::Parse(format!("line {n}: expected header {CSV_HEADER:?}, got {h:?}"))),
        None => return Ok(Vec::new()),
    }
    let mut out: Vec<MessageModel> = Vec::new();
    for (n, line) in lines {
        let mut it = line.split(',').map(str::trim);
        let mut next = |what: &str| it.next().ok_or_else(|| GenError::Parse(format!("line {n}: missing {what}")));
        let id_s = next("id")?;
        let id: u32 = match id_s.strip_prefix("0x").or_else(|| id_s.strip_prefix("0X")) {
            Some(hex) => {
                u32::from_str_radix(hex, 16).map_err(|_| GenError::Parse(format!("line {n}: bad id {id_s:?}")))?
            }
            None => parse_num(id_s, "id", n)?,
        };
        let extended = parse_bool(next("extended")?, "extended", n)?;
        let message = next("message")?.to_string();
        let dlc: u8 = parse_num(next("dlc")?, "dlc", n)?;
        let signal = next("signal")?.to_string();
        let start_bit: u16 = parse_num(next("start_bit")?, "start_bit", n)?;
        let len: u8 = parse_num(next("len")?, "len", n)?;
        let big_endian = match next("order")?.to_ascii_lowercase().as_str() {
            "le" => false,
            "be" => true,
            other => return Err(GenError::Parse(format!("line {n}: order must be le or be, got {other:?}"))),
        };
        let kind = match next("kind")?.to_ascii_lowercase().as_str() {
            "u" => KindModel::Unsigned,
            "s" => KindModel::Signed,
            "f32" => KindModel::Float32,
            other => return Err(GenError::Parse(format!("line {n}: kind must be u, s or f32, got {other:?}"))),
        };
        let factor: f64 = parse_num(next("factor")?, "factor", n)?;
        let offset: f64 = parse_num(next("offset")?, "offset", n)?;
        let unit = it.next().unwrap_or("").to_string();
        if it.next().is_some() {
            return Err(GenError::Parse(format!("line {n}: too many fields (commas are not allowed inside fields)")));
        }
        if signal.is_empty() || message.is_empty() {
            return Err(GenError::Parse(format!("line {n}: message and signal names must not be empty")));
        }
        let sig = SignalModel { name: signal, start_bit, len, big_endian, kind, factor, offset, unit };
        match out.iter_mut().find(|m| (m.extended, m.id) == (extended, id)) {
            Some(m) => {
                if m.name != message || m.dlc != dlc {
                    return Err(GenError::Parse(format!(
                        "line {n}: message {id:#x} repeated with a different name or dlc"
                    )));
                }
                if m.signals.iter().any(|s| s.name == sig.name) {
                    return Err(GenError::Parse(format!("line {n}: duplicate signal {}.{}", m.name, sig.name)));
                }
                m.signals.push(sig);
            }
            None => out.push(MessageModel { id, extended, name: message, dlc, signals: std::vec![sig] }),
        }
    }
    for m in &out {
        check_message(m)?;
    }
    finish(out)
}

/// Convenience: CSV signal list to Rust source.
pub fn generate_from_csv(text: &str, source: &str) -> Result<String, GenError> {
    Ok(render(&model_from_csv(text)?, source, &Options::default()))
}

fn kind_tokens(k: KindModel) -> &'static str {
    match k {
        KindModel::Unsigned => "Kind::Unsigned",
        KindModel::Signed => "Kind::Signed",
        KindModel::Float32 => "Kind::Float32",
    }
}

/// Render the model as Rust source.
pub fn render(models: &[MessageModel], source: &str, opts: &Options) -> String {
    let mut s = String::new();
    // `write!` to a String cannot fail; results are ignored deliberately.
    let _ = writeln!(s, "// @generated by tools/dbc2table from {source}. Do not edit; regenerate instead.");
    if !opts.skip.is_empty() {
        let _ = writeln!(s, "// Skipped messages: {}", opts.skip.join(", "));
    }
    let _ = writeln!(s, "#[allow(unused_imports)] // an empty table uses none of these");
    let _ = writeln!(s, "use tel_can::{{ByteOrder, Catalog, Kind, MessageDef, Signal}};\n");
    let mut first_signal: usize = 0;
    let mut first_signals = Vec::with_capacity(models.len());
    for (i, m) in models.iter().enumerate() {
        first_signals.push(first_signal);
        let _ = writeln!(s, "static SIGNALS_{i}: [Signal; {}] = [", m.signals.len());
        for sig in &m.signals {
            let _ = writeln!(
                s,
                "    Signal {{ name: {:?}, start_bit: {}, len: {}, order: ByteOrder::{}, kind: {}, factor: {:?}, offset: {:?}, unit: {:?} }},",
                sig.name,
                sig.start_bit,
                sig.len,
                if sig.big_endian { "BigEndian" } else { "LittleEndian" },
                kind_tokens(sig.kind),
                sig.factor,
                sig.offset,
                sig.unit,
            );
        }
        let _ = writeln!(s, "];\n");
        first_signal = first_signal.saturating_add(m.signals.len());
    }
    let _ = writeln!(s, "/// Messages sorted by `(extended, id)`.");
    let _ = writeln!(s, "pub static MESSAGES: [MessageDef; {}] = [", models.len());
    for (i, (m, first)) in models.iter().zip(first_signals).enumerate() {
        let _ = writeln!(
            s,
            "    MessageDef {{ id: {:#x}, extended: {}, name: {:?}, dlc: {}, first_signal: {}, signals: &SIGNALS_{i} }},",
            m.id, m.extended, m.name, m.dlc, first,
        );
    }
    let _ = writeln!(s, "];\n");
    let _ = writeln!(s, "/// The catalog over [`MESSAGES`].");
    let _ = writeln!(s, "pub const CATALOG: Catalog = Catalog::new(&MESSAGES);\n");
    let _ = writeln!(s, "/// Total number of signals (the size for `tel_can::Latest`).");
    let _ = writeln!(s, "pub const SIGNAL_COUNT: usize = {first_signal};");
    s
}

/// Convenience: DBC text to Rust source.
pub fn generate(text: &str, source: &str, opts: &Options) -> Result<String, GenError> {
    Ok(render(&model_from_dbc(text, opts)?, source, opts))
}
