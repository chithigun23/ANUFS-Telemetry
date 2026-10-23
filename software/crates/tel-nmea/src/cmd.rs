//! Encoding of the Quectel PAIR and PQTM commands used to configure the
//! LC29H(EA). Output goes into a caller-provided buffer; nothing allocates.

use crate::framing::checksum;
use core::fmt::Write;

/// Longest command body this module will build.
const MAX_BODY: usize = 64;

/// UART baud rates accepted by `$PAIR864` on the LC29H(EA) (spec V1.5).
const BAUDS: [u32; 10] = [4_800, 9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600, 3_000_000];

/// Why a command could not be encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The output buffer cannot hold the encoded command.
    BufferTooSmall,
    /// An argument is outside the range the module accepts.
    InvalidArgument,
}

/// A command to send to the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// `$PAIR002`: power on the GNSS subsystem.
    Pair002,
    /// `$PAIR003`: power off the GNSS subsystem.
    Pair003,
    /// `$PAIR050,<ms>`: position fix interval, 100 to 1000 ms.
    Pair050 {
        /// Interval in milliseconds.
        interval_ms: u16,
    },
    /// `$PAIR051`: query the fix interval.
    Pair051,
    /// `$PAIR062,<type>,<rate>`: NMEA sentence output rate.
    Pair062 {
        /// Sentence type (0 = GGA, 4 = RMC, ...).
        kind: u8,
        /// Output once every `rate` fixes, 0 to 20.
        rate: u8,
    },
    /// `$PAIR382,<value>`: used before changing the fix rate.
    Pair382 {
        /// 1 to power the GNSS system down for reconfiguration.
        value: u8,
    },
    /// `$PAIR513`: save settings.
    Pair513,
    /// `$PAIR864,0,0,<baud>`: UART1 baud rate (applies after reboot).
    Pair864 {
        /// Baud rate.
        baud: u32,
    },
    /// `$PAIR865,0,0`: query the UART1 baud rate.
    Pair865,
    /// `$PQTMCFGMSGRATE,W,<name>,<rate>,<ver>`.
    PqtmCfgMsgRateSet {
        /// Message name, for example `PQTMTAR`.
        name: &'a str,
        /// 0 = disabled, 1 = every fix.
        rate: u8,
        /// Message version.
        version: u8,
    },
    /// `$PQTMCFGMSGRATE,R,<name>,<ver>`.
    PqtmCfgMsgRateGet {
        /// Message name.
        name: &'a str,
        /// Message version.
        version: u8,
    },
    /// `$PQTMSAVEPAR`: save parameters to flash.
    PqtmSavePar,
    /// `$PQTMVERNO`: firmware version query.
    PqtmVerNo,
}

/// Fixed-capacity writer that fails instead of panicking.
struct Body {
    buf: [u8; MAX_BODY],
    len: usize,
}

