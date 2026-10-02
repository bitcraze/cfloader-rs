/// Multi-device broadcast firmware upgrade example
///
/// Connects to multiple running Crazyflies, jumps them to bootloader,
/// broadcasts firmware, and verifies via CRC on each device.
///
/// Usage:
///   cargo run --example broadcast_flash_multi -- --uri <uri> [--uri <uri> ...] --stm32-fw <path> [--nrf51-fw <path>]
///       [--broadcast-address BCE7E7E7E7]
///
/// Use different broadcast addresses to flash separate groups of Crazyflies.

use cfloader::{Bllink, CFLoader, bootloader};
use crazyflie_link::{LinkContext, Packet};
use std::time::Instant;
use anyhow::Result;
use clap::Parser;
use std::io::{self, Write};
use tokio::time::{sleep, Duration};

const TARGET_NRF51: u8 = 0xFE;

#[derive(Parser)]
#[command(about = "Broadcast firmware to multiple Crazyflies (warm reset)")]
struct Args {
    /// URI of a running Crazyflie to flash (repeat for each Crazyflie)
    #[arg(long = "uri", required = true)]
    uris: Vec<String>,

    /// STM32 firmware binary file
    #[arg(long)]
    stm32_fw: String,

    /// nRF51 firmware binary file (optional)
    #[arg(long)]
    nrf51_fw: Option<String>,

    /// Broadcast address to arm the Crazyflies with (hex, same byte order as radio URIs)
    #[arg(long, default_value = "BCE7E7E7E7", value_parser = parse_address)]
    broadcast_address: [u8; 5],
}

fn parse_address(s: &str) -> Result<[u8; 5], String> {
    if s.len() != 10 {
        return Err("expected 10 hex digits".into());
    }
    let mut address = [0u8; 5];
    for (i, byte) in address.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(address)
}

/// Reset a Crazyflie to bootloader and return its Bllink + bootloader address.
/// Follows the same pattern as cfcli's restart_and_get_bllink.
async fn restart_and_get_bllink(context: &LinkContext, uri: &str) -> Result<(Bllink, [u8; 5])> {
    let link = context.open_link(&format!("{}?safelink=0", uri)).await?;
    let addr = reset_and_get_bootloader_address(&link).await?;
    link.close().await;

    // Use the link context's radio rather than opening the Crazyradio a second
    // time. With a second handle, retrying after a setup that timed out failed
    // with UsbError(Busy).
    let radio = context.get_radio(0).await?;
    let bllink = Bllink::new_with_radio(radio, Some(&addr)).await?;
    Ok((bllink, addr))
}

/// Get bootloader address from a running Crazyflie (disable safelink, RESET_INIT, RESET).
/// Mirrors cfcli's reset_and_get_bootloader_address exactly.
async fn reset_and_get_bootloader_address(link: &crazyflie_link::Connection) -> Result<[u8; 5]> {
    // Disable safelink so we can send bootloader commands to the nRF51
    let packet: Packet = vec![0xFF, TARGET_NRF51, 0xFF, 0x05, 0x00].into();
    link.send_packet(packet).await?;

    // Send RESET_INIT to get the device address, again every 200 ms until it
    // is answered: the request or its answer can be lost
    let reset_init: Packet = vec![0xFF, TARGET_NRF51, 0xFF].into();
    link.send_packet(reset_init.clone()).await?;
    let mut last_sent = tokio::time::Instant::now();

    let mut bl_address = [0u8; 5];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2000);
    loop {
        let packet = tokio::select! {
            result = link.recv_packet() => result?,
            _ = sleep(Duration::from_millis(100)) => {
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!("Timeout waiting for bootloader address response");
                }
                if last_sent.elapsed() >= Duration::from_millis(200) {
                    link.send_packet(reset_init.clone()).await?;
                    last_sent = tokio::time::Instant::now();
                }
                continue;
            }
        };
        let data = packet.get_data();
        if data.len() > 5 && data[0..2] == [TARGET_NRF51, 0xFF] {
            bl_address[0] = 0xb1;
            bl_address[1] = data[5];
            bl_address[2] = data[4];
            bl_address[3] = data[3];
            bl_address[4] = data[2];
            break;
        }
    }

    // Send RESET command (to bootloader) multiple times for reliability
    for _ in 0..10 {
        let packet: Packet = vec![0xFF, TARGET_NRF51, 0xF0, 0x00].into();
        link.send_packet(packet).await?;
    }
    sleep(Duration::from_millis(500)).await;

    Ok(bl_address)
}

