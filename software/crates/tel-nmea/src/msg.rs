//! Typed messages decoded from verified sentences.
//!
//! All values are fixed-point integers; there is no floating point here so the
//! results are exact and identical on every target.

use crate::framing::Sentence;
use crate::num::{parse_coord, parse_fixed, parse_u32};

/// UTC time of day from a `hhmmss.sss` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UtcTime {
    /// Hour, 0 to 23.
    pub hour: u8,
    /// Minute, 0 to 59.
    pub minute: u8,
    /// Second, 0 to 60 (60 allowed for a leap second).
    pub second: u8,
    /// Milliseconds, 0 to 999.
    pub millis: u16,
}

/// Calendar date from a `ddmmyy` field (years 2000 to 2099).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    /// Day of month, 1 to 31.
    pub day: u8,
    /// Month, 1 to 12.
    pub month: u8,
    /// Four digit year.
    pub year: u16,
}

/// GNSS quality indicator (GGA and PQTMTAR).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixQuality {
    /// 0: no fix.
    Invalid,
    /// 1: standalone GNSS.
    Gps,
    /// 2: differential.
    Dgps,
    /// 4: RTK with fixed integers.
    RtkFixed,
    /// 5: RTK with floating integers.
    RtkFloat,
    /// 6: dead reckoning.
    DeadReckoning,
    /// 7: manual input.
    Manual,
    /// Any other value, kept so nothing is silently dropped.
    Other(u8),
}

impl FixQuality {
    /// Map the numeric indicator.
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Invalid,
            1 => Self::Gps,
            2 => Self::Dgps,
            4 => Self::RtkFixed,
            5 => Self::RtkFloat,
            6 => Self::DeadReckoning,
            7 => Self::Manual,
            other => Self::Other(other),
        }
    }

    /// Numeric indicator as on the wire.
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Invalid => 0,
            Self::Gps => 1,
            Self::Dgps => 2,
            Self::RtkFixed => 4,
            Self::RtkFloat => 5,
            Self::DeadReckoning => 6,
            Self::Manual => 7,
            Self::Other(v) => v,
        }
    }
}

/// RMC mode indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeIndicator {
    /// A: autonomous.
    Autonomous,
    /// D: differential.
    Differential,
    /// E: estimated.
    Estimated,
    /// F: float RTK.
    FloatRtk,
    /// M: manual.
    Manual,
    /// N: no fix.
    NoFix,
    /// R: fixed RTK.
    RtkFixed,
    /// Anything else.
    Other(u8),
}

impl ModeIndicator {
    fn from_byte(b: u8) -> Self {
        match b {
            b'A' => Self::Autonomous,
            b'D' => Self::Differential,
            b'E' => Self::Estimated,
            b'F' => Self::FloatRtk,
            b'M' => Self::Manual,
            b'N' => Self::NoFix,
            b'R' => Self::RtkFixed,
            other => Self::Other(other),
        }
    }
}

/// Recommended minimum data (RMC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rmc {
    /// UTC time of the fix, if present.
    pub time: Option<UtcTime>,
    /// Status field was `A` (data valid).
    pub valid: bool,
    /// Latitude in 1e-9 degrees, north positive.
    pub lat_e9: Option<i64>,
    /// Longitude in 1e-9 degrees, east positive.
    pub lon_e9: Option<i64>,
    /// Speed over ground in mm/s (truncated).
    pub sog_mm_s: Option<u32>,
    /// Course over ground in 0.01 degree units, 0 to 35999.
    pub cog_cdeg: Option<u16>,
    /// UTC date, if present.
    pub date: Option<Date>,
    /// Mode indicator, if present.
    pub mode: Option<ModeIndicator>,
}

