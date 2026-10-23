//! Record schema and codec.
//!
//! Inside a data block payload, records are packed back to back as
//! `[kind: u8][len: u8][body: len bytes]`. All integers are little endian.
//! Decoders accept bodies longer than they need (newer firmware may append
//! fields) and reject bodies that are too short.

/// Largest record body.
pub const MAX_BODY: usize = 255;

/// Record kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// [`SessionInfo`], first record of every session.
    SessionInfo,
    /// [`Sample`], the periodic snapshot.
    Sample,
    /// [`CanRawBatch`].
    CanRaw,
    /// [`SignalsBatch`].
    Signals,
    /// [`Event`].
    Event,
    /// A kind this version does not know; kept so nothing is dropped.
    Unknown(u8),
}

impl Kind {
    /// Wire value.
    pub const fn code(self) -> u8 {
        match self {
            Self::SessionInfo => 1,
            Self::Sample => 2,
            Self::CanRaw => 3,
            Self::Signals => 4,
            Self::Event => 5,
            Self::Unknown(c) => c,
        }
    }

    /// From the wire value.
    pub const fn from_code(c: u8) -> Self {
        match c {
            1 => Self::SessionInfo,
            2 => Self::Sample,
            3 => Self::CanRaw,
            4 => Self::Signals,
            5 => Self::Event,
            other => Self::Unknown(other),
        }
    }
}

/// Decode failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The body is shorter than the record requires.
    Truncated,
    /// A field holds a value outside its legal range.
    BadValue,
}

/// Encode failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The output buffer is too small.
    BufferTooSmall,
}

struct Enc<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Enc<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }
    fn bytes(&mut self, b: &[u8]) -> Result<(), EncodeError> {
        let end = self.len.checked_add(b.len()).ok_or(EncodeError::BufferTooSmall)?;
        self.buf.get_mut(self.len..end).ok_or(EncodeError::BufferTooSmall)?.copy_from_slice(b);
        self.len = end;
        Ok(())
    }
    fn u8(&mut self, v: u8) -> Result<(), EncodeError> {
        self.bytes(&[v])
    }
    fn u16(&mut self, v: u16) -> Result<(), EncodeError> {
        self.bytes(&v.to_le_bytes())
    }
    fn i16(&mut self, v: i16) -> Result<(), EncodeError> {
        self.bytes(&v.to_le_bytes())
    }
    fn u32(&mut self, v: u32) -> Result<(), EncodeError> {
        self.bytes(&v.to_le_bytes())
    }
    fn i32(&mut self, v: i32) -> Result<(), EncodeError> {
        self.bytes(&v.to_le_bytes())
    }
    fn u64(&mut self, v: u64) -> Result<(), EncodeError> {
        self.bytes(&v.to_le_bytes())
    }
    fn i64(&mut self, v: i64) -> Result<(), EncodeError> {
        self.bytes(&v.to_le_bytes())
    }
}

#[derive(Clone)]
struct Dec<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Dec<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Truncated)?;
        let s = self.buf.get(self.pos..end).ok_or(DecodeError::Truncated)?;
        self.pos = end;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        self.take(N)?.try_into().map_err(|_| DecodeError::Truncated)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(u8::from_le_bytes(self.arr()?))
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    fn i16(&mut self) -> Result<i16, DecodeError> {
        Ok(i16::from_le_bytes(self.arr()?))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_le_bytes(self.arr()?))
    }
}

/// Schema version written by this crate.
pub const SCHEMA_VERSION: u16 = 1;

// ---------------------------------------------------------------- SessionInfo

/// Describes the session: written first, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// Schema version of the records that follow.
    pub schema: u16,
    /// Firmware version string, ASCII, zero padded.
    pub firmware: [u8; 16],
    /// GNSS fix interval in milliseconds.
    pub fix_interval_ms: u16,
    /// CAN1 bit rate in bit/s (0 if unknown).
    pub can1_bitrate: u32,
    /// CAN2 bit rate in bit/s (0 if unknown).
    pub can2_bitrate: u32,
    /// IMU output data rate in Hz.
    pub imu_odr_hz: u16,
    /// Monotonic microseconds at session start.
    pub t_us: u64,
    /// UTC at session start in Unix milliseconds, or 0 if not yet known.
    pub utc_unix_ms: i64,
}

