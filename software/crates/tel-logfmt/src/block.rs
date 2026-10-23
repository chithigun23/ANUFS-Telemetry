//! On-card block layouts. Every block is 512 bytes and ends with a CRC-32 of
//! the preceding 508 bytes, so a torn or corrupted block is always detected.
//!
//! Data block (`TLB1`), all integers little endian:
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `TLB1` |
//! | 4 | 4 | `card_id` (random per prepared card; rejects stale data) |
//! | 8 | 4 | `session` (starts at 1) |
//! | 12 | 4 | `seq` (block index inside the session, starts at 0) |
//! | 16 | 2 | `used` payload bytes |
//! | 18 | 2 | `flags` (bit 0 FIRST, bit 1 LAST) |
//! | 20 | 488 | payload (unused bytes zero) |
//! | 508 | 4 | CRC-32 of bytes 0..508 |
//!
//! Region header (`TLH1`, region block 0) and hint (`TLT1`, region blocks 1
//! and 2, written alternately) use the same trailing CRC.

use crate::crc::crc32;

/// Size of every block.
pub const BLOCK_SIZE: usize = 512;
/// Offset of the CRC.
pub const CRC_OFFSET: usize = BLOCK_SIZE - 4;
/// Bytes before the data block payload.
pub const DATA_HEADER_LEN: usize = 20;
/// Most payload bytes a data block can carry.
pub const PAYLOAD_MAX: usize = CRC_OFFSET - DATA_HEADER_LEN;
/// Region-relative block number of the region header.
pub const HEADER_LBA: u32 = 0;
/// Region-relative block numbers of the two hint slots.
pub const HINT_LBA: [u32; 2] = [1, 2];
/// Region-relative block number of the first data block.
pub const DATA_START: u32 = 3;
/// On-card format version.
pub const FORMAT_VERSION: u16 = 1;

const MAGIC_DATA: [u8; 4] = *b"TLB1";
const MAGIC_REGION: [u8; 4] = *b"TLH1";
const MAGIC_HINT: [u8; 4] = *b"TLT1";

/// Flag: first block of a session.
pub const FLAG_FIRST: u16 = 1 << 0;
/// Flag: the session was closed cleanly by the writer.
pub const FLAG_LAST: u16 = 1 << 1;

/// A whole block.
pub type Block = [u8; BLOCK_SIZE];

/// Why a block was rejected or could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockError {
    /// The magic bytes are wrong for this block type.
    BadMagic,
    /// The CRC does not match (torn write, bit rot, or never written).
    BadCrc,
    /// A length field is larger than the block allows.
    BadLength,
    /// The format version is not supported.
    BadVersion,
    /// The payload does not fit.
    TooLarge,
}

fn put(buf: &mut Block, at: usize, bytes: &[u8]) -> Option<()> {
    let end = at.checked_add(bytes.len())?;
    buf.get_mut(at..end)?.copy_from_slice(bytes);
    Some(())
}

fn get_u32(buf: &Block, at: usize) -> Option<u32> {
    let b = buf.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(b.try_into().ok()?))
}

fn get_u16(buf: &Block, at: usize) -> Option<u16> {
    let b = buf.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(b.try_into().ok()?))
}

fn seal(buf: &mut Block) {
    let c = crc32(buf.get(..CRC_OFFSET).unwrap_or(&[]));
    // CRC_OFFSET + 4 == BLOCK_SIZE, so this cannot fail.
    let _ = put(buf, CRC_OFFSET, &c.to_le_bytes());
}

fn check(buf: &Block, magic: [u8; 4]) -> Result<(), BlockError> {
    if buf.get(..4) != Some(&magic[..]) {
        return Err(BlockError::BadMagic);
    }
    let stored = get_u32(buf, CRC_OFFSET).ok_or(BlockError::BadLength)?;
    if stored != crc32(buf.get(..CRC_OFFSET).unwrap_or(&[])) {
        return Err(BlockError::BadCrc);
    }
    Ok(())
}

/// Header fields of a data block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataHeader {
    /// Card identifier.
    pub card_id: u32,
    /// Session number.
    pub session: u32,
    /// Block index within the session.
    pub seq: u32,
    /// Payload bytes in use.
    pub used: u16,
    /// [`FLAG_FIRST`] and [`FLAG_LAST`].
    pub flags: u16,
}

/// A parsed data block borrowing the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataBlock<'a> {
    /// Header fields.
    pub header: DataHeader,
    /// The `used` payload bytes.
    pub payload: &'a [u8],
}

