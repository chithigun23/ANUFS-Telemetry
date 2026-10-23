//! Byte-at-a-time NMEA frame assembler with a fixed-size buffer.
//!
//! Frames look like `$<payload>*HH` terminated by CR/LF (the terminator is not
//! required to complete a frame). The assembler never allocates, never indexes
//! out of bounds and resynchronises on the next `$` after any error.

use crate::num::hex_digit;

/// Maximum payload length (bytes between `$` and `*`). NMEA sentences are at
/// most 82 characters, Quectel proprietary sentences stay well below this.
pub const MAX_PAYLOAD: usize = 120;

/// A checksum-verified sentence payload (no `$`, no `*HH`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sentence<'a> {
    payload: &'a [u8],
}

impl<'a> Sentence<'a> {
    /// Wrap a payload that has already been checksum-verified.
    pub fn new(payload: &'a [u8]) -> Self {
        Self { payload }
    }

    /// The raw payload bytes (for example `GNRMC,093423.000,A,...`).
    pub fn payload(&self) -> &'a [u8] {
        self.payload
    }

    /// Address field: talker plus sentence type, or a proprietary name.
    pub fn address(&self) -> &'a [u8] {
        self.field(0).unwrap_or(&[])
    }

    /// Comma separated field `n`; field 0 is the address.
    pub fn field(&self, n: usize) -> Option<&'a [u8]> {
        self.payload.split(|&b| b == b',').nth(n)
    }

    /// Number of comma separated fields, including the address.
    pub fn field_count(&self) -> usize {
        self.payload.split(|&b| b == b',').count()
    }
}

/// Why a frame was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Computed and received checksums differ.
    ChecksumMismatch {
        /// Checksum computed over the payload.
        computed: u8,
        /// Checksum carried in the frame.
        received: u8,
    },
}

/// Running counters. All saturate, none can wrap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames delivered with a good checksum.
    pub frames_ok: u32,
    /// Frames dropped for a bad checksum.
    pub checksum_errors: u32,
    /// Frames dropped because the payload exceeded [`MAX_PAYLOAD`].
    pub overflows: u32,
    /// Frames abandoned because of a stray byte or a new `$` mid-frame.
    pub aborted: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Payload,
    Hex1,
    Hex2(u8),
}

enum Outcome {
    Nothing,
    Bad(FrameError),
    Good,
}

/// Incremental frame assembler.
#[derive(Clone, Debug)]
pub struct LineAssembler {
    buf: [u8; MAX_PAYLOAD],
    len: usize,
    xor: u8,
    state: State,
    stats: Stats,
}

impl Default for LineAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl LineAssembler {
    /// Create an empty assembler.
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_PAYLOAD],
            len: 0,
            xor: 0,
            state: State::Idle,
            stats: Stats { frames_ok: 0, checksum_errors: 0, overflows: 0, aborted: 0 },
        }
    }

    /// Counters since creation.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    fn start_frame(&mut self) {
        self.len = 0;
        self.xor = 0;
        self.state = State::Payload;
    }

    fn abort(&mut self) {
        self.stats.aborted = self.stats.aborted.saturating_add(1);
        self.state = State::Idle;
    }

    /// Feed one byte. Returns a sentence (valid until the next call) or an
    /// error when a frame completes.
    pub fn push(&mut self, byte: u8) -> Option<Result<Sentence<'_>, FrameError>> {
        let outcome = self.step(byte);
        match outcome {
            Outcome::Nothing => None,
            Outcome::Bad(e) => Some(Err(e)),
            Outcome::Good => {
                let payload = self.buf.get(..self.len).unwrap_or(&[]);
                Some(Ok(Sentence::new(payload)))
            }
        }
    }

    fn step(&mut self, byte: u8) -> Outcome {
        // A `$` always starts a new frame, even mid-frame.
        if byte == b'$' {
            if self.state != State::Idle {
                self.stats.aborted = self.stats.aborted.saturating_add(1);
            }
            self.start_frame();
            return Outcome::Nothing;
        }
        match self.state {
            State::Idle => Outcome::Nothing,
            State::Payload => {
                if byte == b'*' {
                    self.state = State::Hex1;
                } else if (0x20..=0x7E).contains(&byte) {
                    match self.buf.get_mut(self.len) {
                        Some(slot) => {
                            *slot = byte;
                            self.len = self.len.saturating_add(1);
                            self.xor ^= byte;
                        }
                        None => {
                            self.stats.overflows = self.stats.overflows.saturating_add(1);
                            self.state = State::Idle;
                        }
                    }
                } else {
                    // CR, LF or noise before the checksum: not a valid frame.
                    self.abort();
                }
                Outcome::Nothing
            }
            State::Hex1 => {
                match hex_digit(byte) {
                    Some(hi) => self.state = State::Hex2(hi),
                    None => self.abort(),
                }
                Outcome::Nothing
            }
            State::Hex2(hi) => {
                self.state = State::Idle;
                match hex_digit(byte) {
                    Some(lo) => {
                        let received = (hi << 4) | lo;
                        if received == self.xor {
                            self.stats.frames_ok = self.stats.frames_ok.saturating_add(1);
                            Outcome::Good
                        } else {
                            self.stats.checksum_errors = self.stats.checksum_errors.saturating_add(1);
                            Outcome::Bad(FrameError::ChecksumMismatch { computed: self.xor, received })
                        }
                    }
                    None => {
                        self.stats.aborted = self.stats.aborted.saturating_add(1);
                        Outcome::Nothing
                    }
                }
            }
        }
    }
}

