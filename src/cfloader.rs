// High-level interface for the Crazyflie 2.x bootloader
// Provide connectivity to both bootloader on the nRF and STM32
// as well as high-level algorithm to program the Crazyflie 2.x

use std::time::Duration;

use crate::Bllink;
use crate::bootloader::{self, Bootloader};
use crate::packets::InfoPacket;

/// How long to wait between attempts at re-reading the bootloader info.
///
/// Only the gap between attempts: how long the whole retry is allowed to take
/// is the caller's `timeout` in [`CFLoader::refresh_info`].
const INFO_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// High-level interface for Crazyflie 2.x bootloader operations
///
/// This struct provides a convenient way to interact with both the nRF51822 and STM32F405
/// bootloaders on the Crazyflie 2.x platform. It handles the low-level communication
/// details and provides high-level methods for common operations like flashing firmware
/// and reading flash memory.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> anyhow::Result<()> {
/// use cfloader::{Bllink, CFLoader};
///
/// let bllink = Bllink::new(None).await?;
/// let mut loader = CFLoader::new(bllink).await?;
///
/// // Flash firmware to STM32
/// let firmware = std::fs::read("firmware.bin")?;
/// loader.flash_stm32(0x8000, &firmware).await?;
///
/// // Reset to normal operation
/// loader.reset_to_firmware().await?;
/// # Ok(())
/// # }
/// ```
pub struct CFLoader {
    bllink: Bllink,
    nrf51: Bootloader,
    stm32: Bootloader,
    nrf51_info: InfoPacket,
    stm32_info: InfoPacket,
}

impl CFLoader {
    /// Create a new CFLoader instance
    ///
    /// Initializes both the nRF51822 and STM32F405 bootloader interfaces and
    /// retrieves their information packets.
    ///
    /// # Arguments
    ///
    /// * `bllink` - An established Bllink connection to the Crazyflie bootloader
    ///
    /// # Returns
    ///
    /// A new `CFLoader` instance ready for bootloader operations
    ///
    /// # Errors
    ///
    /// Returns an error if communication with either bootloader fails
    pub async fn new(mut bllink: Bllink) -> anyhow::Result<Self> {
        let nrf51 = Bootloader::new(bootloader::TARGET_NRF51);
        let stm32 = Bootloader::new(bootloader::TARGET_STM32);
        
        // Get info from both bootloaders
        let nrf51_info = nrf51.get_info(&mut bllink).await?;
        let stm32_info = stm32.get_info(&mut bllink).await?;
        
        Ok(CFLoader { 
            bllink, 
            nrf51, 
            stm32,
            nrf51_info,
            stm32_info,
        })
    }

    /// Get a formatted string with info from both bootloaders
    ///
    /// # Returns
    ///
    /// A formatted string containing information about both the nRF51 and STM32 bootloaders
    pub async fn get_info(&mut self) -> anyhow::Result<String> {
        // Return info from both bootloaders
        Ok(format!(
            "nRF51 Bootloader: {}\nSTM32 Bootloader: {}",
            self.nrf51_info,
            self.stm32_info
        ))
    }

    /// Get nRF51 bootloader info
    pub fn nrf51_info(&self) -> &InfoPacket {
        &self.nrf51_info
    }

    /// Get STM32 bootloader info
    pub fn stm32_info(&self) -> &InfoPacket {
        &self.stm32_info
    }

    /// Get a detailed summary of both bootloaders
    pub fn get_bootloader_summary(&self) -> String {
        format!(
            "Crazyflie 2.x Bootloader Information:\n\
            \n\
            nRF51822 Bootloader:\n\
            - Page Size: {} bytes\n\
            - Buffer Pages: {}\n\
            - Flash Pages: {}\n\
            - Flash Start: {}\n\
            - Version: 0x{:02X}\n\
            \n\
            STM32F405 Bootloader:\n\
            - Page Size: {} bytes\n\
            - Buffer Pages: {}\n\
            - Flash Pages: {}\n\
            - Flash Start: {}\n\
            - Version: 0x{:02X}",
            self.nrf51_info.page_size(),
            self.nrf51_info.n_buff_page(),
            self.nrf51_info.n_flash_page(),
            self.nrf51_info.flash_start(),
            self.nrf51_info.version(),
            self.stm32_info.page_size(),
            self.stm32_info.n_buff_page(),
            self.stm32_info.n_flash_page(),
            self.stm32_info.flash_start(),
            self.stm32_info.version()
        )
    }

