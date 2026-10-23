//! Static message and signal tables plus the decoder that walks them.
//!
//! A [`Catalog`] is a sorted slice of [`MessageDef`]s. Tables are plain data, so
//! the only code that touches frame bytes is [`crate::bits`], which is bounds
//! checked. [`Catalog::validate`] proves a table is internally consistent and
//! is run in tests for every shipped table and may be run once at boot.

use crate::bits::{extract, sign_extend, ByteOrder};

/// Interpretation of the raw bits of a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Unsigned integer.
    Unsigned,
    /// Two's complement signed integer.
    Signed,
    /// IEEE 754 binary32 (the signal must be 32 bits wide).
    Float32,
}

/// One signal inside a message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Signal {
    /// Signal name from the source database.
    pub name: &'static str,
    /// DBC start bit.
    pub start_bit: u16,
    /// Width in bits, 1 to 64.
    pub len: u8,
    /// Byte order.
    pub order: ByteOrder,
    /// Interpretation of the raw bits.
    pub kind: Kind,
    /// Physical value = raw * factor + offset.
    pub factor: f64,
    /// See `factor`.
    pub offset: f64,
    /// Engineering unit, may be empty.
    pub unit: &'static str,
}

impl Signal {
    /// Raw bits to a signed integer where that is meaningful (`Unsigned`
    /// values above `i64::MAX` saturate; `Float32` returns the bit pattern).
    pub fn raw_to_i64(&self, raw: u64) -> i64 {
        match self.kind {
            Kind::Signed => sign_extend(raw, self.len),
            Kind::Unsigned | Kind::Float32 => i64::try_from(raw).unwrap_or(i64::MAX),
        }
    }

    /// Raw bits to the physical value. Intended for host tools; the firmware
    /// logs raw bits only.
    pub fn physical(&self, raw: u64) -> f64 {
        let base = match self.kind {
            Kind::Float32 => f64::from(f32::from_bits(u32::try_from(raw & 0xFFFF_FFFF).unwrap_or(0))),
            Kind::Signed => sign_extend(raw, self.len) as f64,
            Kind::Unsigned => raw as f64,
        };
        base * self.factor + self.offset
    }
}

/// One CAN message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MessageDef {
    /// CAN identifier (11 or 29 bit).
    pub id: u32,
    /// True for a 29-bit identifier.
    pub extended: bool,
    /// Message name from the source database.
    pub name: &'static str,
    /// Minimum payload length in bytes. Shorter frames are rejected.
    pub dlc: u8,
    /// Index of this message's first signal in the catalog-wide numbering.
    pub first_signal: u16,
    /// Signals in the message.
    pub signals: &'static [Signal],
}

/// Catalog-wide signal number: `first_signal` of its message plus its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignalId(pub u16);

/// Result of [`Catalog::decode`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decoded {
    /// No message with this identifier is in the catalog.
    UnknownId,
    /// The frame is shorter than the message requires.
    TooShort {
        /// Bytes required.
        need: u8,
        /// Bytes received.
        have: usize,
    },
    /// All signals were extracted.
    Ok {
        /// The message that matched.
        message: &'static MessageDef,
        /// Number of signals delivered to the callback.
        signals: u16,
    },
}

/// A problem found by [`Catalog::validate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogError {
    /// Messages are not strictly increasing by `(extended, id)`.
    NotSorted {
        /// Index of the offending message.
        message: usize,
    },
    /// An identifier does not fit its 11 or 29 bit range.
    IdOutOfRange {
        /// Index of the offending message.
        message: usize,
    },
    /// `dlc` is above 8.
    DlcTooLarge {
        /// Index of the offending message.
        message: usize,
    },
    /// `first_signal` does not continue the previous message's numbering.
    BadSignalNumbering {
        /// Index of the offending message.
        message: usize,
    },
    /// A signal has an invalid width or `Float32` with a width other than 32.
    BadWidth {
        /// Index of the offending message.
        message: usize,
        /// Index of the offending signal.
        signal: usize,
    },
    /// A signal does not lie inside `dlc` bytes.
    SignalOutsideFrame {
        /// Index of the offending message.
        message: usize,
        /// Index of the offending signal.
        signal: usize,
    },
    /// The table has more signals than a `SignalId` can number.
    TooManySignals,
}