/// Build a data block image.
pub fn encode_data(out: &mut Block, header: &DataHeader, payload: &[u8]) -> Result<(), BlockError> {
    if payload.len() > PAYLOAD_MAX {
        return Err(BlockError::TooLarge);
    }
    if usize::from(header.used) != payload.len() {
        return Err(BlockError::BadLength);
    }
    *out = [0; BLOCK_SIZE];
    put(out, 0, &MAGIC_DATA).ok_or(BlockError::BadLength)?;
    put(out, 4, &header.card_id.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 8, &header.session.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 12, &header.seq.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 16, &header.used.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 18, &header.flags.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, DATA_HEADER_LEN, payload).ok_or(BlockError::BadLength)?;
    seal(out);
    Ok(())
}

/// Parse and verify a data block.
pub fn parse_data(buf: &Block) -> Result<DataBlock<'_>, BlockError> {
    check(buf, MAGIC_DATA)?;
    let header = DataHeader {
        card_id: get_u32(buf, 4).ok_or(BlockError::BadLength)?,
        session: get_u32(buf, 8).ok_or(BlockError::BadLength)?,
        seq: get_u32(buf, 12).ok_or(BlockError::BadLength)?,
        used: get_u16(buf, 16).ok_or(BlockError::BadLength)?,
        flags: get_u16(buf, 18).ok_or(BlockError::BadLength)?,
    };
    let end = DATA_HEADER_LEN.checked_add(usize::from(header.used)).ok_or(BlockError::BadLength)?;
    if end > CRC_OFFSET {
        return Err(BlockError::BadLength);
    }
    let payload = buf.get(DATA_HEADER_LEN..end).ok_or(BlockError::BadLength)?;
    Ok(DataBlock { header, payload })
}

/// The region header, written once when a card is prepared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionHeader {
    /// Random identifier for this preparation of the card.
    pub card_id: u32,
    /// Size of the region in blocks (including header and hint blocks).
    pub region_blocks: u32,
}

/// Build the region header block.
pub fn encode_region(out: &mut Block, h: &RegionHeader) -> Result<(), BlockError> {
    *out = [0; BLOCK_SIZE];
    put(out, 0, &MAGIC_REGION).ok_or(BlockError::BadLength)?;
    put(out, 4, &FORMAT_VERSION.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 8, &h.card_id.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 12, &h.region_blocks.to_le_bytes()).ok_or(BlockError::BadLength)?;
    seal(out);
    Ok(())
}

/// Parse and verify the region header.
pub fn parse_region(buf: &Block) -> Result<RegionHeader, BlockError> {
    check(buf, MAGIC_REGION)?;
    if get_u16(buf, 4).ok_or(BlockError::BadLength)? != FORMAT_VERSION {
        return Err(BlockError::BadVersion);
    }
    Ok(RegionHeader {
        card_id: get_u32(buf, 8).ok_or(BlockError::BadLength)?,
        region_blocks: get_u32(buf, 12).ok_or(BlockError::BadLength)?,
    })
}

/// A checkpoint: everything before `tail` is known to be valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    /// Card identifier.
    pub card_id: u32,
    /// Increases with every hint written; the larger of the two slots wins.
    pub hint_seq: u32,
    /// Session that was open when the hint was written.
    pub last_session: u32,
    /// Region-relative block number of the next block to be written.
    pub tail: u32,
    /// `seq` the block at `tail` will carry if the session continues.
    pub next_seq: u32,
}

/// Build a hint block.
pub fn encode_hint(out: &mut Block, h: &Hint) -> Result<(), BlockError> {
    *out = [0; BLOCK_SIZE];
    put(out, 0, &MAGIC_HINT).ok_or(BlockError::BadLength)?;
    put(out, 4, &h.card_id.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 8, &h.hint_seq.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 12, &h.last_session.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 16, &h.tail.to_le_bytes()).ok_or(BlockError::BadLength)?;
    put(out, 20, &h.next_seq.to_le_bytes()).ok_or(BlockError::BadLength)?;
    seal(out);
    Ok(())
}

