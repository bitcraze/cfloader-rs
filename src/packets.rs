//! # Packet structures for bootloader communication
//!
//! This module defines the data structures used to parse responses from the
//! Crazyflie bootloader protocol.

use std::{fmt::Debug, fmt::Display};

// Info packet structure:
// [0xff, target, 0x10, pageSize, nBuffPage, nFlashPage, flashStart, cpuId, version]
//
// Command: 0x10
// pageSize (2 bytes): Size of flash and buffer pages
// nBuffPage (2 bytes): Number of RAM buffer pages available
// nFlashPage (2 bytes): Total number of flash pages
// flashStart (2 bytes): Start flash page of firmware
// cpuId (12 bytes): Legacy CPU ID (should be ignored)
// version (1 byte): Protocol version

/// Information packet retrieved from a bootloader
///
/// Contains metadata about the bootloader and flash memory configuration,
/// including page sizes, buffer capacity, and flash layout information.
///
/// # Fields
///
/// * `page_size` - Size of flash and buffer pages in bytes
/// * `n_buff_page` - Number of RAM buffer pages available
/// * `n_flash_page` - Total number of flash pages
/// * `flash_start` - Start flash page of firmware area
/// * `cpu_id` - Legacy CPU ID (12 bytes, should be ignored)
/// * `version` - Bootloader protocol version
pub struct InfoPacket {
    page_size: u16,
    n_buff_page: u16,
    n_flash_page: u16,
    flash_start: u16,
    cpu_id: [u8; 12],
    version: u8,
}

impl InfoPacket {
    /// Create an InfoPacket from raw bytes
    ///
    /// Parses a raw byte slice into an `InfoPacket` structure.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Raw byte slice containing the info packet data (minimum 22 bytes)
    ///
    /// # Panics
    ///
    /// Panics if `bytes` is shorter than 22 bytes
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < 22 {
            panic!("Invalid InfoPacket length: expected at least 22 bytes, got {}", bytes.len());
        }
        InfoPacket {
            page_size: u16::from_le_bytes([bytes[1], bytes[2]]),
            n_buff_page: u16::from_le_bytes([bytes[3], bytes[4]]),
            n_flash_page: u16::from_le_bytes([bytes[5], bytes[6]]),
            flash_start: u16::from_le_bytes([bytes[7], bytes[8]]),
            cpu_id: [bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17], bytes[18], bytes[19], bytes[20]],
            version: bytes[21],
        }
    }

    /// Get the page size in bytes
    ///
    /// The page size is the unit of flash memory that can be erased or written at once.
    pub fn page_size(&self) -> u16 {
        self.page_size
    }

    /// Get the number of RAM buffer pages
    ///
    /// This is the number of pages available in RAM for staging data before flashing.
    pub fn n_buff_page(&self) -> u16 {
        self.n_buff_page
    }

    /// Get the total number of flash pages
    ///
    /// This is the total flash capacity divided by the page size.
    pub fn n_flash_page(&self) -> u16 {
        self.n_flash_page
    }

    /// Get the flash start page number
    ///
    /// This is the first page where user firmware can be written.
    /// Pages before this are typically reserved for the bootloader itself.
    pub fn flash_start(&self) -> u16 {
        self.flash_start
    }

    /// Get the bootloader protocol version
    ///
    /// Used to determine which features and commands are supported.
    pub fn version(&self) -> u8 {
        self.version
    }
}

impl Debug for InfoPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("InfoPacket")
            .field("page_size", &self.page_size)
            .field("n_buff_page", &self.n_buff_page)
            .field("n_flash_page", &self.n_flash_page)
            .field("flash_start", &self.flash_start)
            .field("cpu_id", &self.cpu_id)
            .field("version", &self.version)
            .finish()
    }
}

impl Display for InfoPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "InfoPacket {{ page_size: {}, n_buff_page: {}, n_flash_page: {}, flash_start: {}, cpu_id: {:?}, version: {} }}",
               self.page_size, self.n_buff_page, self.n_flash_page, self.flash_start, self.cpu_id, self.version)
    }
}

/// Response from reading the bootloader's RAM buffer
///
/// Contains the data read from a specific page and address in the buffer.
pub struct BufferReadPacket {
    /// The buffer page number that was read
    pub page: u16,
    /// The address offset within the page
    pub address: u16,
    /// The data read from the buffer
    pub data: Vec<u8>,
}

