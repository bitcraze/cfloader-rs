//! # Crazyflie firmware flasher
//!
//! This crate provides a complete solution for flashing firmware to the
//! Crazyflie 2.x family of quadcopters. It handles the full flash sequence:
//! boot mode entry, STM32/nRF51 firmware flashing, softdevice management,
//! and expansion deck firmware updates.
//!
//! # Supported platforms
//! - Crazyflie 2.0
//! - Crazyflie 2.1
//! - Crazyflie Bolt
//! - Crazyflie Brushless 2.1
//!
//! # Quick start
//!
//! The simplest way to flash firmware is via [`flasher::flash`]:
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! use cfloader::firmware;
//! use cfloader::flasher::{self, FlashConfig};
//! use cfloader::boot_entry::BootMode;
//!
//! let zip_data = std::fs::read("firmware.zip")?;
//! let (_info, images) = firmware::parse_firmware_zip(&zip_data)?;
//!
//! let link_context = crazyflie_link::LinkContext::new();
//! flasher::flash(&link_context, FlashConfig {
//!     boot_mode: BootMode::Warm { uri: "radio://0/80/2M/E7E7E7E7E7".to_string() },
//!     uri: Some("radio://0/80/2M/E7E7E7E7E7".to_string()),
//!     images,
//!     progress: None,
//!     toc_cache: crazyflie_lib::NoTocCache,
//! }).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture
//!
//! The crate has two decoupled APIs:
//! - **Firmware parsing** ([`firmware`]): Parse zip archives or create images from raw binaries
//! - **Flashing** ([`flasher`]): Takes firmware images and handles the full flash sequence
//!
//! For low-level bootloader access, the [`CFLoader`], [`Bootloader`], and [`Bllink`]
//! types are available directly.

#![deny(missing_docs)]

mod bllink;
pub mod boot_entry;
pub mod bootloader;
mod cfloader;
pub(crate) mod deck;
pub mod firmware;
pub mod flasher;
pub mod packets;
pub mod progress;
pub mod softdevice;

pub use bllink::Bllink;
pub use bootloader::Bootloader;
pub use cfloader::CFLoader;