    /// Flash an image to either the nRF51 or STM32 bootloader with progress callback
    /// 
    /// # Arguments
    /// * `target` - The bootloader target (use bootloader::TARGET_NRF51 or bootloader::TARGET_STM32)
    /// * `start_address` - The starting address in flash where the image should be written
    /// * `image` - The image data to flash
    /// * `progress_callback` - Optional callback function to report progress (bytes_written, total_bytes)
    pub async fn flash_image_with_progress<F>(&mut self, target: u8, start_address: u32, image: &[u8], mut progress_callback: Option<F>) -> anyhow::Result<()> 
    where
        F: FnMut(usize, usize),
    {
        self.flash_image_internal(target, start_address, image, &mut progress_callback).await
    }

    /// Flash an image to either the nRF51 or STM32 bootloader
    /// 
    /// # Arguments
    /// * `target` - The bootloader target (use bootloader::TARGET_NRF51 or bootloader::TARGET_STM32)
    /// * `start_address` - The starting address in flash where the image should be written
    /// * `image` - The image data to flash
    pub async fn flash_image(&mut self, target: u8, start_address: u32, image: &[u8]) -> anyhow::Result<()> {
        self.flash_image_internal(target, start_address, image, &mut None::<fn(usize, usize)>).await
    }

    /// Internal flash implementation with optional progress callback
    async fn flash_image_internal<F>(&mut self, target: u8, start_address: u32, image: &[u8], progress_callback: &mut Option<F>) -> anyhow::Result<()> 
    where
        F: FnMut(usize, usize),
    {
        // Get the appropriate bootloader info
        let (page_size, n_buff_pages, flash_start_page) = match target {
            bootloader::TARGET_NRF51 => (
                self.nrf51_info.page_size() as usize,
                self.nrf51_info.n_buff_page() as usize,
                self.nrf51_info.flash_start(),
            ),
            bootloader::TARGET_STM32 => (
                self.stm32_info.page_size() as usize,
                self.stm32_info.n_buff_page() as usize,
                self.stm32_info.flash_start(),
            ),
            _ => return Err(anyhow::anyhow!("Invalid bootloader target: 0x{:02X}", target)),
        };
        
        // Calculate buffer size (total buffer capacity)
        let buffer_size = page_size * n_buff_pages;
        
        // Calculate which flash page corresponds to the start address
        let start_page = (start_address / page_size as u32) as u16;
        
        // Validate that we're writing to a valid flash area
        if start_page < flash_start_page {
            return Err(anyhow::anyhow!(
                "Cannot write to page {} (before flash start page {})", 
                start_page, flash_start_page
            ));
        }


        let mut bytes_written = 0;
        let mut current_address = start_address;


        while bytes_written < image.len() {
            
            // Calculate how much data we can write in this iteration
            let remaining_bytes = image.len() - bytes_written;
            let chunk_size = remaining_bytes.min(buffer_size);
            let chunk = &image[bytes_written..bytes_written + chunk_size];

            // Calculate flash pages to write
            let current_page = (current_address / page_size as u32) as u16;
            let pages_needed = ((chunk_size + page_size - 1) / page_size) as u16; // Round up



            // Load the chunk into the buffer(s)
            self.load_chunk_to_buffer(target, chunk, page_size).await?;
            
            // Flash the buffer to flash memory
            let result = match target {
                bootloader::TARGET_NRF51 => {
                    self.nrf51.write_flash(&mut self.bllink, 0, current_page, pages_needed).await?
                },
                bootloader::TARGET_STM32 => {
                    self.stm32.write_flash(&mut self.bllink, 0, current_page, pages_needed).await?
                },
                _ => unreachable!(), // Already validated above
            };

            // Check if the flash operation was successful
            if !result.is_success() {
                return Err(anyhow::anyhow!(
                    "Flash operation failed at page {}: {}", 
                    current_page, result.error()
                ));
            }


            // Update counters
            bytes_written += chunk_size;
            current_address += chunk_size as u32;
            
            // Call progress callback if provided
            if let Some(callback) = progress_callback {
                callback(bytes_written, image.len());
            }
        }

        Ok(())
    }