impl BufferReadPacket {
    /// Create a BufferReadPacket from raw bytes
    ///
    /// # Arguments
    ///
    /// * `bytes` - Raw byte slice containing the response data
    ///
    /// # Panics
    ///
    /// Panics if `bytes` is shorter than 5 bytes
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < 5 {
            panic!("Invalid BufferReadPacket length");
        }
        BufferReadPacket {
            page: u16::from_le_bytes([bytes[1], bytes[2]]),
            address: u16::from_le_bytes([bytes[3], bytes[4]]),
            data: bytes[5..].to_vec(),
        }
    }
}

impl Debug for BufferReadPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("BufferReadPacket")
            .field("page", &self.page)
            .field("address", &self.address)
            .field("data", &self.data)
            .finish()
    }
}

/// Response from a flash write operation
///
/// Contains the status of the flash write operation, including whether it
/// completed and any error that occurred.
pub struct FlashWriteResponse {
    /// Non-zero if the operation has completed
    pub done: u8,
    /// Error code (0 = no error)
    pub error: u8,
}

impl FlashWriteResponse {
    /// Create a FlashWriteResponse from raw bytes
    ///
    /// # Arguments
    ///
    /// * `bytes` - Raw byte slice containing the response data
    ///
    /// # Panics
    ///
    /// Panics if `bytes` is shorter than 3 bytes
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < 3 {
            panic!("Invalid FlashWriteResponse length");
        }
        FlashWriteResponse {
            done: bytes[1],
            error: bytes[2],
        }
    }

    /// Check if the flash operation has completed
    ///
    /// # Returns
    ///
    /// `true` if the operation is done, `false` if still in progress
    pub fn is_done(&self) -> bool {
        self.done != 0
    }

    /// Get the error status as an enum
    ///
    /// # Returns
    ///
    /// The error code converted to an error variant
    pub fn error(&self) -> FlashError {
        FlashError::from(self.error)
    }

    /// Check if the flash operation completed successfully
    ///
    /// # Returns
    ///
    /// `true` if the operation is done and no error occurred
    pub fn is_success(&self) -> bool {
        self.is_done() && self.error() == FlashError::NoError
    }
}

impl Debug for FlashWriteResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("FlashWriteResponse")
            .field("done", &self.done)
            .field("error", &self.error)
            .finish()
    }
}

/// Response from a flash status query
///
/// This is an alias for [`FlashWriteResponse`] as both use the same response format.
pub type FlashStatusResponse = FlashWriteResponse;

/// Response from reading flash memory
///
/// Contains the data read from a specific page and address in flash memory.
pub struct FlashReadPacket {
    /// The flash page number that was read
    pub page: u16,
    /// The address offset within the page
    pub address: u16,
    /// The data read from flash memory
    pub data: Vec<u8>,
}

impl FlashReadPacket {
    /// Create a FlashReadPacket from raw bytes
    ///
    /// # Arguments
    ///
    /// * `bytes` - Raw byte slice containing the response data
    ///
    /// # Panics
    ///
    /// Panics if `bytes` is shorter than 5 bytes
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < 5 {
            panic!("Invalid FlashReadPacket length");
        }
        FlashReadPacket {
            page: u16::from_le_bytes([bytes[1], bytes[2]]),
            address: u16::from_le_bytes([bytes[3], bytes[4]]),
            data: bytes[5..].to_vec(),
        }
    }
}

impl Debug for FlashReadPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("FlashReadPacket")
            .field("page", &self.page)
            .field("address", &self.address)
            .field("data", &self.data)
            .finish()
    }
}

/// Error codes for flash operations
///
/// Represents the possible error conditions that can occur during flash
/// memory operations like erase and programming.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlashError {
    /// No error occurred
    NoError = 0,
    /// The specified address is outside valid boundaries
    AddressOutOfBounds = 1,
    /// Flash erase operation failed
    FlashEraseFailed = 2,
    /// Flash programming operation failed
    FlashProgrammingFailed = 3,
}

impl From<u8> for FlashError {
    fn from(value: u8) -> Self {
        match value {
            0 => FlashError::NoError,
            1 => FlashError::AddressOutOfBounds,
            2 => FlashError::FlashEraseFailed,
            3 => FlashError::FlashProgrammingFailed,
            _ => FlashError::NoError, // Default to no error for unknown codes
        }
    }
}

impl Display for FlashError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FlashError::NoError => write!(f, "No error"),
            FlashError::AddressOutOfBounds => write!(f, "Addresses are outside of authorized boundaries"),
            FlashError::FlashEraseFailed => write!(f, "Flash erase failed"),
            FlashError::FlashProgrammingFailed => write!(f, "Flash programming failed"),
        }
    }
}