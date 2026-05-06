//! Boot mode entry for the Crazyflie bootloader.
//!
//! This module handles transitioning a Crazyflie into bootloader mode,
//! either via warm boot (from running firmware) or cold boot (rescue mode).

use anyhow::{anyhow, bail, Result};
use crazyflie_link::{LinkContext, Packet};
use tokio::time::{sleep, Duration};

use crate::Bllink;
use crate::bootloader;

const TARGET_NRF51: u8 = bootloader::TARGET_NRF51;

/// How to enter bootloader mode.
#[derive(Debug, Clone)]
pub enum BootMode {
    /// Warm boot from running firmware. The URI is the radio address of the
    /// running Crazyflie (e.g. `radio://0/80/2M/E7E7E7E7E7`).
    Warm {
        /// URI of the running Crazyflie
        uri: String,
    },
    /// Cold boot / rescue mode. The Crazyflie must be manually put in bootloader
    /// mode by holding the power button for ~3 seconds during power-on.
    Cold,
}

/// Enter bootloader mode and return a [`Bllink`] connection ready for flashing.
///
/// # Warm boot
/// Connects to the running firmware, sends reset-to-bootloader commands to the
/// nRF51, extracts the new bootloader address, and creates a Bllink connection.
///
/// # Cold boot
/// Scans for a Crazyflie already in bootloader mode on the known bootloader
/// addresses (channel 0 and 110, default address E7E7E7E7E7).
pub async fn enter_bootloader(link_context: &LinkContext, mode: BootMode) -> Result<Bllink> {
    match mode {
        BootMode::Warm { uri } => warm_boot(link_context, &uri).await,
        BootMode::Cold => cold_boot().await,
    }
}

async fn warm_boot(link_context: &LinkContext, uri: &str) -> Result<Bllink> {
    // Connect with safelink disabled so we can send bootloader commands
    let separator = if uri.contains('?') { "&" } else { "?" };
    let link = link_context
        .open_link(&format!("{}{}safelink=0", uri, separator))
        .await?;

    let new_address = reset_and_get_bootloader_address(&link).await;
    link.close().await;

    let new_address = new_address?;
    let arr: [u8; 5] = new_address
        .try_into()
        .map_err(|_| anyhow!("Bootloader address must be exactly 5 bytes"))?;

    let bllink = Bllink::new(Some(&arr)).await?;
    Ok(bllink)
}

async fn cold_boot() -> Result<Bllink> {
    let context = LinkContext::new();
    let found = context
        .scan_selected(vec![
            "radio://0/110/2M/E7E7E7E7E7",
            "radio://0/0/2M/E7E7E7E7E7",
        ])
        .await?;

    if found.is_empty() {
        bail!("No Crazyflie in bootloader mode found. Hold the power button for ~3 seconds during power-on to enter bootloader mode.");
    }

    // Connect to the first found bootloader and create Bllink with default address
    let bllink = Bllink::new(None).await?;
    Ok(bllink)
}

/// Send reset-to-bootloader commands and extract the new bootloader address.
///
/// Returns a 5-byte address: [0xB1, addr3_rev, addr2_rev, addr1_rev, addr0_rev]
async fn reset_and_get_bootloader_address(
    link: &crazyflie_link::Connection,
) -> Result<Vec<u8>> {
    // Disable safelink on the nRF51 side
    let packet: Packet = vec![0xFF, TARGET_NRF51, 0xFF, 0x05, 0x00].into();
    link.send_packet(packet).await?;

    // Send reset-to-bootloader command
    let packet: Packet = vec![0xFF, TARGET_NRF51, 0xFF].into();
    link.send_packet(packet).await?;

    // Wait for response containing the new bootloader address
    let mut new_address = Vec::new();
    loop {
        let packet = tokio::select! {
            result = link.recv_packet() => result?,
            _ = sleep(Duration::from_millis(100)) => {
                return Err(anyhow!("Timeout waiting for bootloader address response"));
            }
        };
        let data = packet.get_data();
        if data.len() > 2 && data[0..2] == [TARGET_NRF51, 0xFF] {
            new_address.push(0xb1);
            for byte in data[2..6].iter().rev() {
                new_address.push(*byte);
            }
            break;
        }
    }

    // Send reset confirmation commands (10 times)
    for _ in 0..10 {
        let packet: Packet = vec![0xFF, TARGET_NRF51, 0xF0, 0x00].into();
        link.send_packet(packet).await?;
    }
    sleep(Duration::from_millis(500)).await;

    Ok(new_address)
}