impl SessionInfo {
    /// Encoded size.
    pub const LEN: usize = 2 + 16 + 2 + 4 + 4 + 2 + 8 + 8;

    /// Encode into `out`, returning the length.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut e = Enc::new(out);
        e.u16(self.schema)?;
        e.bytes(&self.firmware)?;
        e.u16(self.fix_interval_ms)?;
        e.u32(self.can1_bitrate)?;
        e.u32(self.can2_bitrate)?;
        e.u16(self.imu_odr_hz)?;
        e.u64(self.t_us)?;
        e.i64(self.utc_unix_ms)?;
        Ok(e.len)
    }

    /// Decode a body.
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let mut d = Dec::new(body);
        Ok(Self {
            schema: d.u16()?,
            firmware: d.arr()?,
            fix_interval_ms: d.u16()?,
            can1_bitrate: d.u32()?,
            can2_bitrate: d.u32()?,
            imu_odr_hz: d.u16()?,
            t_us: d.u64()?,
            utc_unix_ms: d.i64()?,
        })
    }
}

// ---------------------------------------------------------------- Sample

/// Bits of [`Sample::flags`].
pub mod flags {
    /// Position fields are valid.
    pub const GNSS_VALID: u32 = 1 << 0;
    /// Heading fields are valid.
    pub const HEADING_VALID: u32 = 1 << 1;
    /// `utc_unix_ms` is valid.
    pub const UTC_VALID: u32 = 1 << 2;
    /// IMU fields are valid.
    pub const IMU_VALID: u32 = 1 << 3;
    /// A CAN1 frame was received recently.
    pub const CAN1_ALIVE: u32 = 1 << 4;
    /// A CAN2 frame was received recently.
    pub const CAN2_ALIVE: u32 = 1 << 5;
    /// Logging started without a GNSS fix.
    pub const NO_FIX_START: u32 = 1 << 6;
    /// A storage fault occurred during this session.
    pub const CARD_FAULT: u32 = 1 << 7;
    /// A queue overran since the previous sample.
    pub const OVERRUN: u32 = 1 << 8;
    /// The PPS-disciplined clock is locked.
    pub const PPS_LOCKED: u32 = 1 << 9;
}

/// The periodic snapshot, one per GNSS fix. Invalid fields are zero and the
/// matching `flags` bit is clear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// Monotonic microseconds when the record was built.
    pub t_us: u64,
    /// UTC in Unix milliseconds (valid when `UTC_VALID`).
    pub utc_unix_ms: i64,
    /// See [`flags`].
    pub flags: u32,
    /// Latitude, 1e-9 degrees.
    pub lat_e9: i64,
    /// Longitude, 1e-9 degrees.
    pub lon_e9: i64,
    /// Altitude above mean sea level, millimetres.
    pub alt_mm: i32,
    /// Speed over ground, mm/s.
    pub sog_mm_s: u32,
    /// Course over ground, 0.01 degree.
    pub cog_cdeg: u16,
    /// GGA fix quality indicator.
    pub fix_quality: u8,
    /// Satellites used.
    pub num_sv: u8,
    /// HDOP in hundredths.
    pub hdop_e2: u16,
    /// Differential age in 0.1 s (0xFFFF if absent).
    pub diff_age_ds: u16,
    /// Dual-antenna heading, 1e-6 degrees.
    pub heading_e6: u32,
    /// Dual-antenna pitch, 1e-6 degrees.
    pub pitch_e6: i32,
    /// Baseline length, millimetres.
    pub baseline_mm: u32,
    /// Heading accuracy, 1e-6 degrees.
    pub acc_heading_e6: u32,
    /// Heading solution quality (`PQTMTAR` quality indicator).
    pub heading_quality: u8,
    /// Satellites used for heading.
    pub heading_sv: u8,
    /// Mean acceleration over the window, micro-g.
    pub accel_mean_ug: [i32; 3],
    /// Peak absolute acceleration over the window, micro-g.
    pub accel_peak_ug: [i32; 3],
    /// Mean angular rate, milli-degrees per second.
    pub gyro_mean_mdps: [i32; 3],
    /// Peak absolute angular rate, milli-degrees per second.
    pub gyro_peak_mdps: [i32; 3],
    /// IMU temperature, hundredths of a degree C.
    pub imu_temp_centi_c: i16,
    /// IMU samples in the window.
    pub imu_count: u16,
    /// Cumulative NMEA checksum errors.
    pub nmea_errors: u32,
    /// Cumulative queue overruns.
    pub overruns: u32,
    /// Cumulative CAN1 frames received.
    pub can1_frames: u32,
    /// Cumulative CAN2 frames received.
    pub can2_frames: u32,
}

