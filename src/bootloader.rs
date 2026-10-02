//! # Interface to one bootloader state machine
//!
//! Crazyflie 2.x platform has 2 such bootloaders, one in the nRF51822 and one in the STM32F405.
//! This module provides the low-level interface to communicate with individual bootloaders.
//!
//! For most use cases, prefer using the high-level [`CFLoader`](crate::CFLoader) interface instead.

use std::time::Duration;

use bllink::Bllink;

use crate::{bllink, packets::*};

// Bootloader command constants
const CMD_GET_INFO: u8 = 0x10;
const CMD_SET_ADDRESS: u8 = 0x11;
const CMD_GET_MAPPING: u8 = 0x12;
const CMD_LOAD_BUFFER: u8 = 0x14;
const CMD_READ_BUFFER: u8 = 0x15;
const CMD_WRITE_FLASH: u8 = 0x18;
const CMD_FLASH_STATUS: u8 = 0x19;
const CMD_READ_FLASH: u8 = 0x1C;
const CMD_PAGE_CRC: u8 = 0x20;
const CMD_SET_BROADCAST_ADDRESS: u8 = 0x21;
const CMD_RANGE_CRC: u8 = 0x22;
const CMD_RESET_INIT: u8 = 0xFF;
const CMD_RESET: u8 = 0xF0;
const CMD_ALLOFF: u8 = 0x01;
const CMD_SYSOFF: u8 = 0x02;
const CMD_SYSON: u8 = 0x03;
const CMD_GETVBAT: u8 = 0x04;

/// STM32 bootloader target identifier
pub const TARGET_STM32: u8 = 0xFF;
/// nRF51 bootloader target identifier
pub const TARGET_NRF51: u8 = 0xFE;

// Default short timeout for bootloader operations that should return directly
const SHORT_TIMEOUT: Duration = Duration::from_millis(100);
// Timeout for flash operation, flash operation can take up to one second to complete
const FLASH_TIMEOUT: Duration = Duration::from_secs(2);

/// Bootloader interface for Crazyflie 2.x platform
/// 
/// The Crazyflie 2.x platform has 2 bootloaders: one in the nRF51822 and one in the STM32F405.
/// This struct provides a unified interface to communicate with either bootloader.
pub struct Bootloader {
    target: u8,
}

impl Bootloader {
    /// Create a new bootloader interface for the given target
    pub fn new(target: u8) -> Self {
        Bootloader { target }
    }

    /// Create a bootloader for the STM32 target (0xFF)
    pub fn stm32() -> Self {
        Bootloader::new(TARGET_STM32)
    }

    /// Create a bootloader for the nRF51 target (0xFE)
    pub fn nrf51() -> Self {
        Bootloader::new(TARGET_NRF51)
    }

    /// Get the target number for this bootloader
    pub fn target(&self) -> u8 {
        self.target
    }

    /// Get bootloader information
    /// 
    /// # Arguments
    /// 
    /// * `bllink` - The Bllink interface to use for communication
    /// 
    /// # Returns
    /// 
    /// An [InfoPacket] containing the bootloader information
    pub async fn get_info(&self, bllink: &mut Bllink) -> anyhow::Result<InfoPacket> {
        let get_info_command = vec![0xff, self.target, CMD_GET_INFO];
        let response = bllink.request(&get_info_command, SHORT_TIMEOUT).await?;
        Ok(InfoPacket::from_bytes(&response[2..]))
    }

    /// Set the bootloader address
    /// 
    /// # Arguments
    /// 
    /// * `bllink` - The Bllink interface to use for communication
    /// * `address` - The address to set (5 bytes)
    /// 
    /// # Returns
    /// 
    /// An empty result indicating success or failure
    pub async fn set_address(&self, bllink: &mut Bllink, address: &[u8; 5]) -> anyhow::Result<()> {
        let mut command = vec![0xff, self.target, CMD_SET_ADDRESS];
        command.extend_from_slice(address);
        bllink.send(&command).await?;
        Ok(())
    }

