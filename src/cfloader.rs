// High-level interface for the Crazyflie 2.x bootloader
// Provide connectivity to both bootloader on the nRF and STM32
// as well as high-level algorithm to program the Crazyflie 2.x

use crate::Bllink;
use crate::bootloader::{self, Bootloader};
use crate::packets::{InfoPacket, PageCrcPacket};
use std::time::Duration;

/// STM32F405 flash sector start pages (1 KB pages from FLASH_BASE), with an end sentinel
const STM32_SECTOR_START_PAGES: [u16; 13] = [0, 16, 32, 48, 64, 128, 256, 384, 512, 640, 768, 896, 1024];

/// Worst-case time for every Crazyflie to finish a broadcast write_flash
///
/// No answers are collected in broadcast mode, so the next chunk must wait for
/// the slowest device: the STM32 bootloader erases and programs with interrupts
/// disabled (packets sent meanwhile are lost) and the nRF51 still copies its
/// single buffer page to flash. The times are datasheet maximums.
fn broadcast_write_flash_time(target: u8, flash_page: u16, n_pages: u16, page_size: usize) -> Duration {
    let words = n_pages as u64 * page_size as u64 / 4;
    let margin_ms = 100;
    match target {
        bootloader::TARGET_STM32 => {
            // x32 parallelism: 100 us per word, sector erase 500 ms (16 KB), 1.1 s (64 KB), 2 s (128 KB)
            let erase_ms: u64 = STM32_SECTOR_START_PAGES[..12]
                .iter()
                .enumerate()
                .filter(|(_, start)| **start >= flash_page && **start < flash_page + n_pages)
                .map(|(sector, _)| match sector {
                    0..=3 => 500,
                    4 => 1100,
                    _ => 2000,
                })
                .sum();
            Duration::from_millis(erase_ms + words * 100 / 1000 + margin_ms)
        }
        _ => {
            // 22.3 ms page erase and 46.3 us per word, through the SoftDevice
            Duration::from_millis(n_pages as u64 * 23 + words * 47 / 1000 + margin_ms)
        }
    }
}

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
    broadcast_packet_delay: Duration,
    broadcast_load_passes: usize,
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
            broadcast_packet_delay: Duration::ZERO,
            broadcast_load_passes: 2,
        })
    }

    /// Set the pause between broadcast packets (default none)
    ///
    /// Back to back the radio sends a packet about every 0.7 ms, which the
    /// bootloaders keep up with. A pause only spreads the packets out in time.
    pub fn set_broadcast_packet_delay(&mut self, delay: Duration) {
        self.broadcast_packet_delay = delay;
    }

    /// Set how many times each chunk is loaded into the buffers when broadcasting (default 2)
    ///
    /// Broadcast packets are not acknowledged, so a Crazyflie that misses one
    /// LOAD_BUFFER ends up with a wrong page, which then has to be fixed over
    /// unicast. Loading every chunk twice, as two passes so that a short burst
    /// of interference can't take both copies, makes that very unlikely. Each
    /// pass adds about 0.7 ms per 25 bytes of image.
    pub fn set_broadcast_load_passes(&mut self, passes: usize) {
        self.broadcast_load_passes = passes.max(1);
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

        // Pad the last page with 0xFF. Only the bytes sent are loaded into the
        // bootloader's buffer, so the rest of a partial last page would be
        // flashed with whatever the previous chunk left there, and fail the
        // CRC verification that expects 0xFF.
        let image_len = image.len();
        let mut padded = image.to_vec();
        padded.resize(image_len.div_ceil(page_size) * page_size, 0xFF);
        let image = &padded[..];

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
                callback(bytes_written.min(image_len), image_len);
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

    /// Get a mutable reference to the underlying Bllink
    pub fn bllink_mut(&mut self) -> &mut Bllink {
        &mut self.bllink
    }

    /// Flash an image via broadcast with progress callback
    ///
    /// Sends LOAD_BUFFER and WRITE_FLASH commands on the broadcast address so all
    /// Crazyflies in range receive the firmware simultaneously. No per-device
    /// acknowledgment is used: each chunk is loaded in two passes by default
    /// (see [`set_broadcast_load_passes`](Self::set_broadcast_load_passes)),
    /// WRITE_FLASH is sent a few times, and the next chunk waits for the
    /// worst-case erase and programming time of the pages written. The last
    /// page is padded with 0xFF. Use
    /// [`verify_flash_crc`](Self::verify_flash_crc) on each device individually
    /// afterward to confirm success, and
    /// [`fix_failed_pages`](Self::fix_failed_pages) for the pages that differ.
    ///
    /// # Arguments
    /// * `target` - The bootloader target (TARGET_STM32 or TARGET_NRF51)
    /// * `start_address` - The starting address in flash
    /// * `image` - The firmware image data
    /// * `progress_callback` - Optional callback (bytes_written, total_bytes)
    pub async fn broadcast_flash_image_with_progress<F>(
        &mut self,
        target: u8,
        start_address: u32,
        image: &[u8],
        mut progress_callback: Option<F>,
    ) -> anyhow::Result<()>
    where
        F: FnMut(usize, usize),
    {
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

        let buffer_size = page_size * n_buff_pages;
        let start_page = (start_address / page_size as u32) as u16;

        if start_page < flash_start_page {
            return Err(anyhow::anyhow!(
                "Cannot write to page {} (before flash start page {})",
                start_page, flash_start_page
            ));
        }

        // Pad the last page with 0xFF. Only the bytes sent are loaded into the
        // bootloader's buffer, so the rest of a partial last page would be
        // flashed with whatever the previous chunk left there, and fail the
        // CRC verification that expects 0xFF.
        let image_len = image.len();
        let mut padded = image.to_vec();
        padded.resize(image_len.div_ceil(page_size) * page_size, 0xFF);
        let image = &padded[..];

        let mut bytes_written = 0;
        let mut current_address = start_address;

        while bytes_written < image.len() {
            let remaining_bytes = image.len() - bytes_written;
            let chunk_size = remaining_bytes.min(buffer_size);
            let chunk = &image[bytes_written..bytes_written + chunk_size];

            let current_page = (current_address / page_size as u32) as u16;
            let pages_needed = ((chunk_size + page_size - 1) / page_size) as u16;

            // Load chunk to buffer via broadcast, in as many passes as configured
            for _ in 0..self.broadcast_load_passes {
                self.broadcast_load_chunk_to_buffer(target, chunk, page_size).await?;
            }

            // Send write_flash via broadcast (repeat a few times for reliability)
            for _ in 0..3 {
                match target {
                    bootloader::TARGET_NRF51 => self.nrf51.broadcast_write_flash(&mut self.bllink, 0, current_page, pages_needed).await?,
                    bootloader::TARGET_STM32 => self.stm32.broadcast_write_flash(&mut self.bllink, 0, current_page, pages_needed).await?,
                    _ => unreachable!(),
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            }

            // Wait for flash operation to complete on all devices
            tokio::time::sleep(broadcast_write_flash_time(target, current_page, pages_needed, page_size)).await;

            bytes_written += chunk_size;
            current_address += chunk_size as u32;

            if let Some(callback) = progress_callback.as_mut() {
                callback(bytes_written.min(image_len), image_len);
            }
        }

        Ok(())
    }

    /// Load a chunk of data into the bootloader's buffer pages via broadcast
    async fn broadcast_load_chunk_to_buffer(&mut self, target: u8, chunk: &[u8], page_size: usize) -> anyhow::Result<()> {
        let mut chunk_offset = 0;
        let mut buffer_page = 0u16;

        while chunk_offset < chunk.len() {
            let remaining_in_chunk = chunk.len() - chunk_offset;
            let bytes_to_write = remaining_in_chunk.min(page_size);

            let mut page_offset = 0u16;
            let mut bytes_written_to_page = 0;

            while bytes_written_to_page < bytes_to_write {
                let remaining_in_page = bytes_to_write - bytes_written_to_page;
                let load_size = remaining_in_page.min(25);

                let data_slice = &chunk[chunk_offset + bytes_written_to_page..chunk_offset + bytes_written_to_page + load_size];

                match target {
                    bootloader::TARGET_NRF51 => {
                        self.nrf51.broadcast_load_buffer(&mut self.bllink, buffer_page, page_offset, data_slice).await?;
                    },
                    bootloader::TARGET_STM32 => {
                        self.stm32.broadcast_load_buffer(&mut self.bllink, buffer_page, page_offset, data_slice).await?;
                    },
                    _ => return Err(anyhow::anyhow!("Invalid bootloader target: 0x{:02X}", target)),
                }

                if !self.broadcast_packet_delay.is_zero() {
                    tokio::time::sleep(self.broadcast_packet_delay).await;
                }

                page_offset += load_size as u16;
                bytes_written_to_page += load_size;
            }

            chunk_offset += bytes_to_write;
            buffer_page += 1;
        }

        Ok(())
    }

    /// Verify flash contents by CRC32
    ///
    /// Compares the CRC32 of the whole image, with its last page padded with
    /// 0xFF as it is flashed, against CMD_RANGE_CRC over the same range. Only if
    /// that differs is each page checked with CMD_PAGE_CRC, to find the ones
    /// that have to be flashed again.
    /// Returns the list of page numbers that have mismatched CRCs.
    ///
    /// # Arguments
    /// * `target` - The bootloader target
    /// * `start_address` - The starting address in flash
    /// * `image` - The expected firmware image data
    ///
    /// # Returns
    /// A vector of page numbers that failed CRC verification (empty = all good)
    pub async fn verify_flash_crc(
        &mut self,
        target: u8,
        start_address: u32,
        image: &[u8],
    ) -> anyhow::Result<Vec<u16>> {
        let page_size = match target {
            bootloader::TARGET_NRF51 => self.nrf51_info.page_size() as usize,
            bootloader::TARGET_STM32 => self.stm32_info.page_size() as usize,
            _ => return Err(anyhow::anyhow!("Invalid bootloader target: 0x{:02X}", target)),
        };

        let start_page = (start_address / page_size as u32) as u16;
        let n_pages = ((image.len() + page_size - 1) / page_size) as u16;

        // One CRC over the whole image first, page by page only if it differs
        let mut padded = image.to_vec();
        padded.resize(n_pages as usize * page_size, 0xFF);
        let whole = self.range_crc(target, start_page as u32 * page_size as u32, padded.len() as u32).await?;
        if whole == crc32fast::hash(&padded) {
            return Ok(Vec::new());
        }

        let mut failed_pages = Vec::new();

        for i in 0..n_pages {
            let page = start_page + i;
            let offset = i as usize * page_size;
            let end = (offset + page_size).min(image.len());

            // Compute expected CRC32: pad with 0xFF if the last page is partial
            let mut page_data = vec![0xFFu8; page_size];
            page_data[..end - offset].copy_from_slice(&image[offset..end]);
            let expected_crc = crc32fast::hash(&page_data);

            let actual: PageCrcPacket = match target {
                bootloader::TARGET_NRF51 => self.nrf51.page_crc(&mut self.bllink, page).await?,
                bootloader::TARGET_STM32 => self.stm32.page_crc(&mut self.bllink, page).await?,
                _ => unreachable!(),
            };

            if actual.crc32 != expected_crc {
                failed_pages.push(page);
            }
        }

        Ok(failed_pages)
    }

    /// Get the CRC32 checksum of any byte range of the flash
    ///
    /// # Arguments
    /// * `target` - The bootloader target (TARGET_STM32 or TARGET_NRF51)
    /// * `address` - Start of the range, counted from the start of the flash
    /// * `length` - Length of the range in bytes
    ///
    /// # Returns
    /// The CRC32 (same as `crc32fast::hash`) of the range
    pub async fn range_crc(&mut self, target: u8, address: u32, length: u32) -> anyhow::Result<u32> {
        let packet = match target {
            bootloader::TARGET_NRF51 => self.nrf51.range_crc(&mut self.bllink, address, length).await?,
            bootloader::TARGET_STM32 => self.stm32.range_crc(&mut self.bllink, address, length).await?,
            _ => return Err(anyhow::anyhow!("Invalid bootloader target: 0x{:02X}", target)),
        };
        Ok(packet.crc32)
    }

    /// Re-flash failed pages via unicast, respecting flash erase granularity
    ///
    /// The STM32's flash is organized into sectors (16 KB, 64 KB, or 128 KB)
    /// that must be erased as a whole. The bootloader only triggers a sector
    /// erase when a `write_flash` command starts at a sector boundary. So when
    /// fixing individual failed pages, we must re-flash the entire sector
    /// containing each failed page to ensure a proper erase+write cycle.
    ///
    /// The nRF51 has 1 KB erase pages matching the bootloader page size, so
    /// individual page re-flashing works fine there.
    ///
    /// # Arguments
    /// * `target` - The bootloader target
    /// * `start_address` - The starting address used during the original flash
    /// * `image` - The firmware image data
    /// * `failed_pages` - Page numbers that need re-flashing (from verify_flash_crc)
    pub async fn fix_failed_pages(
        &mut self,
        target: u8,
        start_address: u32,
        image: &[u8],
        failed_pages: &[u16],
    ) -> anyhow::Result<()> {
        let (page_size, n_buff_pages) = match target {
            bootloader::TARGET_NRF51 => (
                self.nrf51_info.page_size() as usize,
                self.nrf51_info.n_buff_page() as usize,
            ),
            bootloader::TARGET_STM32 => (
                self.stm32_info.page_size() as usize,
                self.stm32_info.n_buff_page() as usize,
            ),
            _ => return Err(anyhow::anyhow!("Invalid bootloader target: 0x{:02X}", target)),
        };

        let start_page = (start_address / page_size as u32) as u16;
        let total_image_pages = (image.len() + page_size - 1) / page_size;
        let last_page = start_page + total_image_pages as u16;

        // Determine sectors to re-flash. For nRF51, each page is its own sector.
        // For STM32, expand to full sector boundaries.
        let sectors_to_flash: Vec<(u16, u16)> = if target == bootloader::TARGET_STM32 {
            let sector_starts = &STM32_SECTOR_START_PAGES;

            let mut sectors: Vec<(u16, u16)> = Vec::new();
            for &page in failed_pages {
                // Find which sector this page belongs to
                let sector_start = sector_starts.iter()
                    .rev()
                    .find(|&&s| s <= page)
                    .copied()
                    .unwrap_or(0);
                let sector_end = sector_starts.iter()
                    .find(|&&s| s > sector_start)
                    .copied()
                    .unwrap_or(1024);

                // Clamp to image range
                let range_start = sector_start.max(start_page);
                let range_end = sector_end.min(last_page);

                if !sectors.iter().any(|&(s, _)| s == range_start) {
                    sectors.push((range_start, range_end));
                }
            }
            sectors.sort();
            sectors
        } else {
            // nRF51: each page is its own erase unit
            failed_pages.iter().map(|&p| (p, p + 1)).collect()
        };

        for (sector_start, sector_end) in &sectors_to_flash {
            let n_pages = (*sector_end - *sector_start) as usize;

            // Flash the sector in buffer-sized chunks
            let mut pages_done = 0;
            while pages_done < n_pages {
                let chunk_pages = (n_pages - pages_done).min(n_buff_pages);
                let chunk_flash_page = *sector_start + pages_done as u16;

                // Load chunk into buffer
                for buf_page in 0..chunk_pages {
                    let flash_page = chunk_flash_page + buf_page as u16;
                    let image_offset = (flash_page - start_page) as usize * page_size;

                    let mut page_data = vec![0xFFu8; page_size];
                    if image_offset < image.len() {
                        let end = (image_offset + page_size).min(image.len());
                        page_data[..end - image_offset].copy_from_slice(&image[image_offset..end]);
                    }

                    let mut byte_offset = 0u16;
                    let mut bytes_written = 0;
                    while bytes_written < page_size {
                        let load_size = (page_size - bytes_written).min(25);
                        let data_slice = &page_data[bytes_written..bytes_written + load_size];

                        match target {
                            bootloader::TARGET_NRF51 => {
                                self.nrf51.load_buffer(&mut self.bllink, buf_page as u16, byte_offset, data_slice).await?;
                            },
                            bootloader::TARGET_STM32 => {
                                self.stm32.load_buffer(&mut self.bllink, buf_page as u16, byte_offset, data_slice).await?;
                            },
                            _ => unreachable!(),
                        }

                        byte_offset += load_size as u16;
                        bytes_written += load_size;
                    }
                }

                // Write buffer to flash
                let result = match target {
                    bootloader::TARGET_NRF51 => {
                        self.nrf51.write_flash(&mut self.bllink, 0, chunk_flash_page, chunk_pages as u16).await?
                    },
                    bootloader::TARGET_STM32 => {
                        self.stm32.write_flash(&mut self.bllink, 0, chunk_flash_page, chunk_pages as u16).await?
                    },
                    _ => unreachable!(),
                };

                if !result.is_success() {
                    return Err(anyhow::anyhow!(
                        "Failed to re-flash pages {}-{}: {}",
                        chunk_flash_page, chunk_flash_page + chunk_pages as u16 - 1, result.error()
                    ));
                }

                pages_done += chunk_pages;
            }
        }

        Ok(())
    }

    /// Arm broadcast reception on this Crazyflie
    ///
    /// Sends the broadcast address to the nRF51 bootloader of the connected
    /// Crazyflie and makes this CFLoader broadcast to the same address.
    /// Call it for each Crazyflie once it is in bootloader mode. Groups armed
    /// with different addresses only receive the broadcasts sent to their own.
    ///
    /// # Arguments
    /// * `address` - The 5-byte broadcast address, in the same byte order as the unicast address
    pub async fn set_broadcast_address(&mut self, address: &[u8; 5]) -> anyhow::Result<()> {
        self.nrf51.set_broadcast_address(&mut self.bllink, address).await?;
        self.bllink.set_broadcast_address(address);
        Ok(())
    }

    /// Move this Crazyflie to another radio channel and follow it there
    ///
    /// Sends SET_CHANNEL to the nRF51 bootloader, moves this CFLoader to the
    /// same channel and checks that the bootloader answers there. The
    /// Crazyflie stays on the new channel until it is restarted.
    ///
    /// This lets several Crazyradios, each on its own channel, work on
    /// different Crazyflies at the same time, for example to write
    /// per-Crazyflie data after a firmware broadcast on the common channel.
    ///
    /// # Arguments
    /// * `channel` - The new radio channel, 0 to 125. Keep the channels used
    ///   at the same time a few apart: a 2 Mbit/s channel is about 2 MHz wide.
    pub async fn set_channel(&mut self, channel: u8) -> anyhow::Result<()> {
        let old_channel = self.bllink.channel();
        crazyradio::Channel::from_number(channel)
            .map_err(|e| anyhow::anyhow!("Invalid radio channel {}: {}", channel, e))?;

        // The bootloader moves as soon as it has the command, so a missing
        // acknowledgement does not mean it stayed: look on the new channel first
        let sent = self.nrf51.set_channel(&mut self.bllink, channel).await;
        self.bllink.set_channel(channel)?;
        if self.nrf51.get_info(&mut self.bllink).await.is_ok() {
            return Ok(());
        }

        // Not on the new channel: the command may not have arrived, or the
        // bootloader may not know it
        self.bllink.set_channel(old_channel)?;
        if self.nrf51.get_info(&mut self.bllink).await.is_ok() {
            return Err(match sent {
                Ok(()) => anyhow::anyhow!(
                    "The Crazyflie stayed on channel {}, its bootloader may not support SET_CHANNEL",
                    old_channel
                ),
                Err(e) => anyhow::anyhow!(
                    "SET_CHANNEL did not reach the Crazyflie, it is still on channel {}: {}",
                    old_channel, e
                ),
            });
        }
        Err(anyhow::anyhow!(
            "The Crazyflie answers on neither channel {} nor {}",
            channel, old_channel
        ))
    }

    /// Broadcast flash the STM32 with progress callback
    pub async fn broadcast_flash_stm32_with_progress<F>(
        &mut self,
        start_address: u32,
        image: &[u8],
        progress_callback: Option<F>,
    ) -> anyhow::Result<()>
    where
        F: FnMut(usize, usize),
    {
        self.broadcast_flash_image_with_progress(bootloader::TARGET_STM32, start_address, image, progress_callback).await
    }

    /// Broadcast flash the nRF51 with progress callback
    pub async fn broadcast_flash_nrf51_with_progress<F>(
        &mut self,
        start_address: u32,
        image: &[u8],
        progress_callback: Option<F>,
    ) -> anyhow::Result<()>
    where
        F: FnMut(usize, usize),
    {
        self.broadcast_flash_image_with_progress(bootloader::TARGET_NRF51, start_address, image, progress_callback).await
    }
}