impl Sample {
    /// Encoded size.
    pub const LEN: usize = 138;

    /// Encode into `out`, returning the length ([`Sample::LEN`]).
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut e = Enc::new(out);
        e.u64(self.t_us)?;
        e.i64(self.utc_unix_ms)?;
        e.u32(self.flags)?;
        e.i64(self.lat_e9)?;
        e.i64(self.lon_e9)?;
        e.i32(self.alt_mm)?;
        e.u32(self.sog_mm_s)?;
        e.u16(self.cog_cdeg)?;
        e.u8(self.fix_quality)?;
        e.u8(self.num_sv)?;
        e.u16(self.hdop_e2)?;
        e.u16(self.diff_age_ds)?;
        e.u32(self.heading_e6)?;
        e.i32(self.pitch_e6)?;
        e.u32(self.baseline_mm)?;
        e.u32(self.acc_heading_e6)?;
        e.u8(self.heading_quality)?;
        e.u8(self.heading_sv)?;
        for v in
            self.accel_mean_ug.iter().chain(&self.accel_peak_ug).chain(&self.gyro_mean_mdps).chain(&self.gyro_peak_mdps)
        {
            e.i32(*v)?;
        }
        e.i16(self.imu_temp_centi_c)?;
        e.u16(self.imu_count)?;
        e.u32(self.nmea_errors)?;
        e.u32(self.overruns)?;
        e.u32(self.can1_frames)?;
        e.u32(self.can2_frames)?;
        Ok(e.len)
    }

    /// Decode a body (extra trailing bytes are ignored).
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let mut d = Dec::new(body);
        let mut s = Self {
            t_us: d.u64()?,
            utc_unix_ms: d.i64()?,
            flags: d.u32()?,
            lat_e9: d.i64()?,
            lon_e9: d.i64()?,
            alt_mm: d.i32()?,
            sog_mm_s: d.u32()?,
            cog_cdeg: d.u16()?,
            fix_quality: d.u8()?,
            num_sv: d.u8()?,
            hdop_e2: d.u16()?,
            diff_age_ds: d.u16()?,
            heading_e6: d.u32()?,
            pitch_e6: d.i32()?,
            baseline_mm: d.u32()?,
            acc_heading_e6: d.u32()?,
            heading_quality: d.u8()?,
            heading_sv: d.u8()?,
            ..Self::default()
        };
        for arr in [&mut s.accel_mean_ug, &mut s.accel_peak_ug, &mut s.gyro_mean_mdps, &mut s.gyro_peak_mdps] {
            for v in arr.iter_mut() {
                *v = d.i32()?;
            }
        }
        s.imu_temp_centi_c = d.i16()?;
        s.imu_count = d.u16()?;
        s.nmea_errors = d.u32()?;
        s.overruns = d.u32()?;
        s.can1_frames = d.u32()?;
        s.can2_frames = d.u32()?;
        Ok(s)
    }
}

// ---------------------------------------------------------------- Event

/// A discrete happening worth recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// Monotonic microseconds.
    pub t_us: u64,
    /// Event code (see [`event`]).
    pub code: u16,
    /// Code-specific argument.
    pub arg: u32,
}