    /// Set the broadcast address on the bootloader and enable broadcast reception
    ///
    /// This command configures the nRF51 radio to also listen on the specified
    /// broadcast address. Until this command is sent, the bootloader only listens
    /// on its unicast address. This must be sent via unicast to each device
    /// before starting broadcast firmware operations.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    /// * `address` - The 5-byte broadcast address to enable
    ///
    /// # Returns
    ///
    /// An empty result indicating success or failure
    pub async fn set_broadcast_address(&self, bllink: &mut Bllink, address: &[u8; 5]) -> anyhow::Result<()> {
        let mut command = vec![0xff, self.target, CMD_SET_BROADCAST_ADDRESS];
        command.extend_from_slice(address);
        bllink.send(&command).await?;
        Ok(())
    }

    /// Get the flash memory mapping from the bootloader
    ///
    /// Returns the memory layout information for the target device.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    ///
    /// # Returns
    ///
    /// A vector containing the raw mapping data bytes
    pub async fn get_mapping(&self, bllink: &mut Bllink) -> anyhow::Result<Vec<u8>> {
        let command = vec![0xff, self.target, CMD_GET_MAPPING];
        let response = bllink.request(&command, SHORT_TIMEOUT).await?;
        // Skip the first byte (command echo) and return the mapping data
        Ok(response[1..].to_vec())
    }

    /// Load data into the bootloader's RAM buffer
    ///
    /// This function loads data into a temporary buffer before flashing.
    /// The buffer is organized by pages and addresses within pages.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    /// * `page` - The page number in the buffer
    /// * `address` - The address offset within the page
    /// * `data` - The data to load (maximum 25 bytes)
    ///
    /// # Returns
    ///
    /// An empty result indicating success or failure
    ///
    /// # Errors
    ///
    /// Returns an error if `data` is longer than 25 bytes
    pub async fn load_buffer(&self, bllink: &mut Bllink, page: u16, address: u16, data: &[u8]) -> anyhow::Result<()> {
        if data.len() > 25 {
            return Err(anyhow::anyhow!("Data too large for buffer load (max 25 bytes)"));
        }
        
        let mut command = vec![0xff, self.target, CMD_LOAD_BUFFER];
        command.extend_from_slice(&page.to_le_bytes());
        command.extend_from_slice(&address.to_le_bytes());
        command.extend_from_slice(data);
        
        // Simple send with ACK - no detailed response validation since it's just an ACK
        bllink.send(&command).await?;
        Ok(())
    }

    /// Read data from the bootloader's RAM buffer
    ///
    /// Reads back data that was previously loaded into the buffer.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    /// * `page` - The page number in the buffer to read from
    /// * `address` - The address offset within the page
    ///
    /// # Returns
    ///
    /// A `BufferReadPacket` containing the buffer data
    pub async fn read_buffer(&self, bllink: &mut Bllink, page: u16, address: u16) -> anyhow::Result<BufferReadPacket> {
        let mut command = vec![0xff, self.target, CMD_READ_BUFFER];
        command.extend_from_slice(&page.to_le_bytes());
        command.extend_from_slice(&address.to_le_bytes());
        
        let response = bllink.request(&command, SHORT_TIMEOUT).await?;
        Ok(BufferReadPacket::from_bytes(&response[2..]))
    }

    /// Write buffer contents to flash memory
    ///
    /// Copies data from the RAM buffer to flash memory. This operation may take
    /// up to 2 seconds to complete as flash write operations are slow.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    /// * `buffer_page` - The starting page in the buffer to read from
    /// * `flash_page` - The starting page in flash to write to
    /// * `n_pages` - The number of pages to write
    ///
    /// # Returns
    ///
    /// A `FlashWriteResponse` indicating the result of the write operation
    pub async fn write_flash(&self, bllink: &mut Bllink, buffer_page: u16, flash_page: u16, n_pages: u16) -> anyhow::Result<FlashWriteResponse> {
        let mut command = vec![0xff, self.target, CMD_WRITE_FLASH];
        command.extend_from_slice(&buffer_page.to_le_bytes());
        command.extend_from_slice(&flash_page.to_le_bytes());
        command.extend_from_slice(&n_pages.to_le_bytes());
        
        // TODO: When flashing, if the ack is lost, we should send again a flash status request and not a flash
        //       This is because flash reequest both takes a lot of time and utilize flash endurance of the chip.
        let response = bllink.request_match_response(&command, 3, FLASH_TIMEOUT).await?;
        Ok(FlashWriteResponse::from_bytes(&response[2..]))
    }

