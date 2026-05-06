//! Top-level flash orchestrator.
//!
//! This module ties together boot mode entry, softdevice validation,
//! STM32/nRF51 flashing, and deck flashing into a single `flash()` function.

use anyhow::{bail, Result};
use crazyflie_link::LinkContext;
use tokio::time::{sleep, Duration};

use crate::boot_entry::{self, BootMode};
use crate::firmware::{FirmwareImage, FlashStartOverride, FlashTarget};
use crate::progress::FlashProgress;
use crate::softdevice::Softdevice;
use crate::CFLoader;

/// Configuration for a flash operation.
pub struct FlashConfig<T: crazyflie_lib::TocCache = crazyflie_lib::NoTocCache> {
    /// How to enter bootloader mode
    pub boot_mode: BootMode,
    /// URI of the Crazyflie (needed for deck phase reconnection after reboot).
    /// Required if deck images are provided, or for warm boot.
    pub uri: Option<String>,
    /// Firmware images to flash
    pub images: Vec<FirmwareImage>,
    /// Progress callback
    pub progress: Option<Box<dyn FnMut(FlashProgress) + Send>>,
    /// TOC cache for crazyflie-lib connections (deck flashing phase)
    pub toc_cache: T,
}

/// Execute the full flash sequence.
///
/// This is the main entry point for flashing firmware to a Crazyflie.
/// It handles:
/// 1. Entering bootloader mode (warm or cold boot)
/// 2. Softdevice detection and validation
/// 3. Flashing STM32 and/or nRF51 firmware
/// 4. Resetting to firmware mode
/// 5. Flashing deck firmware (if any)
///
/// # Arguments
/// * `link_context` - The link context for radio communication
/// * `config` - Flash configuration including boot mode, images, and callbacks
pub async fn flash<T: crazyflie_lib::TocCache>(link_context: &LinkContext, mut config: FlashConfig<T>) -> Result<()> {
    // Separate images into bootloader targets and deck targets
    let (bootloader_images, deck_images): (Vec<_>, Vec<_>) = config
        .images
        .into_iter()
        .partition(|img| matches!(img.target, FlashTarget::Stm32 { .. } | FlashTarget::Nrf51 { .. }));

    // Validate: deck images need a URI for reconnection
    if !deck_images.is_empty() && config.uri.is_none() {
        bail!("Deck firmware images provided but no URI specified. A URI is needed to reconnect after rebooting for deck flashing.");
    }

    // -- Bootloader phase --
    if !bootloader_images.is_empty() {
        emit(&mut config.progress, FlashProgress::EnteringBootloader);

        let bllink = boot_entry::enter_bootloader(link_context, config.boot_mode.clone()).await?;
        let mut loader = CFLoader::new(bllink).await?;

        emit(&mut config.progress, FlashProgress::BootloaderConnected);

        // Detect installed softdevice
        let nrf51_info = loader.nrf51_info();
        let installed_sd = Softdevice::from_start_page(nrf51_info.flash_start());

        // Sort images: softdevice providers first, then nRF51 fw, then STM32
        let mut sorted_images = bootloader_images;
        sorted_images.sort_by_key(|img| {
            if !img.provides.is_empty() {
                0 // softdevice images first
            } else if matches!(img.target, FlashTarget::Nrf51 { .. }) {
                1 // then nRF51 firmware
            } else {
                2 // then STM32
            }
        });

        for image in &sorted_images {
            // Validate softdevice compatibility for nRF51 images
            if matches!(image.target, FlashTarget::Nrf51 { .. }) {
                let needs_update = Softdevice::check_compatibility(
                    &installed_sd,
                    &image.requires,
                    &image.provides,
                )?;
                if !image.provides.is_empty() && !needs_update {
                    // Softdevice already installed, skip this image
                    continue;
                }
            }

            let (target_name, start_address) = match &image.target {
                FlashTarget::Stm32 { start_override } => {
                    let info = loader.stm32_info();
                    let addr = resolve_start_address(start_override, info);
                    ("stm32".to_string(), addr)
                }
                FlashTarget::Nrf51 { start_override } => {
                    let info = loader.nrf51_info();
                    let addr = resolve_start_address(start_override, info);
                    ("nrf51".to_string(), addr)
                }
                FlashTarget::Deck { .. } => unreachable!("Deck images filtered out above"),
            };

            let total_bytes = image.data.len();
            emit(
                &mut config.progress,
                FlashProgress::FlashingTarget {
                    target: target_name.clone(),
                    bytes_written: 0,
                    total_bytes,
                },
            );

            // Create progress callback that emits FlashProgress events
            let target_for_cb = target_name.clone();
            let progress_ref = &mut config.progress;
            let progress_callback = |bytes_written: usize, total: usize| {
                emit(
                    progress_ref,
                    FlashProgress::FlashingTarget {
                        target: target_for_cb.clone(),
                        bytes_written,
                        total_bytes: total,
                    },
                );
            };

            match &image.target {
                FlashTarget::Stm32 { .. } => {
                    loader
                        .flash_stm32_with_progress(start_address, &image.data, Some(progress_callback))
                        .await?;
                }
                FlashTarget::Nrf51 { .. } => {
                    loader
                        .flash_nrf51_with_progress(start_address, &image.data, Some(progress_callback))
                        .await?;
                }
                _ => unreachable!(),
            }

            emit(
                &mut config.progress,
                FlashProgress::FlashComplete {
                    target: target_name,
                },
            );
        }

        // Reset to firmware
        emit(&mut config.progress, FlashProgress::ResettingToFirmware);
        loader.reset_to_firmware().await?;

        // Wait for reboot if deck phase follows
        if !deck_images.is_empty() {
            let wait_secs = 7; // Conservative: accounts for AI-deck slow startup
            emit(
                &mut config.progress,
                FlashProgress::WaitingForReboot {
                    estimated_seconds: wait_secs,
                },
            );
            sleep(Duration::from_secs(wait_secs as u64)).await;
        }
    }

    // -- Deck phase --
    if !deck_images.is_empty() {
        let uri = config.uri.as_ref().unwrap(); // validated above
        crate::deck::flash_decks(
            link_context,
            uri,
            &deck_images,
            &mut config.progress,
            config.toc_cache.clone(),
        )
        .await?;
    }

    emit(&mut config.progress, FlashProgress::Complete);
    Ok(())
}

/// Resolve the start address from an override or bootloader info defaults.
fn resolve_start_address(
    start_override: &Option<FlashStartOverride>,
    info: &crate::packets::InfoPacket,
) -> u32 {
    match start_override {
        Some(FlashStartOverride::Address(addr)) => *addr,
        Some(FlashStartOverride::Page(page)) => *page as u32 * info.page_size() as u32,
        None => info.flash_start() as u32 * info.page_size() as u32,
    }
}

/// Helper to emit a progress event if a callback is set.
fn emit(progress: &mut Option<Box<dyn FnMut(FlashProgress) + Send>>, event: FlashProgress) {
    if let Some(cb) = progress {
        cb(event);
    }
}