/// Fix data (GGA).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gga {
    /// UTC time of the fix, if present.
    pub time: Option<UtcTime>,
    /// Latitude in 1e-9 degrees, north positive.
    pub lat_e9: Option<i64>,
    /// Longitude in 1e-9 degrees, east positive.
    pub lon_e9: Option<i64>,
    /// Fix quality indicator.
    pub quality: FixQuality,
    /// Satellites used.
    pub num_sv: u8,
    /// HDOP in hundredths.
    pub hdop_e2: Option<u16>,
    /// Altitude above mean sea level in millimetres.
    pub alt_mm: Option<i32>,
    /// Geoid separation in millimetres.
    pub geoid_sep_mm: Option<i32>,
    /// Age of differential data in 0.1 s units.
    pub diff_age_ds: Option<u32>,
}

/// Dual-antenna attitude (`$PQTMTAR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tar {
    /// Message version (1).
    pub version: u8,
    /// UTC time, if present.
    pub time: Option<UtcTime>,
    /// Solution quality.
    pub quality: FixQuality,
    /// Baseline length in millimetres.
    pub baseline_mm: Option<u32>,
    /// Pitch in 1e-6 degrees, -90 to 90 degrees.
    pub pitch_e6: Option<i32>,
    /// Heading in 1e-6 degrees, 0 to 359.999999 degrees.
    pub heading_e6: Option<u32>,
    /// Pitch accuracy in 1e-6 degrees.
    pub acc_pitch_e6: Option<u32>,
    /// Heading accuracy in 1e-6 degrees.
    pub acc_heading_e6: Option<u32>,
    /// Satellites used in the heading solution.
    pub used_sv: Option<u8>,
}

/// Which PQTM command a response belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PqtmCommand {
    /// `$PQTMCFGMSGRATE`.
    CfgMsgRate,
    /// `$PQTMSAVEPAR`.
    SavePar,
}

/// A fixed-capacity message name (for example `PQTMTAR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsgName {
    bytes: [u8; 16],
    len: u8,
}

impl MsgName {
    pub(crate) fn from_slice(s: &[u8]) -> Option<Self> {
        if s.is_empty() || s.len() > 16 {
            return None;
        }
        let mut bytes = [0_u8; 16];
        for (dst, src) in bytes.iter_mut().zip(s) {
            *dst = *src;
        }
        Some(Self { bytes, len: u8::try_from(s.len()).ok()? })
    }

    /// The name bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

/// Result part of a PQTM response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PqtmResult {
    /// Plain `OK`.
    Ok,
    /// `OK` to a message-rate query, with the reported settings.
    OkMsgRate {
        /// Message the rate applies to.
        name: MsgName,
        /// Output rate (0 = disabled, 1 = every fix).
        rate: u8,
        /// Message version.
        version: u8,
    },
    /// `ERROR,<code>`; 1 = invalid parameters, 2 = execution failed.
    Error(u8),
}

/// A decoded message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// `xxRMC`.
    Rmc(Rmc),
    /// `xxGGA`.
    Gga(Gga),
    /// `$PQTMTAR`.
    Tar(Tar),
    /// `$PAIR001` acknowledgement.
    PairAck {
        /// Command being acknowledged.
        command_id: u16,
        /// 0 = sent OK, 1 = processing, 2 = failed, 3 = unsupported,
        /// 4 = parameter error, 5 = busy.
        result: u8,
    },
    /// `$PAIR051,<ms>` fix interval report.
    FixRate {
        /// Position fix interval in milliseconds.
        interval_ms: u16,
    },
    /// `$PAIR865,<baud>` UART baud report.
    Baud {
        /// Baud rate.
        baud: u32,
    },
    /// Response to a PQTM command.
    Pqtm {
        /// Which command.
        command: PqtmCommand,
        /// Outcome.
        result: PqtmResult,
    },
    /// A checksum-valid sentence this crate does not decode.
    Unknown,
}

/// Why a sentence could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer fields than the message requires.
    TooFewFields,
    /// A field was present but malformed or out of range.
    BadField(&'static str),
}