/// Event codes.
pub mod event {
    /// Firmware booted; `arg` is the reset-reason bit field.
    pub const BOOT: u16 = 1;
    /// GNSS module configuration was changed; `arg` is the new baud.
    pub const GNSS_RECONFIGURED: u16 = 2;
    /// A storage card faulted; `arg` is the card (1 or 2).
    pub const CARD_FAULT: u16 = 3;
    /// Logging moved to another card; `arg` is the new card.
    pub const CARD_SWITCH: u16 = 4;
    /// A queue overran; `arg` is the queue id.
    pub const QUEUE_OVERRUN: u16 = 5;
    /// A CAN controller went bus-off; `arg` is the bus (1 or 2).
    pub const CAN_BUS_OFF: u16 = 6;
    /// GNSS fix acquired.
    pub const FIX_ACQUIRED: u16 = 7;
    /// GNSS fix lost.
    pub const FIX_LOST: u16 = 8;
    /// The IMU reported a fault; `arg` is a driver error code.
    pub const IMU_FAULT: u16 = 9;
    /// The LED drivers needed re-initialisation.
    pub const LED_REINIT: u16 = 10;
}

impl Event {
    /// Encoded size.
    pub const LEN: usize = 14;

    /// Encode into `out`.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut e = Enc::new(out);
        e.u64(self.t_us)?;
        e.u16(self.code)?;
        e.u32(self.arg)?;
        Ok(e.len)
    }

    /// Decode a body.
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let mut d = Dec::new(body);
        Ok(Self { t_us: d.u64()?, code: d.u16()?, arg: d.u32()? })
    }
}

// ---------------------------------------------------------------- CAN raw

/// One received CAN frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanFrame {
    /// Receive time, monotonic microseconds.
    pub t_us: u64,
    /// Identifier (11 or 29 bit).
    pub id: u32,
    /// 29-bit identifier.
    pub extended: bool,
    /// Data length, 0 to 8.
    pub dlc: u8,
    /// Payload; bytes beyond `dlc` are zero.
    pub data: [u8; 8],
}

const EXT_BIT: u32 = 1 << 31;
const RAW_HEADER: usize = 1 + 1 + 8;

/// Packs raw CAN frames into one record body (at most [`MAX_BODY`] bytes).
#[derive(Clone, Debug)]
pub struct CanRawBatch {
    bus: u8,
    base_t_us: u64,
    count: u8,
    frames: [u8; MAX_BODY],
    len: usize,
}

impl CanRawBatch {
    /// An empty batch for `bus` (1 or 2).
    pub const fn new(bus: u8) -> Self {
        Self { bus, base_t_us: 0, count: 0, frames: [0; MAX_BODY], len: RAW_HEADER }
    }

    /// Number of frames held.
    pub const fn count(&self) -> u8 {
        self.count
    }

    /// True if no frame has been added.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Reset to empty (keeps the bus).
    pub fn clear(&mut self) {
        *self = Self::new(self.bus);
    }

    /// Try to add a frame. Returns `false` (and leaves the batch unchanged)
    /// when the frame is invalid, does not fit, or is more than 2^32 us away
    /// from the first frame; the caller should emit the batch and retry.
    pub fn try_push(&mut self, f: &CanFrame) -> bool {
        if f.dlc > 8 || f.id > 0x1FFF_FFFF || (!f.extended && f.id > 0x7FF) {
            return false;
        }
        let base = if self.count == 0 { f.t_us } else { self.base_t_us };
        let Some(dt) = f.t_us.checked_sub(base).and_then(|d| u32::try_from(d).ok()) else { return false };
        let need = 4 + 4 + 1 + usize::from(f.dlc);
        let Some(end) = self.len.checked_add(need) else { return false };
        if end > MAX_BODY || self.count == u8::MAX {
            return false;
        }
        let id = if f.extended { f.id | EXT_BIT } else { f.id };
        let mut e = Enc { buf: &mut self.frames, len: self.len };
        let ok = e.u32(dt).is_ok()
            && e.u32(id).is_ok()
            && e.u8(f.dlc).is_ok()
            && f.data.get(..usize::from(f.dlc)).is_some_and(|d| e.bytes(d).is_ok());
        if !ok {
            return false;
        }
        if self.count == 0 {
            self.base_t_us = base;
        }
        self.len = e.len;
        self.count = self.count.saturating_add(1);
        true
    }