    /// Load a chunk of data into the bootloader's buffer pages
    async fn load_chunk_to_buffer(&mut self, target: u8, chunk: &[u8], page_size: usize) -> anyhow::Result<()> {
        let mut chunk_offset = 0;
        let mut buffer_page = 0u16;

        while chunk_offset < chunk.len() {
            let remaining_in_chunk = chunk.len() - chunk_offset;
            let bytes_to_write = remaining_in_chunk.min(page_size);
            
            // Load data into the current buffer page
            let mut page_offset = 0u16;
            let mut bytes_written_to_page = 0;

            while bytes_written_to_page < bytes_to_write {
                // Calculate how much we can write in this load_buffer call (max 25 bytes per call)
                let remaining_in_page = bytes_to_write - bytes_written_to_page;
                let load_size = remaining_in_page.min(25); // reduced from 27 to 25 due to missing last 2 bytes
                
                let data_slice = &chunk[chunk_offset + bytes_written_to_page..chunk_offset + bytes_written_to_page + load_size];
                let _global_offset = chunk_offset + bytes_written_to_page;
                
                match target {
                    bootloader::TARGET_NRF51 => {
                        self.nrf51.load_buffer(&mut self.bllink, buffer_page, page_offset, data_slice).await?;
                    },
                    bootloader::TARGET_STM32 => {
                        self.stm32.load_buffer(&mut self.bllink, buffer_page, page_offset, data_slice).await?;
                    },
                    _ => return Err(anyhow::anyhow!("Invalid bootloader target: 0x{:02X}", target)),
                }
                
                page_offset += load_size as u16;
                bytes_written_to_page += load_size;
            }

            chunk_offset += bytes_to_write;
            buffer_page += 1;
        }

        Ok(())
    }

    /// Flash an image to the STM32 bootloader with progress callback
    ///
    /// Convenience method that wraps [`flash_image_with_progress`](Self::flash_image_with_progress)
    /// for the STM32 target.
    ///
    /// # Arguments
    ///
    /// * `start_address` - The starting address in flash where the image should be written
    /// * `image` - The image data to flash
    /// * `progress_callback` - Optional callback function to report progress (bytes_written, total_bytes)
    pub async fn flash_stm32_with_progress<F>(&mut self, start_address: u32, image: &[u8], progress_callback: Option<F>) -> anyhow::Result<()> 
    where
        F: FnMut(usize, usize),
    {
        self.flash_image_with_progress(bootloader::TARGET_STM32, start_address, image, progress_callback).await
    }

    /// Flash an image to the nRF51 bootloader with progress callback
    ///
    /// Convenience method that wraps [`flash_image_with_progress`](Self::flash_image_with_progress)
    /// for the nRF51 target.
    ///
    /// # Arguments
    ///
    /// * `start_address` - The starting address in flash where the image should be written
    /// * `image` - The image data to flash
    /// * `progress_callback` - Optional callback function to report progress (bytes_written, total_bytes)
    pub async fn flash_nrf51_with_progress<F>(&mut self, start_address: u32, image: &[u8], progress_callback: Option<F>) -> anyhow::Result<()> 
    where
        F: FnMut(usize, usize),
    {
        self.flash_image_with_progress(bootloader::TARGET_NRF51, start_address, image, progress_callback).await
    }

    /// Flash an image to the STM32 bootloader
    ///
    /// Convenience method that wraps [`flash_image`](Self::flash_image) for the STM32 target.
    ///
    /// # Arguments
    ///
    /// * `start_address` - The starting address in flash where the image should be written
    /// * `image` - The image data to flash
    pub async fn flash_stm32(&mut self, start_address: u32, image: &[u8]) -> anyhow::Result<()> {
        self.flash_image(bootloader::TARGET_STM32, start_address, image).await
    }

    /// Flash an image to the nRF51 bootloader
    ///
    /// Convenience method that wraps [`flash_image`](Self::flash_image) for the nRF51 target.
    ///
    /// # Arguments
    ///
    /// * `start_address` - The starting address in flash where the image should be written
    /// * `image` - The image data to flash
    pub async fn flash_nrf51(&mut self, start_address: u32, image: &[u8]) -> anyhow::Result<()> {
        self.flash_image(bootloader::TARGET_NRF51, start_address, image).await
    }