impl Write for Body {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(core::fmt::Error)?;
        let dst = self.buf.get_mut(self.len..end).ok_or(core::fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 16 && name.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl Command<'_> {
    /// The `$PAIR` command id this command is acknowledged with, if any.
    pub fn pair_id(&self) -> Option<u16> {
        match self {
            Self::Pair002 => Some(2),
            Self::Pair003 => Some(3),
            Self::Pair050 { .. } => Some(50),
            Self::Pair051 => Some(51),
            Self::Pair062 { .. } => Some(62),
            Self::Pair382 { .. } => Some(382),
            Self::Pair513 => Some(513),
            Self::Pair864 { .. } => Some(864),
            Self::Pair865 => Some(865),
            Self::PqtmCfgMsgRateSet { .. } | Self::PqtmCfgMsgRateGet { .. } | Self::PqtmSavePar | Self::PqtmVerNo => {
                None
            }
        }
    }

    fn write_body(&self, w: &mut Body) -> Result<(), EncodeError> {
        let r = match self {
            Self::Pair002 => w.write_str("PAIR002"),
            Self::Pair003 => w.write_str("PAIR003"),
            Self::Pair050 { interval_ms } => {
                if !(100..=1000).contains(interval_ms) {
                    return Err(EncodeError::InvalidArgument);
                }
                write!(w, "PAIR050,{interval_ms}")
            }
            Self::Pair051 => w.write_str("PAIR051"),
            Self::Pair062 { kind, rate } => {
                if *kind > 9 || *rate > 20 {
                    return Err(EncodeError::InvalidArgument);
                }
                write!(w, "PAIR062,{kind},{rate}")
            }
            Self::Pair382 { value } => {
                if *value > 1 {
                    return Err(EncodeError::InvalidArgument);
                }
                write!(w, "PAIR382,{value}")
            }
            Self::Pair513 => w.write_str("PAIR513"),
            Self::Pair864 { baud } => {
                if !BAUDS.contains(baud) {
                    return Err(EncodeError::InvalidArgument);
                }
                write!(w, "PAIR864,0,0,{baud}")
            }
            Self::Pair865 => w.write_str("PAIR865,0,0"),
            Self::PqtmCfgMsgRateSet { name, rate, version } => {
                if !valid_name(name) || *rate > 1 {
                    return Err(EncodeError::InvalidArgument);
                }
                write!(w, "PQTMCFGMSGRATE,W,{name},{rate},{version}")
            }
            Self::PqtmCfgMsgRateGet { name, version } => {
                if !valid_name(name) {
                    return Err(EncodeError::InvalidArgument);
                }
                write!(w, "PQTMCFGMSGRATE,R,{name},{version}")
            }
            Self::PqtmSavePar => w.write_str("PQTMSAVEPAR"),
            Self::PqtmVerNo => w.write_str("PQTMVERNO"),
        };
        r.map_err(|_| EncodeError::BufferTooSmall)
    }

    /// Encode as `$<body>*HH\r\n` into `out`, returning the length written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut body = Body { buf: [0; MAX_BODY], len: 0 };
        self.write_body(&mut body)?;
        let payload = body.buf.get(..body.len).ok_or(EncodeError::BufferTooSmall)?;
        let cs = checksum(payload);
        let mut tail = Body { buf: [0; MAX_BODY], len: 0 };
        write!(tail, "*{cs:02X}\r\n").map_err(|_| EncodeError::BufferTooSmall)?;
        let tail_bytes = tail.buf.get(..tail.len).ok_or(EncodeError::BufferTooSmall)?;
        let total = 1_usize
            .checked_add(payload.len())
            .and_then(|v| v.checked_add(tail_bytes.len()))
            .ok_or(EncodeError::BufferTooSmall)?;
        let dst = out.get_mut(..total).ok_or(EncodeError::BufferTooSmall)?;
        let (dollar, rest) = dst.split_first_mut().ok_or(EncodeError::BufferTooSmall)?;
        *dollar = b'$';
        let (pl, tl) = rest.split_at_mut_checked(payload.len()).ok_or(EncodeError::BufferTooSmall)?;
        pl.copy_from_slice(payload);
        tl.copy_from_slice(tail_bytes);
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;

    fn enc(c: Command<'_>) -> String {
        let mut buf = [0_u8; 96];
        let n = c.encode(&mut buf).unwrap();
        String::from_utf8(buf[..n].to_vec()).unwrap()
    }

    // Every vector below is printed in a Quectel document:
    // LC29H&LC79H Series GNSS Protocol Specification V1.5 and the
    // LC29H(EA) Moving Base Application Note V1.1.
    #[test]
    fn spec_checksum_vectors() {
        assert_eq!(enc(Command::Pair002), "$PAIR002*38\r\n");
        assert_eq!(enc(Command::Pair003), "$PAIR003*39\r\n");
        assert_eq!(enc(Command::Pair050 { interval_ms: 1000 }), "$PAIR050,1000*12\r\n");
        assert_eq!(enc(Command::Pair051), "$PAIR051*3E\r\n");
        assert_eq!(enc(Command::Pair062 { kind: 0, rate: 3 }), "$PAIR062,0,3*3D\r\n");
        assert_eq!(enc(Command::Pair382 { value: 1 }), "$PAIR382,1*2E\r\n");
        assert_eq!(enc(Command::Pair513), "$PAIR513*3D\r\n");
        assert_eq!(enc(Command::Pair864 { baud: 115_200 }), "$PAIR864,0,0,115200*1B\r\n");
        assert_eq!(
            enc(Command::PqtmCfgMsgRateSet { name: "PQTMTAR", rate: 1, version: 1 }),
            "$PQTMCFGMSGRATE,W,PQTMTAR,1,1*09\r\n"
        );
        assert_eq!(enc(Command::PqtmSavePar), "$PQTMSAVEPAR*5A\r\n");
        assert_eq!(enc(Command::PqtmVerNo), "$PQTMVERNO*58\r\n");
    }

    #[test]
    fn encoded_commands_pass_the_frame_assembler() {
        use crate::framing::LineAssembler;
        let cmds = [
            Command::Pair050 { interval_ms: 100 },
            Command::Pair864 { baud: 460_800 },
            Command::Pair865,
            Command::PqtmCfgMsgRateGet { name: "PQTMTAR", version: 1 },
        ];
        for c in cmds {
            let mut buf = [0_u8; 96];
            let n = c.encode(&mut buf).unwrap();
            let mut a = LineAssembler::new();
            let mut got = 0;
            for &b in &buf[..n] {
                if let Some(r) = a.push(b) {
                    assert!(r.is_ok());
                    got += 1;
                }
            }
            assert_eq!(got, 1, "{c:?}");
        }
    }

    #[test]
    fn argument_validation() {
        let mut b = [0_u8; 96];
        assert_eq!(Command::Pair050 { interval_ms: 99 }.encode(&mut b), Err(EncodeError::InvalidArgument));
        assert_eq!(Command::Pair050 { interval_ms: 1001 }.encode(&mut b), Err(EncodeError::InvalidArgument));
        assert_eq!(Command::Pair864 { baud: 123 }.encode(&mut b), Err(EncodeError::InvalidArgument));
        assert_eq!(Command::Pair062 { kind: 10, rate: 1 }.encode(&mut b), Err(EncodeError::InvalidArgument));
        assert_eq!(Command::Pair062 { kind: 0, rate: 21 }.encode(&mut b), Err(EncodeError::InvalidArgument));
        assert_eq!(Command::Pair382 { value: 2 }.encode(&mut b), Err(EncodeError::InvalidArgument));
        for bad in ["", "PQ TAR", "PQ,TAR", "PQ*TAR", "PQTMTARPQTMTARPQTMTAR", "PQTM$"] {
            assert_eq!(
                Command::PqtmCfgMsgRateGet { name: bad, version: 1 }.encode(&mut b),
                Err(EncodeError::InvalidArgument),
                "{bad:?}"
            );
        }
        assert_eq!(
            Command::PqtmCfgMsgRateSet { name: "PQTMTAR", rate: 2, version: 1 }.encode(&mut b),
            Err(EncodeError::InvalidArgument)
        );
    }

    #[test]
    fn buffer_too_small_never_panics() {
        let c = Command::PqtmCfgMsgRateSet { name: "PQTMTAR", rate: 1, version: 1 };
        let full = {
            let mut b = [0_u8; 96];
            c.encode(&mut b).unwrap()
        };
        for cap in 0..full {
            let mut b = std::vec![0_u8; cap];
            assert_eq!(c.encode(&mut b), Err(EncodeError::BufferTooSmall), "cap {cap}");
        }
        let mut b = std::vec![0_u8; full];
        assert_eq!(c.encode(&mut b), Ok(full));
    }

    #[test]
    fn all_bauds_encode() {
        for baud in BAUDS {
            let mut b = [0_u8; 96];
            assert!(Command::Pair864 { baud }.encode(&mut b).is_ok(), "{baud}");
        }
    }

    #[test]
    fn pair_ids() {
        assert_eq!(Command::Pair050 { interval_ms: 100 }.pair_id(), Some(50));
        assert_eq!(Command::PqtmSavePar.pair_id(), None);
    }
}
