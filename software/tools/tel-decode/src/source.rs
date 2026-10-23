//! Block sources: a card or image file, or memory.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use tel_logfmt::block::{Block, BLOCK_SIZE};

/// Something blocks can be read from.
pub trait BlockSource {
    /// Read the block at absolute block number `lba`.
    fn read_block(&mut self, lba: u32) -> io::Result<Block>;
    /// Total number of blocks, if known.
    fn len_blocks(&mut self) -> io::Result<u64>;
}

/// Something blocks can be written to.
pub trait BlockSink {
    /// Write the block at absolute block number `lba`.
    fn write_block(&mut self, lba: u32, data: &Block) -> io::Result<()>;
}

fn offset(lba: u32) -> u64 {
    u64::from(lba).saturating_mul(BLOCK_SIZE as u64)
}

/// A card or image opened as a file.
pub struct FileSource {
    file: File,
}

impl FileSource {
    /// Open read-only.
    pub fn open(path: &str) -> io::Result<Self> {
        Ok(Self { file: File::open(path)? })
    }

    /// Open for reading and writing (for `prepare`).
    pub fn open_rw(path: &str) -> io::Result<Self> {
        Ok(Self { file: std::fs::OpenOptions::new().read(true).write(true).open(path)? })
    }
}

impl BlockSource for FileSource {
    fn read_block(&mut self, lba: u32) -> io::Result<Block> {
        self.file.seek(SeekFrom::Start(offset(lba)))?;
        let mut b = [0_u8; BLOCK_SIZE];
        self.file.read_exact(&mut b)?;
        Ok(b)
    }

    fn len_blocks(&mut self) -> io::Result<u64> {
        let len = self.file.seek(SeekFrom::End(0))?;
        Ok(len.checked_div(BLOCK_SIZE as u64).unwrap_or(0))
    }
}

impl BlockSink for FileSource {
    fn write_block(&mut self, lba: u32, data: &Block) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(offset(lba)))?;
        self.file.write_all(data)?;
        self.file.flush()
    }
}

/// Blocks held in memory (tests, and tools that already loaded an image).
#[derive(Clone, Debug, Default)]
pub struct MemSource {
    /// The blocks.
    pub blocks: Vec<Block>,
}

impl BlockSource for MemSource {
    fn read_block(&mut self, lba: u32) -> io::Result<Block> {
        self.blocks
            .get(lba as usize)
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, format!("block {lba} is beyond the end")))
    }

    fn len_blocks(&mut self) -> io::Result<u64> {
        Ok(self.blocks.len() as u64)
    }
}

impl BlockSink for MemSource {
    fn write_block(&mut self, lba: u32, data: &Block) -> io::Result<()> {
        match self.blocks.get_mut(lba as usize) {
            Some(slot) => {
                *slot = *data;
                Ok(())
            }
            None => Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("block {lba} is beyond the end"))),
        }
    }
}