    /// Encode the record body.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut e = Enc::new(out);
        e.u8(self.bus)?;
        e.u8(self.count)?;
        e.u64(self.base_t_us)?;
        let body = self.frames.get(RAW_HEADER..self.len).ok_or(EncodeError::BufferTooSmall)?;
        e.bytes(body)?;
        Ok(e.len)
    }
}

/// Decoded view of a raw CAN record.
#[derive(Clone, Copy, Debug)]
pub struct CanRawView<'a> {
    /// Bus number.
    pub bus: u8,
    /// Number of frames the record claims to hold.
    pub count: u8,
    base_t_us: u64,
    rest: &'a [u8],
}

impl<'a> CanRawView<'a> {
    /// Parse the header of a raw CAN record body.
    pub fn parse(body: &'a [u8]) -> Result<Self, DecodeError> {
        let mut d = Dec::new(body);
        let bus = d.u8()?;
        let count = d.u8()?;
        let base_t_us = d.u64()?;
        let rest = body.get(d.pos..).ok_or(DecodeError::Truncated)?;
        Ok(Self { bus, count, base_t_us, rest })
    }

    /// Iterate over the frames.
    pub fn frames(&self) -> CanRawFrames<'a> {
        CanRawFrames { d: Dec::new(self.rest), base_t_us: self.base_t_us, left: self.count }
    }
}

/// Iterator over the frames of a raw CAN record.
#[derive(Clone)]
pub struct CanRawFrames<'a> {
    d: Dec<'a>,
    base_t_us: u64,
    left: u8,
}

impl Iterator for CanRawFrames<'_> {
    type Item = Result<CanFrame, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.left == 0 {
            return None;
        }
        self.left = self.left.saturating_sub(1);
        let r = (|| {
            let dt = self.d.u32()?;
            let id_raw = self.d.u32()?;
            let dlc = self.d.u8()?;
            if dlc > 8 {
                return Err(DecodeError::BadValue);
            }
            let payload = self.d.take(usize::from(dlc))?;
            let mut data = [0_u8; 8];
            for (dst, src) in data.iter_mut().zip(payload) {
                *dst = *src;
            }
            let extended = id_raw & EXT_BIT != 0;
            Ok(CanFrame {
                t_us: self.base_t_us.checked_add(u64::from(dt)).ok_or(DecodeError::BadValue)?,
                id: id_raw & !EXT_BIT,
                extended,
                dlc,
                data,
            })
        })();
        if r.is_err() {
            self.left = 0; // stop after the first error
        }
        Some(r)
    }
}

// ---------------------------------------------------------------- decoded signals

const SIG_HEADER: usize = 1 + 8 + 1;
const SIG_ENTRY: usize = 2 + 8;
/// Most signals one [`SignalsBatch`] can hold.
pub const MAX_SIGNALS: usize = (MAX_BODY - SIG_HEADER) / SIG_ENTRY;

/// A batch of decoded CAN signals: `(catalog-wide signal id, raw bits)`.
#[derive(Clone, Debug)]
pub struct SignalsBatch {
    bus: u8,
    t_us: u64,
    entries: [(u16, u64); MAX_SIGNALS],
    count: usize,
}

impl SignalsBatch {
    /// An empty batch for `bus` stamped `t_us`.
    pub const fn new(bus: u8, t_us: u64) -> Self {
        Self { bus, t_us, entries: [(0, 0); MAX_SIGNALS], count: 0 }
    }

    /// Number of entries.
    pub const fn len(&self) -> usize {
        self.count
    }

    /// True if empty.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Add an entry; `false` when full.
    pub fn try_push(&mut self, signal_id: u16, raw: u64) -> bool {
        match self.entries.get_mut(self.count) {
            Some(slot) => {
                *slot = (signal_id, raw);
                self.count = self.count.saturating_add(1);
                true
            }
            None => false,
        }
    }