/// XOR checksum over a payload, as used by NMEA.
pub fn checksum(payload: &[u8]) -> u8 {
    payload.iter().fold(0, |acc, &b| acc ^ b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    fn feed(asm: &mut LineAssembler, bytes: &[u8]) -> Vec<Result<String, FrameError>> {
        let mut out = Vec::new();
        for &b in bytes {
            if let Some(r) = asm.push(b) {
                out.push(r.map(|s| String::from_utf8_lossy(s.payload()).into_owned()));
            }
        }
        out
    }

    #[test]
    fn good_frame_spec_example() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"$GNRMC,093423.000,A,3149.332006,N,11706.913200,E,0.01,0.00,140122,,,A*79\r\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].as_deref().unwrap(), "GNRMC,093423.000,A,3149.332006,N,11706.913200,E,0.01,0.00,140122,,,A");
        assert_eq!(a.stats().frames_ok, 1);
    }

    #[test]
    fn terminator_not_required() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"$PQTMSAVEPAR*5A");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].as_deref().unwrap(), "PQTMSAVEPAR");
    }

    #[test]
    fn lowercase_checksum_accepted() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"$GNGGA,093316.000,3149.332558,N,11706.912570,E,1,28,0.67,54.481,M,-0.337,M,,*5d\r\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].is_ok());
    }

    #[test]
    fn bad_checksum_reported_then_recovers() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"$PQTMSAVEPAR*5B\r\n$PQTMSAVEPAR*5A\r\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], Err(FrameError::ChecksumMismatch { computed: 0x5A, received: 0x5B }));
        assert!(out[1].is_ok());
        assert_eq!(a.stats().checksum_errors, 1);
        assert_eq!(a.stats().frames_ok, 1);
    }

    #[test]
    fn garbage_between_frames_ignored() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"\x00\xff junk \r\n$PQTMSAVEPAR*5A\r\nmore junk");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn new_dollar_restarts_frame() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"$GNRM$PQTMSAVEPAR*5A");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].as_deref().unwrap(), "PQTMSAVEPAR");
        assert_eq!(a.stats().aborted, 1);
    }

    #[test]
    fn overlong_payload_dropped_and_recovers() {
        let mut a = LineAssembler::new();
        let mut data = Vec::new();
        data.push(b'$');
        data.extend(std::iter::repeat_n(b'A', MAX_PAYLOAD + 10));
        data.extend_from_slice(b"*00\r\n$PQTMSAVEPAR*5A\r\n");
        let out = feed(&mut a, &data);
        assert_eq!(out.len(), 1);
        assert_eq!(a.stats().overflows, 1);
    }

    #[test]
    fn exactly_max_payload_is_accepted() {
        let mut a = LineAssembler::new();
        let payload = std::iter::repeat_n(b'A', MAX_PAYLOAD).collect::<Vec<u8>>();
        let cs = checksum(&payload);
        let mut data = Vec::new();
        data.push(b'$');
        data.extend_from_slice(&payload);
        data.extend_from_slice(std::format!("*{cs:02X}").as_bytes());
        let out = feed(&mut a, &data);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_ok());
    }

    #[test]
    fn missing_checksum_line_is_dropped() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"$GNRMC,1,2,3\r\n$PQTMSAVEPAR*5A\r\n");
        assert_eq!(out.len(), 1);
        assert_eq!(a.stats().aborted, 1);
    }

    #[test]
    fn bad_hex_digit_aborts() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"$X*ZZ\r\n$PQTMSAVEPAR*5A");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn empty_payload_frame() {
        let mut a = LineAssembler::new();
        let out = feed(&mut a, b"$*00");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].as_deref().unwrap(), "");
    }

    #[test]
    fn sentence_fields() {
        let s = Sentence::new(b"GNRMC,1,,3");
        assert_eq!(s.address(), b"GNRMC");
        assert_eq!(s.field(1), Some(&b"1"[..]));
        assert_eq!(s.field(2), Some(&b""[..]));
        assert_eq!(s.field(3), Some(&b"3"[..]));
        assert_eq!(s.field(4), None);
        assert_eq!(s.field_count(), 4);
    }

    #[test]
    fn counters_saturate() {
        let mut a = LineAssembler::new();
        a.stats.checksum_errors = u32::MAX;
        let _ = feed(&mut a, b"$PQTMSAVEPAR*00");
        assert_eq!(a.stats().checksum_errors, u32::MAX);
    }
}