/// Parse and verify a hint block.
pub fn parse_hint(buf: &Block) -> Result<Hint, BlockError> {
    check(buf, MAGIC_HINT)?;
    Ok(Hint {
        card_id: get_u32(buf, 4).ok_or(BlockError::BadLength)?,
        hint_seq: get_u32(buf, 8).ok_or(BlockError::BadLength)?,
        last_session: get_u32(buf, 12).ok_or(BlockError::BadLength)?,
        tail: get_u32(buf, 16).ok_or(BlockError::BadLength)?,
        next_seq: get_u32(buf, 20).ok_or(BlockError::BadLength)?,
    })
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    fn hdr(used: u16) -> DataHeader {
        DataHeader { card_id: 0xDEAD_BEEF, session: 7, seq: 9, used, flags: FLAG_FIRST }
    }

    #[test]
    fn layout_constants() {
        assert_eq!(PAYLOAD_MAX, 488);
        assert_eq!(CRC_OFFSET, 508);
        assert_eq!(DATA_HEADER_LEN + PAYLOAD_MAX + 4, BLOCK_SIZE);
    }

    #[test]
    fn data_block_roundtrip_and_byte_layout() {
        let payload: std::vec::Vec<u8> = (0..100_u8).collect();
        let mut b = [0_u8; BLOCK_SIZE];
        encode_data(&mut b, &hdr(100), &payload).unwrap();
        assert_eq!(&b[0..4], b"TLB1");
        assert_eq!(&b[4..8], &0xDEAD_BEEF_u32.to_le_bytes());
        assert_eq!(&b[8..12], &7_u32.to_le_bytes());
        assert_eq!(&b[12..16], &9_u32.to_le_bytes());
        assert_eq!(&b[16..18], &100_u16.to_le_bytes());
        assert_eq!(&b[18..20], &1_u16.to_le_bytes());
        assert_eq!(&b[20..120], &payload[..]);
        assert!(b[120..508].iter().all(|x| *x == 0), "unused payload is zero");
        let p = parse_data(&b).unwrap();
        assert_eq!(p.header, hdr(100));
        assert_eq!(p.payload, &payload[..]);
    }

    #[test]
    fn full_and_empty_payloads() {
        let mut b = [0_u8; BLOCK_SIZE];
        encode_data(&mut b, &hdr(0), &[]).unwrap();
        assert_eq!(parse_data(&b).unwrap().payload.len(), 0);
        let full = [0x5A_u8; PAYLOAD_MAX];
        encode_data(&mut b, &hdr(PAYLOAD_MAX as u16), &full).unwrap();
        assert_eq!(parse_data(&b).unwrap().payload, &full[..]);
        assert_eq!(encode_data(&mut b, &hdr(489), &[0; 489]), Err(BlockError::TooLarge));
        assert_eq!(encode_data(&mut b, &hdr(5), &[0; 4]), Err(BlockError::BadLength));
    }

    #[test]
    fn every_single_bit_flip_is_rejected() {
        let mut b = [0_u8; BLOCK_SIZE];
        encode_data(&mut b, &hdr(50), &[0xC3; 50]).unwrap();
        for byte in 0..BLOCK_SIZE {
            for bit in 0..8 {
                let mut c = b;
                c[byte] ^= 1 << bit;
                assert!(parse_data(&c).is_err(), "byte {byte} bit {bit} flip not detected");
            }
        }
    }

    #[test]
    fn blank_and_foreign_blocks_are_rejected() {
        assert_eq!(parse_data(&[0; BLOCK_SIZE]), Err(BlockError::BadMagic));
        assert_eq!(parse_data(&[0xFF; BLOCK_SIZE]), Err(BlockError::BadMagic));
        let mut b = [0_u8; BLOCK_SIZE];
        encode_region(&mut b, &RegionHeader { card_id: 1, region_blocks: 100 }).unwrap();
        assert_eq!(parse_data(&b), Err(BlockError::BadMagic), "wrong block type");
        assert_eq!(parse_hint(&b), Err(BlockError::BadMagic));
    }

    #[test]
    fn used_length_beyond_the_payload_area_is_rejected_even_with_a_valid_crc() {
        let mut b = [0_u8; BLOCK_SIZE];
        encode_data(&mut b, &hdr(10), &[1; 10]).unwrap();
        b[16..18].copy_from_slice(&600_u16.to_le_bytes());
        super::seal(&mut b);
        assert_eq!(parse_data(&b), Err(BlockError::BadLength));
    }

    #[test]
    fn region_header_roundtrip_and_version_check() {
        let h = RegionHeader { card_id: 0x1234_5678, region_blocks: 62_333_952 };
        let mut b = [0_u8; BLOCK_SIZE];
        encode_region(&mut b, &h).unwrap();
        assert_eq!(&b[0..4], b"TLH1");
        assert_eq!(parse_region(&b), Ok(h));
        b[4..6].copy_from_slice(&2_u16.to_le_bytes());
        super::seal(&mut b);
        assert_eq!(parse_region(&b), Err(BlockError::BadVersion));
    }

    #[test]
    fn hint_roundtrip() {
        let h = Hint { card_id: 5, hint_seq: 99, last_session: 3, tail: 12_345, next_seq: 777 };
        let mut b = [0_u8; BLOCK_SIZE];
        encode_hint(&mut b, &h).unwrap();
        assert_eq!(&b[0..4], b"TLT1");
        assert_eq!(parse_hint(&b), Ok(h));
        b[100] ^= 1;
        assert_eq!(parse_hint(&b), Err(BlockError::BadCrc));
    }
}