/// Reset a Crazyflie to bootloader, connect via Bllink, verify and arm broadcast.
/// Returns the bootloader address for later use.
async fn reset_and_setup_device(context: &LinkContext, uri: &str, device_num: usize, broadcast_address: &[u8; 5]) -> Result<[u8; 5]> {
    println!("Connecting to {} ...", uri);

    let (bllink, addr) = restart_and_get_bllink(context, uri).await?;
    println!("  Bootloader address: {:02X?}", addr);

    let mut loader = CFLoader::new(bllink).await?;

    println!("  nRF51 version: 0x{:02X}", loader.nrf51_info().version());
    println!("  STM32 version: 0x{:02X}", loader.stm32_info().version());

    if loader.nrf51_info().version() < 0x11 {
        anyhow::bail!(
            "Device {} nRF51 bootloader version 0x{:02X} does not support broadcast. Need >= 0x11.",
            device_num, loader.nrf51_info().version()
        );
    }

    loader.set_broadcast_address(broadcast_address).await?;
    println!("  Broadcast armed on {:02X?}", broadcast_address);

    Ok(addr)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let stm32_fw = std::fs::read(&args.stm32_fw)?;
    println!("Loaded STM32 firmware: {} bytes", stm32_fw.len());

    let nrf51_fw = match &args.nrf51_fw {
        Some(path) => {
            let data = std::fs::read(path)?;
            println!("Loaded nRF51 firmware: {} bytes", data.len());
            Some(data)
        }
        None => None,
    };

    // --- Phase 1: Jump each device to bootloader, verify, and arm broadcast ---
    println!("\n=== Phase 1: Reset & Setup ===");

    let context = LinkContext::new();
    let mut bl_addresses: Vec<[u8; 5]> = Vec::new();

    for (i, uri) in args.uris.iter().enumerate() {
        // Getting a Crazyflie into its bootloader occasionally hangs, so give
        // up on an attempt after 10 s and try again
        let mut attempt = 0;
        let addr = loop {
            attempt += 1;
            let setup = reset_and_setup_device(&context, uri, i + 1, &args.broadcast_address);
            match tokio::time::timeout(Duration::from_secs(10), setup).await {
                Ok(Ok(addr)) => break addr,
                Ok(Err(e)) if attempt < 3 => println!("  Setup failed ({}), retrying", e),
                Err(_) if attempt < 3 => println!("  Setup timed out after 10 s, retrying"),
                Ok(Err(e)) => return Err(e),
                Err(_) => anyhow::bail!("Device {}: setup timed out 3 times", i + 1),
            }
        };
        bl_addresses.push(addr);
    }
    drop(context);

    println!("\nAll {} devices ready.", bl_addresses.len());

    // --- Phase 2: Broadcast firmware ---
    println!("\n=== Phase 2: Broadcast ===");

    // Connect to first device for bootloader info (page sizes, etc)
    let bllink = Bllink::new(Some(&bl_addresses[0])).await?;
    let mut loader = CFLoader::new(bllink).await?;
    loader.bllink_mut().set_broadcast_address(&args.broadcast_address);

    let stm32_start = loader.stm32_info().flash_start() as u32 * loader.stm32_info().page_size() as u32;

    println!("Broadcasting STM32 firmware ({} bytes)...", stm32_fw.len());
    let start = Instant::now();
    loader.broadcast_flash_stm32_with_progress(stm32_start, &stm32_fw, Some(|written, total| {
        print!("\r  STM32: {}/{} ({:.0}%)", written, total, written as f64 / total as f64 * 100.0);
        io::stdout().flush().unwrap();
    })).await?;
    println!("\n  Done in {:.1}s", start.elapsed().as_secs_f64());

    if let Some(ref nrf_fw) = nrf51_fw {
        let nrf51_start = loader.nrf51_info().flash_start() as u32 * loader.nrf51_info().page_size() as u32;

        println!("Broadcasting nRF51 firmware ({} bytes)...", nrf_fw.len());
        let start = Instant::now();
        loader.broadcast_flash_nrf51_with_progress(nrf51_start, nrf_fw, Some(|written, total| {
            print!("\r  nRF51: {}/{} ({:.0}%)", written, total, written as f64 / total as f64 * 100.0);
            io::stdout().flush().unwrap();
        })).await?;
        println!("\n  Done in {:.1}s", start.elapsed().as_secs_f64());
    }

    let radio = loader.bllink_mut().radio().clone();

    // --- Phase 3: Verify & fix (unicast, per-device) ---
    println!("\n=== Phase 3: Verify & Fix ===");

    for (i, addr) in bl_addresses.iter().enumerate() {
        println!("\nVerifying device {} ({:02X?})...", i + 1, addr);

        let bllink = Bllink::new_with_radio(radio.clone(), Some(addr)).await?;
        let mut loader = CFLoader::new(bllink).await?;

        // Verify + fix STM32 (retry up to 3 times for transient errors)
        let stm32_start = loader.stm32_info().flash_start() as u32 * loader.stm32_info().page_size() as u32;
        let mut failed = loader.verify_flash_crc(bootloader::TARGET_STM32, stm32_start, &stm32_fw).await?;
        if failed.is_empty() {
            println!("  STM32: all pages OK");
        } else {
            for attempt in 1..=3 {
                println!("  STM32: {} pages failed, re-flashing (attempt {})...", failed.len(), attempt);
                loader.fix_failed_pages(bootloader::TARGET_STM32, stm32_start, &stm32_fw, &failed).await?;
                failed = loader.verify_flash_crc(bootloader::TARGET_STM32, stm32_start, &stm32_fw).await?;
                if failed.is_empty() {
                    println!("  STM32: all pages now OK");
                    break;
                }
            }
            if !failed.is_empty() {
                anyhow::bail!("Device {}: STM32 {} pages still failed after 3 attempts: {:?}", i + 1, failed.len(), failed);
            }
        }

        // Verify + fix nRF51 (retry up to 3 times for transient errors)
        if let Some(ref nrf_fw) = nrf51_fw {
            let nrf51_start = loader.nrf51_info().flash_start() as u32 * loader.nrf51_info().page_size() as u32;
            let mut failed = loader.verify_flash_crc(bootloader::TARGET_NRF51, nrf51_start, nrf_fw).await?;
            if failed.is_empty() {
                println!("  nRF51: all pages OK");
            } else {
                for attempt in 1..=3 {
                    println!("  nRF51: {} pages failed, re-flashing (attempt {})...", failed.len(), attempt);
                    loader.fix_failed_pages(bootloader::TARGET_NRF51, nrf51_start, nrf_fw, &failed).await?;
                    failed = loader.verify_flash_crc(bootloader::TARGET_NRF51, nrf51_start, nrf_fw).await?;
                    if failed.is_empty() {
                        println!("  nRF51: all pages now OK");
                        break;
                    }
                }
                if !failed.is_empty() {
                    anyhow::bail!("Device {}: nRF51 {} pages still failed after 3 attempts: {:?}", i + 1, failed.len(), failed);
                }
            }
        }

        // Reset to firmware
        loader.reset_to_firmware().await?;
        println!("  Device {} done.", i + 1);
    }

    println!("\nBroadcast flash complete for {} devices.", args.uris.len());
    Ok(())
}