    /// Read flash content from either the nRF51 or STM32 bootloader
    /// 
    /// # Arguments
    /// * `target` - The bootloader target (use bootloader::TARGET_NRF51 or bootloader::TARGET_STM32)
    /// * `start_address` - The starting address in flash to read from
    /// * `length` - The number of bytes to read
    /// 
    /// # Returns
    /// A `Vec<u8>` containing the read flash content
    pub async fn read_flash(&mut self, target: u8, start_address: u32, length: u32) -> anyhow::Result<Vec<u8>> {
        // Get the appropriate bootloader info
        let page_size = match target {
            bootloader::TARGET_NRF51 => self.nrf51_info.page_size() as usize,
            bootloader::TARGET_STM32 => self.stm32_info.page_size() as usize,
            _ => return Err(anyhow::anyhow!("Invalid bootloader target: 0x{:02X}", target)),
        };


        let mut result = Vec::with_capacity(length as usize);
        let mut bytes_read = 0u32;
        let mut current_address = start_address;

        // The bootloader can read up to 27 bytes per read_flash call (based on protocol limit)
        const MAX_READ_SIZE: usize = 27;

        while bytes_read < length {
            let remaining_bytes = length - bytes_read;
            let read_size = (remaining_bytes as usize).min(MAX_READ_SIZE);

            // Calculate page and offset within page
            let current_page = (current_address / page_size as u32) as u16;
            let page_offset = (current_address % page_size as u32) as u16;

            // Read from flash
            let flash_data = match target {
                bootloader::TARGET_NRF51 => {
                    self.nrf51.read_flash(&mut self.bllink, current_page, page_offset).await?
                },
                bootloader::TARGET_STM32 => {
                    self.stm32.read_flash(&mut self.bllink, current_page, page_offset).await?
                },
                _ => unreachable!(), // Already validated above
            };

            // Take only the bytes we need (the response might contain more data than requested)
            let data_to_take = read_size.min(flash_data.data.len());
            
            if data_to_take == 0 {
                break;
            }
            
            result.extend_from_slice(&flash_data.data[..data_to_take]);

            bytes_read += data_to_take as u32;
            current_address += data_to_take as u32;
        }

        Ok(result)
    }

    /// Read flash content from the STM32 bootloader
    ///
    /// Convenience method that wraps [`read_flash`](Self::read_flash) for the STM32 target.
    ///
    /// # Arguments
    ///
    /// * `start_address` - The starting address in flash to read from
    /// * `length` - The number of bytes to read
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing the read flash content
    pub async fn read_stm32_flash(&mut self, start_address: u32, length: u32) -> anyhow::Result<Vec<u8>> {
        self.read_flash(bootloader::TARGET_STM32, start_address, length).await
    }

    /// Read flash content from the nRF51 bootloader
    ///
    /// Convenience method that wraps [`read_flash`](Self::read_flash) for the nRF51 target.
    ///
    /// # Arguments
    ///
    /// * `start_address` - The starting address in flash to read from
    /// * `length` - The number of bytes to read
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing the read flash content
    pub async fn read_nrf51_flash(&mut self, start_address: u32, length: u32) -> anyhow::Result<Vec<u8>> {
        self.read_flash(bootloader::TARGET_NRF51, start_address, length).await
    }

    /// Reset the Crazyflie and boot into normal firmware
    ///
    /// Sends the reset initialization and reset commands to the nRF51 bootloader,
    /// which will cause the Crazyflie to exit bootloader mode and boot into
    /// normal firmware operation.
    ///
    /// # Note
    ///
    /// After calling this method, the Bllink connection will no longer be valid
    /// as the Crazyflie will be running normal firmware instead of the bootloader.
    pub async fn reset_to_firmware(&mut self) -> anyhow::Result<()> {
        let reset_init_command = vec![0xFF, bootloader::TARGET_NRF51, 0xFF];
        self.bllink.send(&reset_init_command).await?;

        let reset_command = vec![0xFF, bootloader::TARGET_NRF51, 0xF0, 0x01];
        self.bllink.send(&reset_command).await?;

        Ok(())
    }