type Res<T> = Result<T, ParseError>;

fn bad<T>(name: &'static str) -> Res<T> {
    Err(ParseError::BadField(name))
}

fn req<'a>(s: &Sentence<'a>, n: usize) -> Res<&'a [u8]> {
    s.field(n).ok_or(ParseError::TooFewFields)
}

fn two_digits(b: &[u8]) -> Option<u8> {
    let hi = crate::num::digit(*b.first()?)?;
    let lo = crate::num::digit(*b.get(1)?)?;
    hi.checked_mul(10)?.checked_add(lo)
}

/// Parse the digits after the decimal point into milliseconds (extra digits are
/// truncated, missing digits are zero).
fn parse_millis(frac: &[u8]) -> Option<u16> {
    let mut value: u16 = 0;
    let mut taken: u8 = 0;
    for &b in frac {
        let d = u16::from(crate::num::digit(b)?);
        if taken < 3 {
            value = value.checked_mul(10)?.checked_add(d)?;
            taken = taken.checked_add(1)?;
        }
    }
    while taken < 3 {
        value = value.checked_mul(10)?;
        taken = taken.checked_add(1)?;
    }
    Some(value)
}

fn parse_time(field: &[u8]) -> Res<Option<UtcTime>> {
    if field.is_empty() {
        return Ok(None);
    }
    let Some(hms) = field.get(..6) else { return bad("time") };
    let rest = field.get(6..).unwrap_or(&[]);
    let (Some(h), Some(m), Some(s)) =
        (two_digits(hms), two_digits(hms.get(2..).unwrap_or(&[])), two_digits(hms.get(4..).unwrap_or(&[])))
    else {
        return bad("time");
    };
    if h > 23 || m > 59 || s > 60 {
        return bad("time");
    }
    let millis = match rest.split_first() {
        None => 0,
        Some((&b'.', frac)) => parse_millis(frac).ok_or(ParseError::BadField("time"))?,
        Some(_) => return bad("time"),
    };
    Ok(Some(UtcTime { hour: h, minute: m, second: s, millis }))
}

fn parse_date(field: &[u8]) -> Res<Option<Date>> {
    if field.is_empty() {
        return Ok(None);
    }
    if field.len() != 6 {
        return bad("date");
    }
    let (Some(d), Some(m), Some(y)) =
        (two_digits(field), two_digits(field.get(2..).unwrap_or(&[])), two_digits(field.get(4..).unwrap_or(&[])))
    else {
        return bad("date");
    };
    if !(1..=31).contains(&d) || !(1..=12).contains(&m) {
        return bad("date");
    }
    Ok(Some(Date { day: d, month: m, year: 2000_u16.saturating_add(u16::from(y)) }))
}

/// Parse a lat/lon pair. Both must be present to produce a position; a lone
/// coordinate or missing hemisphere yields `None` for both.
fn parse_position(lat: &[u8], ns: &[u8], lon: &[u8], ew: &[u8]) -> Res<(Option<i64>, Option<i64>)> {
    if lat.is_empty() || lon.is_empty() {
        return Ok((None, None));
    }
    let Some(lat_mag) = parse_coord(lat, 2, 90) else { return bad("lat") };
    let Some(lon_mag) = parse_coord(lon, 3, 180) else { return bad("lon") };
    let lat_v = match ns {
        b"N" => lat_mag,
        b"S" => lat_mag.checked_neg().ok_or(ParseError::BadField("lat"))?,
        _ => return bad("n/s"),
    };
    let lon_v = match ew {
        b"E" => lon_mag,
        b"W" => lon_mag.checked_neg().ok_or(ParseError::BadField("lon"))?,
        _ => return bad("e/w"),
    };
    Ok((Some(lat_v), Some(lon_v)))
}