    /// Encode the record body.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut e = Enc::new(out);
        e.u8(self.bus)?;
        e.u64(self.t_us)?;
        e.u8(u8::try_from(self.count).map_err(|_| EncodeError::BufferTooSmall)?)?;
        for (id, raw) in self.entries.iter().take(self.count) {
            e.u16(*id)?;
            e.u64(*raw)?;
        }
        Ok(e.len)
    }
}

/// Decoded view of a signals record.
#[derive(Clone, Copy, Debug)]
pub struct SignalsView<'a> {
    /// Bus number.
    pub bus: u8,
    /// Timestamp, monotonic microseconds.
    pub t_us: u64,
    count: u8,
    rest: &'a [u8],
}

impl<'a> SignalsView<'a> {
    /// Parse a signals record body.
    pub fn parse(body: &'a [u8]) -> Result<Self, DecodeError> {
        let mut d = Dec::new(body);
        let bus = d.u8()?;
        let t_us = d.u64()?;
        let count = d.u8()?;
        let rest = body.get(d.pos..).ok_or(DecodeError::Truncated)?;
        if rest.len() < usize::from(count).saturating_mul(SIG_ENTRY) {
            return Err(DecodeError::Truncated);
        }
        Ok(Self { bus, t_us, count, rest })
    }

    /// Number of entries.
    pub const fn len(&self) -> usize {
        self.count as usize
    }

    /// True if there are no entries.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Iterate over `(signal id, raw bits)`.
    pub fn entries(&self) -> impl Iterator<Item = (u16, u64)> + 'a {
        let mut d = Dec::new(self.rest);
        (0..self.count).filter_map(move |_| {
            let id = d.u16().ok()?;
            let raw = d.u64().ok()?;
            Some((id, raw))
        })
    }
}

// ---------------------------------------------------------------- record stream

/// A framed record taken from a payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record<'a> {
    /// Kind.
    pub kind: Kind,
    /// Body bytes.
    pub body: &'a [u8],
}

/// Why walking a payload stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Truncated;

/// Iterates over the `[kind][len][body]` records of a payload. After the first
/// framing error it yields that error once and then ends.
#[derive(Clone)]
pub struct Records<'a> {
    rest: &'a [u8],
}

impl<'a> Records<'a> {
    /// Walk `payload`.
    pub const fn new(payload: &'a [u8]) -> Self {
        Self { rest: payload }
    }
}

impl<'a> Iterator for Records<'a> {
    type Item = Result<Record<'a>, Truncated>;

    fn next(&mut self) -> Option<Self::Item> {
        let (&kind, after_kind) = self.rest.split_first()?;
        let Some((&len, after_len)) = after_kind.split_first() else {
            self.rest = &[];
            return Some(Err(Truncated));
        };
        let len = usize::from(len);
        match (after_len.get(..len), after_len.get(len..)) {
            (Some(body), Some(rest)) => {
                self.rest = rest;
                Some(Ok(Record { kind: Kind::from_code(kind), body }))
            }
            _ => {
                self.rest = &[];
                Some(Err(Truncated))
            }
        }
    }
}

/// Append one framed record to `buf` at `*len`. Returns `false` (leaving
/// everything unchanged) if it does not fit.
pub fn append_record(buf: &mut [u8], len: &mut usize, kind: Kind, body: &[u8]) -> bool {
    let Ok(blen) = u8::try_from(body.len()) else { return false };
    let Some(total) = body.len().checked_add(2) else { return false };
    let Some(end) = len.checked_add(total) else { return false };
    let Some(dst) = buf.get_mut(*len..end) else { return false };
    let mut it = dst.iter_mut();
    if let (Some(a), Some(b)) = (it.next(), it.next()) {
        *a = kind.code();
        *b = blen;
    } else {
        return false;
    }
    if let Some(body_dst) = dst.get_mut(2..) {
        body_dst.copy_from_slice(body);
    }
    *len = end;
    true
}
