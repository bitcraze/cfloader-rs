//! Deck firmware flashing via crazyflie-lib.
//!
//! Handles discovering attached decks, flashing firmware to them,
//! and managing the reboot sequence between decks.

use anyhow::{anyhow, bail, Result};
use crazyflie_link::LinkContext;
use crazyflie_lib::subsystems::memory::{DeckMemory, MemoryType};
use crazyflie_lib::Crazyflie;
use futures::StreamExt;
use tokio::time::{sleep, Duration};

use crate::firmware::{FirmwareImage, FlashTarget};
use crate::progress::FlashProgress;

const AIDECK_SECTION_NAME: &str = "bcAI:esp";

/// Flash deck firmware images.
///
/// Connects to the running Crazyflie firmware, discovers attached decks,
/// and flashes each deck image sequentially with reboots between them.
pub(crate) async fn flash_decks<T: crazyflie_lib::TocCache>(
    link_context: &LinkContext,
    uri: &str,
    deck_images: &[FirmwareImage],
    progress: &mut Option<Box<dyn FnMut(FlashProgress) + Send>>,
    toc_cache: T,
) -> Result<()> {
    emit(progress, FlashProgress::ConnectingForDeckPhase);

    // First connection: discover decks and determine timing
    let cf = connect_crazyflie(link_context, uri, toc_cache.clone()).await?;

    let deck_sections = get_deck_section_names(&cf).await?;
    let has_aideck = deck_sections.iter().any(|name| name.starts_with(AIDECK_SECTION_NAME));
    let reboot_delay_ms: u64 = if has_aideck { 10000 } else { 3000 };

    emit(
        progress,
        FlashProgress::DiscoveringDecks {
            found: deck_sections.clone(),
        },
    );

    cf.disconnect().await;

    // Flash each deck image
    for image in deck_images.iter() {
        let deck_name = match &image.target {
            FlashTarget::Deck { name } => name.clone(),
            _ => continue, // shouldn't happen, but skip non-deck images
        };

        // Reboot before each deck flash to ensure a clean state
        reboot(link_context, uri).await?;

        let wait_secs = (reboot_delay_ms / 1000) as u32;
        emit(
            progress,
            FlashProgress::WaitingForReboot {
                estimated_seconds: wait_secs,
            },
        );
        sleep(Duration::from_millis(reboot_delay_ms)).await;

        let cf = connect_crazyflie(link_context, uri, toc_cache.clone()).await?;

        // Drain firmware console to stderr so debug prints and asserts are visible
        let mut console_stream = cf.console.line_stream_no_history().await;
        let console_task = tokio::spawn(async move {
            while let Some(line) = console_stream.next().await {
                eprintln!("[cf] {}", line);
            }
        });

        flash_single_deck(&cf, &deck_name, &image.data, progress).await?;

        console_task.abort();
        cf.disconnect().await;

        emit(
            progress,
            FlashProgress::DeckFlashComplete {
                name: deck_name.clone(),
            },
        );
    }

    // Final reboot so the Crazyflie starts with the new deck firmware
    reboot(link_context, uri).await?;

    Ok(())
}

/// Flash firmware to a single deck section.
async fn flash_single_deck(
    cf: &Crazyflie,
    deck_name: &str,
    data: &[u8],
    progress: &mut Option<Box<dyn FnMut(FlashProgress) + Send>>,
) -> Result<()> {
    let memories = cf.memory.get_memories(Some(MemoryType::DeckMemory));
    if memories.is_empty() {
        bail!("No DeckMemory found on Crazyflie");
    }

    let deck_memory = cf
        .memory
        .open_memory::<DeckMemory>(memories[0].clone())
        .await
        .ok_or_else(|| anyhow!("DeckMemory not found"))?
        .map_err(|e| anyhow!("Failed to open DeckMemory: {:?}", e))?;

    let section = deck_memory
        .sections()
        .iter()
        .find(|s| s.name() == deck_name)
        .ok_or_else(|| {
            anyhow!(
                "Deck section '{}' not found. Attached deck sections: {:?}",
                deck_name,
                deck_memory
                    .sections()
                    .iter()
                    .map(|s| s.name().to_string())
                    .collect::<Vec<_>>()
            )
        })?;

    // Activate bootloader if not already active
    let bootloader_active = section.bootloader_active().await?;
    if !bootloader_active {
        section.reset_to_bootloader().await?;
        sleep(Duration::from_millis(10)).await;

        let bootloader_active = section.bootloader_active().await?;
        if !bootloader_active {
            bail!(
                "Failed to activate bootloader for deck section '{}'",
                deck_name
            );
        }
    }

    // Set the new firmware size before writing data.
    // The firmware uses this to configure the flash operation (erase size, block count).
    section
        .set_new_firmware_size(data.len() as u32)
        .await?;

    let total_bytes = data.len();
    let name_for_cb = deck_name.to_string();
    let progress_ref = &mut *progress;
    let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
        emit(
            progress_ref,
            FlashProgress::FlashingDeck {
                name: name_for_cb.clone(),
                bytes_written,
                total_bytes,
            },
        );
    };

    section
        .write_with_progress(0, data, progress_callback)
        .await?;

    Ok(())
}

/// Get names of all deck memory sections.
async fn get_deck_section_names(cf: &Crazyflie) -> Result<Vec<String>> {
    let memories = cf.memory.get_memories(Some(MemoryType::DeckMemory));
    if memories.is_empty() {
        return Ok(Vec::new());
    }

    let deck_memory = cf
        .memory
        .open_memory::<DeckMemory>(memories[0].clone())
        .await
        .ok_or_else(|| anyhow!("DeckMemory not found"))?
        .map_err(|e| anyhow!("Failed to open DeckMemory: {:?}", e))?;

    Ok(deck_memory
        .sections()
        .iter()
        .map(|s| s.name().to_string())
        .collect())
}

/// Connect to the Crazyflie in firmware mode.
async fn connect_crazyflie<T: crazyflie_lib::TocCache>(
    link_context: &LinkContext,
    uri: &str,
    toc_cache: T,
) -> Result<Crazyflie> {
    // TODO: Handle protocol version incompatibility gracefully.
    // Ancient firmware may have a CRTP protocol version too old for crazyflie-lib.
    // In that case, we should error with a helpful message like:
    // "Deck flashing not supported for this firmware version.
    //  Use an older cfloader or crazyflie-clients-python to flash deck firmware."
    Crazyflie::connect_from_uri(link_context, uri, toc_cache)
        .await
        .map_err(|e| anyhow!("Failed to connect to Crazyflie for deck flashing: {:?}", e))
}

/// Reboot the Crazyflie by sending reset commands.
async fn reboot(link_context: &LinkContext, uri: &str) -> Result<()> {
    let separator = if uri.contains('?') { "&" } else { "?" };
    let link = link_context
        .open_link(&format!("{}{}safelink=0", uri, separator))
        .await?;

    // Send reset init + reset to firmware
    let reset_init: crazyflie_link::Packet = vec![0xFF, 0xFE, 0xFF].into();
    link.send_packet(reset_init).await?;

    let reset_fw: crazyflie_link::Packet = vec![0xFF, 0xFE, 0xF0, 0x01].into();
    link.send_packet(reset_fw).await?;

    sleep(Duration::from_millis(500)).await;
    link.close().await;

    Ok(())
}

/// Helper to emit a progress event if a callback is set.
fn emit(progress: &mut Option<Box<dyn FnMut(FlashProgress) + Send>>, event: FlashProgress) {
    if let Some(cb) = progress {
        cb(event);
    }
}