fn opt_fixed(field: &[u8], scale: u32, name: &'static str) -> Res<Option<i64>> {
    if field.is_empty() {
        Ok(None)
    } else {
        parse_fixed(field, scale).map(Some).ok_or(ParseError::BadField(name))
    }
}

fn opt_u32_fixed(field: &[u8], scale: u32, name: &'static str) -> Res<Option<u32>> {
    match opt_fixed(field, scale, name)? {
        None => Ok(None),
        Some(v) => u32::try_from(v).map(Some).map_err(|_| ParseError::BadField(name)),
    }
}

fn is_talker_sentence(addr: &[u8], kind: &[u8; 3]) -> bool {
    addr.len() == 5
        && addr.get(..2).is_some_and(|t| t.iter().all(u8::is_ascii_uppercase))
        && addr.get(2..).is_some_and(|k| k == kind)
}

fn parse_rmc(s: &Sentence<'_>) -> Res<Rmc> {
    let time = parse_time(req(s, 1)?)?;
    let valid = match req(s, 2)? {
        b"A" => true,
        b"V" => false,
        _ => return bad("status"),
    };
    let (lat_e9, lon_e9) = parse_position(req(s, 3)?, req(s, 4)?, req(s, 5)?, req(s, 6)?)?;
    let sog_mm_s = match opt_fixed(req(s, 7)?, 3, "sog")? {
        None => None,
        Some(knots_e3) => {
            let mm_s = u64::try_from(knots_e3)
                .ok()
                .and_then(|k| k.checked_mul(514_444))
                .and_then(|v| v.checked_div(1_000_000))
                .and_then(|v| u32::try_from(v).ok());
            match mm_s {
                Some(v) => Some(v),
                None => return bad("sog"),
            }
        }
    };
    let cog_cdeg = match opt_fixed(req(s, 8)?, 2, "cog")? {
        None => None,
        Some(v) => match u16::try_from(v) {
            Ok(c) if c <= 35_999 => Some(c),
            _ => return bad("cog"),
        },
    };
    let date = parse_date(req(s, 9)?)?;
    // Fields 10 and 11 (magnetic variation) are unsupported by the module.
    let mode = s.field(12).and_then(|f| f.first().copied()).map(ModeIndicator::from_byte);
    Ok(Rmc { time, valid, lat_e9, lon_e9, sog_mm_s, cog_cdeg, date, mode })
}

fn parse_gga(s: &Sentence<'_>) -> Res<Gga> {
    let time = parse_time(req(s, 1)?)?;
    let (lat_e9, lon_e9) = parse_position(req(s, 2)?, req(s, 3)?, req(s, 4)?, req(s, 5)?)?;
    let q = parse_u32(req(s, 6)?).and_then(|v| u8::try_from(v).ok());
    let Some(q) = q else { return bad("quality") };
    let num_sv = match parse_u32(req(s, 7)?).and_then(|v| u8::try_from(v).ok()) {
        Some(n) => n,
        None => return bad("num_sv"),
    };
    let hdop_e2 = match opt_fixed(req(s, 8)?, 2, "hdop")? {
        None => None,
        Some(v) => Some(u16::try_from(v).map_err(|_| ParseError::BadField("hdop"))?),
    };
    let alt_mm = match opt_fixed(req(s, 9)?, 3, "alt")? {
        None => None,
        Some(v) => Some(i32::try_from(v).map_err(|_| ParseError::BadField("alt"))?),
    };
    // Field 10 is the altitude unit.
    let geoid_sep_mm = match opt_fixed(req(s, 11)?, 3, "geoid")? {
        None => None,
        Some(v) => Some(i32::try_from(v).map_err(|_| ParseError::BadField("geoid"))?),
    };
    // Field 12 is the separation unit.
    let diff_age_ds = opt_u32_fixed(s.field(13).unwrap_or(&[]), 1, "diff_age")?;
    Ok(Gga {
        time,
        lat_e9,
        lon_e9,
        quality: FixQuality::from_u8(q),
        num_sv,
        hdop_e2,
        alt_mm,
        geoid_sep_mm,
        diff_age_ds,
    })
}