    /// Get the current flash operation status
    ///
    /// Queries the bootloader for the status of any ongoing flash operation.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    ///
    /// # Returns
    ///
    /// A `FlashStatusResponse` containing the current flash status
    pub async fn flash_status(&self, bllink: &mut Bllink) -> anyhow::Result<FlashStatusResponse> {
        let command = vec![0xff, self.target, CMD_FLASH_STATUS];
        let response = bllink.request(&command, SHORT_TIMEOUT).await?;
        Ok(FlashStatusResponse::from_bytes(&response[2..]))
    }

    /// Read data directly from flash memory
    ///
    /// Reads a chunk of data from the specified flash location.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    /// * `page` - The flash page number to read from
    /// * `address` - The address offset within the page
    ///
    /// # Returns
    ///
    /// A `FlashReadPacket` containing the flash data
    ///
    /// # Errors
    ///
    /// Returns an error if the response is too short or if a stale packet is detected
    /// (response page/address doesn't match the request)
    pub async fn read_flash(&self, bllink: &mut Bllink, page: u16, address: u16) -> anyhow::Result<FlashReadPacket> {
        let mut command = vec![0xff, self.target, CMD_READ_FLASH];
        command.extend_from_slice(&page.to_le_bytes());
        command.extend_from_slice(&address.to_le_bytes());
        
        let response = bllink.request(&command, SHORT_TIMEOUT).await?;
        
        if response.len() < 2 {
            return Err(anyhow::anyhow!("Response too short: {} bytes", response.len()));
        }
        
        let flash_packet = FlashReadPacket::from_bytes(&response[2..]);
        
        // Validate response matches request
        if flash_packet.page != page || flash_packet.address != address {
            return Err(anyhow::anyhow!(
                "Response mismatch: requested page={}, addr={} but got page={}, addr={} (stale packet detected)", 
                page, address, flash_packet.page, flash_packet.address
            ));
        }
        
        Ok(flash_packet)
    }

