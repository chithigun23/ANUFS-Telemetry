//! `BlockDevice` adapters for the two microSD slots.
//!
//! * [`Sd1`]: SDMMC1, 4-bit, DMA (slot J3).
//! * [`Sd2`]: SPI mode over SPI2 (slot J11, which has only CLK, CMD, DAT0 and
//!   DAT3 wired to the MCU), through the `embedded-sdmmc` SPI driver.
//!
//! Both present the same 512 byte block interface the log expects. They add no
//! filesystem: the log writes raw blocks into a region of the card.
//!
//! Status: compiled against the real HAL but not yet run on hardware.

use embassy_stm32::sdmmc::sd::{Card, DataBlock, StorageDevice};
use embassy_stm32::sdmmc::Error as SdmmcError;
use embedded_sdmmc::{Block as SdBlock, BlockDevice as SdBlockDevice, BlockIdx, SdCard, SdCardError};
use tel_logfmt::block::Block;
use tel_store::BlockDevice;

/// Card in the SDMMC1 slot.
pub struct Sd1<'a, 'b> {
    inner: StorageDevice<'a, 'b, Card>,
}

impl<'a, 'b> Sd1<'a, 'b> {
    /// Wrap an initialised card (see `StorageDevice::new_sd_card`).
    pub fn new(inner: StorageDevice<'a, 'b, Card>) -> Self {
        Self { inner }
    }

    /// Card size in 512 byte blocks.
    pub fn size_blocks(&self) -> u64 {
        use embassy_stm32::sdmmc::sd::Addressable;
        self.inner.card().size() / 512
    }
}

impl BlockDevice for Sd1<'_, '_> {
    type Error = SdmmcError;

    async fn read_block(&mut self, lba: u32, buf: &mut Block) -> Result<(), SdmmcError> {
        // The HAL needs a 4-byte aligned buffer for DMA, so stage through one.
        let mut staged = DataBlock::new();
        self.inner.read_block(lba, &mut staged).await?;
        buf.copy_from_slice(&staged[..]);
        Ok(())
    }

    async fn write_block(&mut self, lba: u32, data: &Block) -> Result<(), SdmmcError> {
        let mut staged = DataBlock::new();
        staged.copy_from_slice(data);
        self.inner.write_block(lba, &staged).await
    }
}

/// Card in the SPI-mode slot.
///
/// The underlying driver is blocking: each call holds the CPU for the duration
/// of the SPI transfer (a block read or write takes up to a few milliseconds,
/// and a write can occasionally take tens of milliseconds while the card
/// programs). Run the SD2 service at a low priority so it cannot delay the
/// time-critical interrupt handlers.
pub struct Sd2<SPI, D>
where
    SPI: embedded_hal::spi::SpiDevice<u8>,
    D: embedded_hal::delay::DelayNs,
{
    card: SdCard<SPI, D>,
}

impl<SPI, D> Sd2<SPI, D>
where
    SPI: embedded_hal::spi::SpiDevice<u8>,
    D: embedded_hal::delay::DelayNs,
{
    /// Wrap an SPI device (chip select on PB0) and a delay provider.
    pub fn new(spi: SPI, delay: D) -> Self {
        Self { card: SdCard::new(spi, delay) }
    }

    /// Card size in 512 byte blocks (initialises the card on first use).
    pub fn size_blocks(&self) -> Result<u64, SdCardError> {
        Ok(self.card.num_bytes()? / 512)
    }
}

impl<SPI, D> BlockDevice for Sd2<SPI, D>
where
    SPI: embedded_hal::spi::SpiDevice<u8>,
    D: embedded_hal::delay::DelayNs,
{
    type Error = SdCardError;

    async fn read_block(&mut self, lba: u32, buf: &mut Block) -> Result<(), SdCardError> {
        let mut b = [SdBlock::new()];
        self.card.read(&mut b, BlockIdx(lba))?;
        let [first] = b;
        buf.copy_from_slice(&first.contents);
        Ok(())
    }

    async fn write_block(&mut self, lba: u32, data: &Block) -> Result<(), SdCardError> {
        let mut blk = SdBlock::new();
        blk.contents.copy_from_slice(data);
        self.card.write(&[blk], BlockIdx(lba))
    }
}

/// Compile-time proof that both adapters satisfy the log's storage trait.
#[allow(dead_code)]
fn assert_block_devices<SPI, D>()
where
    SPI: embedded_hal::spi::SpiDevice<u8>,
    D: embedded_hal::delay::DelayNs,
{
    fn is_block_device<T: BlockDevice>() {}
    is_block_device::<Sd1<'static, 'static>>();
    is_block_device::<Sd2<SPI, D>>();
}