fn parse_tar(s: &Sentence<'_>) -> Res<Tar> {
    let version = match parse_u32(req(s, 1)?).and_then(|v| u8::try_from(v).ok()) {
        Some(v) => v,
        None => return bad("version"),
    };
    let time = parse_time(req(s, 2)?)?;
    let q = match parse_u32(req(s, 3)?).and_then(|v| u8::try_from(v).ok()) {
        Some(v) => v,
        None => return bad("quality"),
    };
    // Field 4 is reserved.
    let baseline_mm = opt_u32_fixed(req(s, 5)?, 3, "length")?;
    let pitch_e6 = match opt_fixed(req(s, 6)?, 6, "pitch")? {
        None => None,
        Some(v) => match i32::try_from(v) {
            Ok(p) if (-90_000_000..=90_000_000).contains(&p) => Some(p),
            _ => return bad("pitch"),
        },
    };
    // Field 7 is reserved.
    let heading_e6 = match opt_u32_fixed(req(s, 8)?, 6, "heading")? {
        None => None,
        Some(h) if h < 360_000_000 => Some(h),
        Some(_) => return bad("heading"),
    };
    let acc_pitch_e6 = opt_u32_fixed(req(s, 9)?, 6, "acc_pitch")?;
    // Field 10 is reserved.
    let acc_heading_e6 = opt_u32_fixed(req(s, 11)?, 6, "acc_heading")?;
    let used_sv = {
        let f = req(s, 12)?;
        if f.is_empty() {
            None
        } else {
            match parse_u32(f).and_then(|v| u8::try_from(v).ok()) {
                Some(v) => Some(v),
                None => return bad("used_sv"),
            }
        }
    };
    Ok(Tar {
        version,
        time,
        quality: FixQuality::from_u8(q),
        baseline_mm,
        pitch_e6,
        heading_e6,
        acc_pitch_e6,
        acc_heading_e6,
        used_sv,
    })
}

fn parse_pqtm(command: PqtmCommand, s: &Sentence<'_>) -> Res<Message> {
    let result = match req(s, 1)? {
        b"OK" => match (command, s.field(2)) {
            (PqtmCommand::CfgMsgRate, Some(name)) if !name.is_empty() => {
                let name = MsgName::from_slice(name).ok_or(ParseError::BadField("name"))?;
                let rate = parse_u32(req(s, 3)?).and_then(|v| u8::try_from(v).ok());
                let version = parse_u32(req(s, 4)?).and_then(|v| u8::try_from(v).ok());
                match (rate, version) {
                    (Some(rate), Some(version)) => PqtmResult::OkMsgRate { name, rate, version },
                    _ => return bad("rate"),
                }
            }
            _ => PqtmResult::Ok,
        },
        b"ERROR" => match parse_u32(req(s, 2)?).and_then(|v| u8::try_from(v).ok()) {
            Some(code) => PqtmResult::Error(code),
            None => return bad("error code"),
        },
        _ => return bad("result"),
    };
    Ok(Message::Pqtm { command, result })
}