/// A sorted message table.
#[derive(Clone, Copy, Debug)]
pub struct Catalog {
    messages: &'static [MessageDef],
}

impl Catalog {
    /// Wrap a table. Call [`Catalog::validate`] to check it.
    pub const fn new(messages: &'static [MessageDef]) -> Self {
        Self { messages }
    }

    /// The underlying messages.
    pub fn messages(&self) -> &'static [MessageDef] {
        self.messages
    }

    /// Total number of signals, which is also the size needed by
    /// [`crate::latest::Latest`].
    pub fn signal_count(&self) -> usize {
        self.messages.iter().fold(0_usize, |acc, m| acc.saturating_add(m.signals.len()))
    }

    /// Check the table for internal consistency.
    pub fn validate(&self) -> Result<(), CatalogError> {
        let mut prev: Option<(bool, u32)> = None;
        let mut next_signal: usize = 0;
        for (mi, m) in self.messages.iter().enumerate() {
            let key = (m.extended, m.id);
            if let Some(p) = prev {
                if key <= p {
                    return Err(CatalogError::NotSorted { message: mi });
                }
            }
            prev = Some(key);
            let max_id: u32 = if m.extended { 0x1FFF_FFFF } else { 0x7FF };
            if m.id > max_id {
                return Err(CatalogError::IdOutOfRange { message: mi });
            }
            if m.dlc > 8 {
                return Err(CatalogError::DlcTooLarge { message: mi });
            }
            if usize::from(m.first_signal) != next_signal {
                return Err(CatalogError::BadSignalNumbering { message: mi });
            }
            for (si, s) in m.signals.iter().enumerate() {
                if s.len == 0 || s.len > 64 || (s.kind == Kind::Float32 && s.len != 32) {
                    return Err(CatalogError::BadWidth { message: mi, signal: si });
                }
                // Prove the signal lies inside the frame by extracting it from a
                // zero frame of exactly `dlc` bytes.
                let probe = [0_u8; 8];
                let frame = probe.get(..usize::from(m.dlc)).unwrap_or(&[]);
                if extract(frame, s.start_bit, s.len, s.order).is_none() {
                    return Err(CatalogError::SignalOutsideFrame { message: mi, signal: si });
                }
            }
            next_signal = next_signal.saturating_add(m.signals.len());
        }
        if next_signal > usize::from(u16::MAX) {
            return Err(CatalogError::TooManySignals);
        }
        Ok(())
    }

    /// Find the definition for an identifier.
    pub fn find(&self, id: u32, extended: bool) -> Option<&'static MessageDef> {
        let idx = self.messages.binary_search_by_key(&(extended, id), |m| (m.extended, m.id)).ok()?;
        self.messages.get(idx)
    }

    /// Decode a frame, calling `sink` once per signal with its catalog-wide id,
    /// definition and raw bits. Nothing is delivered unless every signal in the
    /// message can be read, so partial messages never reach the caller.
    pub fn decode<F>(&self, id: u32, extended: bool, data: &[u8], mut sink: F) -> Decoded
    where
        F: FnMut(SignalId, &'static Signal, u64),
    {
        let Some(def) = self.find(id, extended) else { return Decoded::UnknownId };
        if data.len() < usize::from(def.dlc) {
            return Decoded::TooShort { need: def.dlc, have: data.len() };
        }
        // First pass: extract everything. Second pass: deliver. A signal that
        // cannot be read (only possible with a table that failed validation)
        // aborts the whole message instead of delivering part of it.
        let mut delivered: u16 = 0;
        for s in def.signals {
            if extract(data, s.start_bit, s.len, s.order).is_none() {
                return Decoded::TooShort { need: def.dlc, have: data.len() };
            }
        }
        for (i, s) in def.signals.iter().enumerate() {
            let Some(raw) = extract(data, s.start_bit, s.len, s.order) else { continue };
            let Some(sid) = u16::try_from(i).ok().and_then(|i| def.first_signal.checked_add(i)) else { continue };
            sink(SignalId(sid), s, raw);
            delivered = delivered.saturating_add(1);
        }
        Decoded::Ok { message: def, signals: delivered }
    }
}