    /// Restart the nRF51 into its bootloader rather than into the firmware
    ///
    /// This is needed after replacing the nRF51 bootloader and softdevice:
    /// the device has to restart for the new bootloader to take over, but it
    /// must come back up in the bootloader so it can still be talked to.
    ///
    /// The link itself is connectionless, so it stays usable across the
    /// restart, but it is moved to the bootloader's own address, which is
    /// also returned. Call [`CFLoader::refresh_info`] afterwards to pick up
    /// the new bootloader's info.
    pub async fn reset_to_bootloader(&mut self) -> anyhow::Result<[u8; 5]> {
        // The restart command answers with the address the bootloader will
        // listen on once it comes back, which is derived from the device and
        // is not the address it was reached on: a Crazyflie entered through
        // the rescue bootloader answers on the default address but returns
        // here on its own. The link has to follow it.
        let reset_init_command = vec![0xFF, bootloader::TARGET_NRF51, 0xFF];
        let response = self
            .bllink
            .request(&reset_init_command, Duration::from_millis(100))
            .await?;

        if response.len() < 7 {
            return Err(anyhow::anyhow!(
                "Bootloader did not report an address to restart on (got {} bytes)",
                response.len()
            ));
        }

        // The four bytes are sent little endian and prefixed with 0xB1.
        let address = [0xB1, response[6], response[5], response[4], response[3]];

        let reset_command = vec![0xFF, bootloader::TARGET_NRF51, 0xF0, 0x00];
        self.bllink.send(&reset_command).await?;
        self.bllink.set_address(address);

        Ok(address)
    }

    /// Read the info packets from both bootloaders
    ///
    /// Both are read before either is returned, so a caller never has to deal
    /// with one of them having been updated and the other not.
    async fn read_info(&mut self) -> anyhow::Result<(InfoPacket, InfoPacket)> {
        let nrf51_info = self.nrf51.get_info(&mut self.bllink).await?;
        let stm32_info = self.stm32.get_info(&mut self.bllink).await?;

        Ok((nrf51_info, stm32_info))
    }

    /// Re-read the info packets from both bootloaders
    ///
    /// The info is cached when the loader is created, so this is how you pick
    /// up a change — after a restart, or after a new bootloader and softdevice
    /// have been flashed and the flash layout has moved.
    ///
    /// Retries for up to `timeout`, since a device that has just been reset
    /// takes a moment to answer again. The deadline governs when a further
    /// attempt may start rather than cutting one short: see the note in the
    /// body on why a read is never cancelled. Both bootloaders have to answer within
    /// that window: after a restart the nRF51 can be ready before the STM32
    /// is, so a read that gets only one of them is retried like any other
    /// failure rather than reported.
    ///
    /// The cached info is replaced only once both packets have been read, so
    /// a call that times out leaves the loader exactly as it was.
    pub async fn refresh_info(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let deadline = std::time::Instant::now() + timeout;

        loop {
            // A read is deliberately never cancelled, so this is not wrapped
            // in `tokio::time::timeout`. The shared radio keeps one response
            // channel per instance, and the radio thread delivers a result to
            // it whether or not anyone is still waiting; abandoning a
            // transfer mid-flight therefore leaves its result queued, and the
            // next transfer would take it as its own. The deadline instead
            // decides whether another attempt may start, which lets an
            // attempt already under way overshoot it by at most one read —
            // around 200 ms with the retry settings in `bllink`.
            match self.read_info().await {
                // Both packets are replaced together, so a failed read leaves
                // the previous pair in place rather than a mixed one.
                Ok((nrf51_info, stm32_info)) => {
                    self.nrf51_info = nrf51_info;
                    self.stm32_info = stm32_info;
                    return Ok(());
                }
                Err(e) => {
                    // The last failure says more than the bare timeout does.
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        return Err(anyhow::anyhow!(
                            "Bootloader did not respond within {:?}: {}",
                            timeout,
                            e
                        ));
                    }

                    tokio::time::sleep(INFO_RETRY_INTERVAL.min(remaining)).await;
                }
            }
        }
    }



}