/// Decode a verified sentence. Unknown but well-formed sentences return
/// [`Message::Unknown`]; known sentences that are malformed return an error.
pub fn parse(s: &Sentence<'_>) -> Result<Message, ParseError> {
    let addr = s.address();
    if is_talker_sentence(addr, b"RMC") {
        return parse_rmc(s).map(Message::Rmc);
    }
    if is_talker_sentence(addr, b"GGA") {
        return parse_gga(s).map(Message::Gga);
    }
    match addr {
        b"PQTMTAR" => parse_tar(s).map(Message::Tar),
        b"PAIR001" => {
            let id = parse_u32(req(s, 1)?).and_then(|v| u16::try_from(v).ok());
            let res = parse_u32(req(s, 2)?).and_then(|v| u8::try_from(v).ok());
            match (id, res) {
                (Some(command_id), Some(result)) => Ok(Message::PairAck { command_id, result }),
                _ => bad("pair001"),
            }
        }
        b"PAIR051" => match parse_u32(req(s, 1)?).and_then(|v| u16::try_from(v).ok()) {
            Some(interval_ms) => Ok(Message::FixRate { interval_ms }),
            None => bad("fix interval"),
        },
        b"PAIR865" => match parse_u32(req(s, 1)?) {
            Some(baud) => Ok(Message::Baud { baud }),
            None => bad("baud"),
        },
        b"PQTMCFGMSGRATE" => parse_pqtm(PqtmCommand::CfgMsgRate, s),
        b"PQTMSAVEPAR" => parse_pqtm(PqtmCommand::SavePar, s),
        _ => Ok(Message::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(payload: &[u8]) -> Result<Message, ParseError> {
        parse(&Sentence::new(payload))
    }

    #[test]
    fn rmc_v301_spec_example() {
        let m = p(b"GNRMC,093423.000,A,3149.332006,N,11706.913200,E,0.01,0.00,140122,,,A").unwrap();
        let Message::Rmc(r) = m else { panic!("not rmc") };
        assert_eq!(r.time, Some(UtcTime { hour: 9, minute: 34, second: 23, millis: 0 }));
        assert!(r.valid);
        assert_eq!(r.lat_e9, Some(31_822_200_100));
        assert_eq!(r.lon_e9, Some(117_115_220_000));
        assert_eq!(r.sog_mm_s, Some(5));
        assert_eq!(r.cog_cdeg, Some(0));
        assert_eq!(r.date, Some(Date { day: 14, month: 1, year: 2022 }));
        assert_eq!(r.mode, Some(ModeIndicator::Autonomous));
    }

    #[test]
    fn rmc_v410_spec_example() {
        let m = p(b"GNRMC,093316.000,A,3149.332558,N,11706.912570,E,0.00,237.67,140122,,,A,V").unwrap();
        let Message::Rmc(r) = m else { panic!("not rmc") };
        assert_eq!(r.cog_cdeg, Some(23_767));
        assert_eq!(r.sog_mm_s, Some(0));
    }

    #[test]
    fn rmc_no_fix_has_empty_fields() {
        let m = p(b"GNRMC,000000.000,V,,,,,,,,,,N,V").unwrap();
        let Message::Rmc(r) = m else { panic!("not rmc") };
        assert!(!r.valid);
        assert_eq!(r.lat_e9, None);
        assert_eq!(r.lon_e9, None);
        assert_eq!(r.sog_mm_s, None);
        assert_eq!(r.date, None);
        assert_eq!(r.mode, Some(ModeIndicator::NoFix));
    }

    #[test]
    fn rmc_southern_western_hemisphere() {
        let m = p(b"GPRMC,010203.5,A,3508.000000,S,14907.500000,W,1.00,90.00,010125,,,A").unwrap();
        let Message::Rmc(r) = m else { panic!("not rmc") };
        assert_eq!(r.lat_e9, Some(-35_133_333_333));
        assert_eq!(r.lon_e9, Some(-149_125_000_000));
        assert_eq!(r.time.unwrap().millis, 500);
        assert_eq!(r.sog_mm_s, Some(514));
    }

    #[test]
    fn rmc_speed_conversion_is_exact() {
        // 10.00 knots = 5.14444 m/s = 5144 mm/s (truncated)
        let m = p(b"GNRMC,093423.000,A,3149.332006,N,11706.913200,E,10.00,0.00,140122,,,A").unwrap();
        let Message::Rmc(r) = m else { panic!("not rmc") };
        assert_eq!(r.sog_mm_s, Some(5_144));
        // 100.00 knots = 51.4444 m/s
        let m = p(b"GNRMC,093423.000,A,3149.332006,N,11706.913200,E,100.00,0.00,140122,,,A").unwrap();
        let Message::Rmc(r) = m else { panic!("not rmc") };
        assert_eq!(r.sog_mm_s, Some(51_444));
    }

    #[test]
    fn gga_spec_examples() {
        let m = p(b"GNGGA,093423.000,3149.332006,N,11706.913200,E,1,22,0.75,45.346,M,-0.337,M,,").unwrap();
        let Message::Gga(g) = m else { panic!("not gga") };
        assert_eq!(g.quality, FixQuality::Gps);
        assert_eq!(g.num_sv, 22);
        assert_eq!(g.hdop_e2, Some(75));
        assert_eq!(g.alt_mm, Some(45_346));
        assert_eq!(g.geoid_sep_mm, Some(-337));
        assert_eq!(g.diff_age_ds, None);

        let m = p(b"GNGGA,093316.000,3149.332558,N,11706.912570,E,4,28,0.67,54.481,M,-0.337,M,1.2,0000").unwrap();
        let Message::Gga(g) = m else { panic!("not gga") };
        assert_eq!(g.quality, FixQuality::RtkFixed);
        assert_eq!(g.diff_age_ds, Some(12));
    }

    #[test]
    fn tar_spec_example() {
        let m = p(b"PQTMTAR,1,165034.000,4,,0.860,1.124780,,50.968541,0.254125,,0.012547,21").unwrap();
        let Message::Tar(t) = m else { panic!("not tar") };
        assert_eq!(t.version, 1);
        assert_eq!(t.time, Some(UtcTime { hour: 16, minute: 50, second: 34, millis: 0 }));
        assert_eq!(t.quality, FixQuality::RtkFixed);
        assert_eq!(t.baseline_mm, Some(860));
        assert_eq!(t.pitch_e6, Some(1_124_780));
        assert_eq!(t.heading_e6, Some(50_968_541));
        assert_eq!(t.acc_pitch_e6, Some(254_125));
        assert_eq!(t.acc_heading_e6, Some(12_547));
        assert_eq!(t.used_sv, Some(21));
    }

    #[test]
    fn tar_invalid_values_are_null() {
        let m = p(b"PQTMTAR,1,165034.000,0,,,,,,,,,0").unwrap();
        let Message::Tar(t) = m else { panic!("not tar") };
        assert_eq!(t.quality, FixQuality::Invalid);
        assert_eq!(t.heading_e6, None);
        assert_eq!(t.pitch_e6, None);
        assert_eq!(t.baseline_mm, None);
        assert_eq!(t.used_sv, Some(0));
    }

    #[test]
    fn tar_rejects_out_of_range() {
        assert!(p(b"PQTMTAR,1,165034.000,4,,0.860,91.0,,50.9,0.2,,0.01,21").is_err());
        assert!(p(b"PQTMTAR,1,165034.000,4,,0.860,1.0,,360.000000,0.2,,0.01,21").is_err());
        assert!(p(b"PQTMTAR,1,165034.000,4,,-0.860,1.0,,50.0,0.2,,0.01,21").is_err());
    }

    #[test]
    fn tar_too_few_fields() {
        assert_eq!(p(b"PQTMTAR,1,165034.000,4,,0.860"), Err(ParseError::TooFewFields));
    }

    #[test]
    fn pair_ack_and_queries() {
        assert_eq!(p(b"PAIR001,004,0"), Ok(Message::PairAck { command_id: 4, result: 0 }));
        assert_eq!(p(b"PAIR001,050,4"), Ok(Message::PairAck { command_id: 50, result: 4 }));
        assert_eq!(p(b"PAIR051,100"), Ok(Message::FixRate { interval_ms: 100 }));
        assert_eq!(p(b"PAIR865,460800"), Ok(Message::Baud { baud: 460_800 }));
        assert!(p(b"PAIR001,xx,0").is_err());
    }

    #[test]
    fn pqtm_responses() {
        assert_eq!(p(b"PQTMSAVEPAR,OK"), Ok(Message::Pqtm { command: PqtmCommand::SavePar, result: PqtmResult::Ok }));
        assert_eq!(
            p(b"PQTMCFGMSGRATE,OK"),
            Ok(Message::Pqtm { command: PqtmCommand::CfgMsgRate, result: PqtmResult::Ok })
        );
        assert_eq!(
            p(b"PQTMCFGMSGRATE,ERROR,1"),
            Ok(Message::Pqtm { command: PqtmCommand::CfgMsgRate, result: PqtmResult::Error(1) })
        );
        let Ok(Message::Pqtm { result: PqtmResult::OkMsgRate { name, rate, version }, .. }) =
            p(b"PQTMCFGMSGRATE,OK,PQTMTAR,1,1")
        else {
            panic!("not msgrate");
        };
        assert_eq!(name.as_bytes(), b"PQTMTAR");
        assert_eq!((rate, version), (1, 1));
        assert!(p(b"PQTMSAVEPAR,MAYBE").is_err());
    }

    #[test]
    fn unknown_sentences_pass_through() {
        assert_eq!(p(b"GNVTG,0.00,T,,M,0.01,N,0.02,K,A"), Ok(Message::Unknown));
        assert_eq!(p(b""), Ok(Message::Unknown));
        assert_eq!(p(b"GNGSV,1,1,01"), Ok(Message::Unknown));
    }

    #[test]
    fn malformed_known_sentences_error_not_panic() {
        assert!(p(b"GNRMC").is_err());
        assert!(p(b"GNRMC,093423.000,X,,,,,,,,,,").is_err()); // bad status
        assert!(p(b"GNRMC,25,A,,,,,,,,,,").is_err()); // bad time
        assert!(p(b"GNRMC,093423.000,A,9999.0,N,11706.9,E,,,,,,").is_err()); // 99 degrees
        assert!(p(b"GNGGA,093423.000,3149.332006,N,11706.913200,E,x,22,0.75,45.346,M,-0.337,M,,").is_err());
        assert!(p(b"GNGGA,093423.000,3149.332006,Q,11706.913200,E,1,22,0.75,45.346,M,-0.337,M,,").is_err());
    }

    #[test]
    fn time_validation() {
        assert_eq!(parse_time(b"235959.999"), Ok(Some(UtcTime { hour: 23, minute: 59, second: 59, millis: 999 })));
        assert_eq!(parse_time(b"235960"), Ok(Some(UtcTime { hour: 23, minute: 59, second: 60, millis: 0 })));
        assert_eq!(parse_time(b""), Ok(None));
        assert!(parse_time(b"240000").is_err());
        assert!(parse_time(b"006000").is_err());
        assert!(parse_time(b"000061").is_err());
        assert!(parse_time(b"12345").is_err());
        assert!(parse_time(b"123456x").is_err());
        assert!(parse_time(b"123456.").is_ok());
        assert_eq!(parse_time(b"123456.1234").unwrap().unwrap().millis, 123); // truncated to ms
        assert_eq!(parse_time(b"123456.05").unwrap().unwrap().millis, 50);
        assert_eq!(parse_time(b"123456.007").unwrap().unwrap().millis, 7);
    }

    #[test]
    fn date_validation() {
        assert_eq!(parse_date(b"311299"), Ok(Some(Date { day: 31, month: 12, year: 2099 })));
        assert!(parse_date(b"000199").is_err());
        assert!(parse_date(b"321299").is_err());
        assert!(parse_date(b"011399").is_err());
        assert!(parse_date(b"0101").is_err());
    }

    #[test]
    fn fix_quality_roundtrip() {
        for v in 0..=255_u8 {
            assert_eq!(FixQuality::from_u8(v).as_u8(), v);
        }
    }
}