    /// Initialize reset sequence (nRF51822 specific)
    ///
    /// Prepares the bootloader for a system reset. This is typically called
    /// before [`reset`](Self::reset) to ensure a clean reset sequence.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    pub async fn reset_init(&self, bllink: &mut Bllink) -> anyhow::Result<()> {
        let command = vec![0xff, self.target, CMD_RESET_INIT];
        bllink.send(&command).await?;
        Ok(())
    }

    /// Reset the system
    ///
    /// Triggers a system reset, restarting the Crazyflie into normal operation mode.
    /// Call [`reset_init`](Self::reset_init) before this function.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    pub async fn reset(&self, bllink: &mut Bllink) -> anyhow::Result<()> {
        let command = vec![0xff, self.target, CMD_RESET];
        // No response expected for reset, but use request method
        let _ = bllink.send(&command).await;
        Ok(())
    }

    /// Turn off all systems (nRF51822 specific)
    ///
    /// Shuts down all subsystems on the Crazyflie.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    pub async fn all_off(&self, bllink: &mut Bllink) -> anyhow::Result<()> {
        let command = vec![0xff, self.target, CMD_ALLOFF];
        // No response expected
        let _ = bllink.send(&command).await;
        Ok(())
    }

    /// Power off the STM32 system (nRF51822 specific)
    ///
    /// Turns off power to the STM32 processor while keeping the nRF51822 running.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    pub async fn sys_off(&self, bllink: &mut Bllink) -> anyhow::Result<()> {
        let command = vec![0xff, self.target, CMD_SYSOFF];
        // No response expected
        let _ = bllink.send(&command).await;
        Ok(())
    }

    /// Power on the STM32 system (nRF51822 specific)
    ///
    /// Turns on power to the STM32 processor.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    pub async fn sys_on(&self, bllink: &mut Bllink) -> anyhow::Result<()> {
        let command = vec![0xff, self.target, CMD_SYSON];
        // No response expected
        let _ = bllink.send(&command).await;
        Ok(())
    }

    /// Get the CRC32 checksum of a flash page
    ///
    /// Queries the bootloader for the CRC32 of a single flash page.
    /// This is used to verify flash contents without reading back the entire page.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    /// * `page` - The flash page number to checksum
    ///
    /// # Returns
    ///
    /// A `PageCrcPacket` containing the page number and its CRC32
    pub async fn page_crc(&self, bllink: &mut Bllink, page: u16) -> anyhow::Result<PageCrcPacket> {
        let mut command = vec![0xff, self.target, CMD_PAGE_CRC];
        command.extend_from_slice(&page.to_le_bytes());

        let response = bllink.request(&command, SHORT_TIMEOUT).await?;
        PageCrcPacket::from_bytes(&response[2..])
    }

    /// Get the CRC32 checksum of any byte range of the flash
    ///
    /// The bootloader calculates the checksum while the request waits, which
    /// takes up to about 1.5 us per byte on the nRF51, so the timeout grows
    /// with the length.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    /// * `address` - Start of the range, counted from the start of the flash
    /// * `length` - Length of the range in bytes
    ///
    /// # Returns
    ///
    /// A `RangeCrcPacket` with the CRC32 of the range, or an error if the
    /// range is outside of the flash
    pub async fn range_crc(&self, bllink: &mut Bllink, address: u32, length: u32) -> anyhow::Result<RangeCrcPacket> {
        let mut command = vec![0xff, self.target, CMD_RANGE_CRC];
        command.extend_from_slice(&address.to_le_bytes());
        command.extend_from_slice(&length.to_le_bytes());

        let timeout = SHORT_TIMEOUT + Duration::from_micros(length as u64 * 4);
        let response = bllink.request(&command, timeout).await?;
        let packet = RangeCrcPacket::from_bytes(&response[2..])?;
        if packet.error != 0 {
            return Err(anyhow::anyhow!(
                "Range 0x{:08X}+{} is outside of the flash (error {})", address, length, packet.error
            ));
        }
        Ok(packet)
    }

    /// Load data into the bootloader's RAM buffer via broadcast
    ///
    /// Same as [`load_buffer`](Self::load_buffer) but sends on the broadcast address
    /// so all Crazyflies in range receive the data simultaneously.
    /// No acknowledgment is expected.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for broadcast
    /// * `page` - The page number in the buffer
    /// * `address` - The address offset within the page
    /// * `data` - The data to load (maximum 25 bytes)
    pub async fn broadcast_load_buffer(&self, bllink: &mut Bllink, page: u16, address: u16, data: &[u8]) -> anyhow::Result<()> {
        if data.len() > 25 {
            return Err(anyhow::anyhow!("Data too large for buffer load (max 25 bytes)"));
        }

        let mut command = vec![0xff, self.target, CMD_LOAD_BUFFER];
        command.extend_from_slice(&page.to_le_bytes());
        command.extend_from_slice(&address.to_le_bytes());
        command.extend_from_slice(data);

        bllink.send_broadcast(&command).await
    }

    /// Write buffer contents to flash memory via broadcast
    ///
    /// Same as [`write_flash`](Self::write_flash) but sends on the broadcast address.
    /// No response is expected - the caller must wait for flash to complete
    /// and verify via [`page_crc`](Self::page_crc) on each device individually.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for broadcast
    /// * `buffer_page` - The starting page in the buffer to read from
    /// * `flash_page` - The starting page in flash to write to
    /// * `n_pages` - The number of pages to write
    pub async fn broadcast_write_flash(&self, bllink: &mut Bllink, buffer_page: u16, flash_page: u16, n_pages: u16) -> anyhow::Result<()> {
        let mut command = vec![0xff, self.target, CMD_WRITE_FLASH];
        command.extend_from_slice(&buffer_page.to_le_bytes());
        command.extend_from_slice(&flash_page.to_le_bytes());
        command.extend_from_slice(&n_pages.to_le_bytes());

        bllink.send_broadcast(&command).await
    }

    /// Get the battery voltage (nRF51822 specific)
    ///
    /// Reads the current battery voltage from the Crazyflie.
    ///
    /// # Arguments
    ///
    /// * `bllink` - The Bllink interface to use for communication
    ///
    /// # Returns
    ///
    /// The battery voltage as a floating-point value in volts
    ///
    /// # Errors
    ///
    /// Returns an error if the response length is invalid
    pub async fn get_vbat(&self, bllink: &mut Bllink) -> anyhow::Result<f32> {
        let command = vec![0xff, self.target, CMD_GETVBAT];
        let response = bllink.request(&command, SHORT_TIMEOUT).await?;
        
        if response.len() < 4 {
            return Err(anyhow::anyhow!("Invalid VBAT response length"));
        }

        let vbat_bytes = [response[2], response[3], response[4], response[5]];
        Ok(f32::from_le_bytes(vbat_bytes))
    }
}

