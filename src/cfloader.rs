// High-level interface for the Crazyflie 2.x bootloader
// Provide connectivity to both bootloader on the nRF and STM32
// as well as high-level algorithm to program the Crazyflie 2.x

use crate::Bllink;
use crate::bootloader::{self, Bootloader};
use crate::packets::InfoPacket;

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



}