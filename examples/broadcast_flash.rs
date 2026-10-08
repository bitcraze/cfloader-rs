/// Broadcast firmware upgrade example
///
/// Demonstrates flashing firmware to multiple Crazyflies simultaneously using
/// broadcast, then verifying and fixing each device individually via CRC.
///
/// Usage:
///   cargo run --example broadcast_flash -- --stm32-fw <path> [--nrf51-fw <path>] --count <N>

use cfloader::{Bllink, CFLoader, bootloader, BROADCAST_ADDRESS};
use std::time::Instant;
use anyhow::Result;
use clap::Parser;
use std::io::{self, Write};

#[derive(Parser)]
#[command(about = "Broadcast firmware to multiple Crazyflies")]
struct Args {
    /// STM32 firmware binary file
    #[arg(long)]
    stm32_fw: String,

    /// nRF51 firmware binary file (optional)
    #[arg(long)]
    nrf51_fw: Option<String>,

    /// Number of Crazyflies to flash
    #[arg(long)]
    count: usize,
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

    // --- Connectivity check ---
    println!("\n=== Connectivity Check ===");
    println!("Connecting to bootloader on default address...");
    let bllink = Bllink::new(None).await?;
    let loader = CFLoader::new(bllink).await?;
    println!("  nRF51 version: 0x{:02X}", loader.nrf51_info().version());
    println!("  STM32 version: 0x{:02X}", loader.stm32_info().version());
    if loader.nrf51_info().version() < 0x11 {
        anyhow::bail!("nRF51 bootloader version 0x{:02X} does not support broadcast. Need >= 0x11.",
                       loader.nrf51_info().version());
    }
    println!("  Bootloader OK.");
    drop(loader);

    // --- Phase 1: Setup (enable broadcast on each device) ---
    println!("\n=== Phase 1: Setup ===");
    println!("Discovering and configuring {} devices...\n", args.count);

    for i in 0..args.count {
        println!("Waiting for device {} on default address...", i + 1);

        let bllink = Bllink::new(None).await?;
        let mut loader = CFLoader::new(bllink).await?;

        // Enable broadcast reception on this device
        loader.set_broadcast_address(&BROADCAST_ADDRESS).await?;
        println!("  Broadcast armed on {:02X?}", BROADCAST_ADDRESS);

        // The loader/bllink is dropped here; the device keeps its broadcast config
        drop(loader);
    }

    println!("\nAll {} devices ready.", args.count);

    // --- Phase 2: Broadcast firmware (all devices simultaneously) ---
    println!("\n=== Phase 2: Broadcast ===");

    // Connect to the device on default address to get bootloader info (page sizes etc),
    // then use broadcast for the actual data transfer.
    let bllink = Bllink::new(None).await?;
    let mut loader = CFLoader::new(bllink).await?;

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

    // --- Phase 3: Verify & fix (unicast, per-device) ---
    println!("\n=== Phase 3: Verify & Fix ===");

    let radio = loader.bllink_mut().radio().clone();

    for i in 0..args.count {
        println!("\nVerifying device {}...", i + 1);

        let bllink = Bllink::new_with_radio(radio.clone(), None).await?;
        let mut loader = CFLoader::new(bllink).await?;

        // Verify + fix STM32
        let stm32_start = loader.stm32_info().flash_start() as u32 * loader.stm32_info().page_size() as u32;
        let failed = loader.verify_flash_crc(bootloader::TARGET_STM32, stm32_start, &stm32_fw).await?;
        if failed.is_empty() {
            println!("  STM32: all pages OK");
        } else {
            println!("  STM32: {} pages failed, re-flashing...", failed.len());
            loader.fix_failed_pages(bootloader::TARGET_STM32, stm32_start, &stm32_fw, &failed).await?;

            // Re-verify
            let still_failed = loader.verify_flash_crc(bootloader::TARGET_STM32, stm32_start, &stm32_fw).await?;
            if still_failed.is_empty() {
                println!("  STM32: fixed, all pages now OK");
            } else {
                println!("  STM32: ERROR - {} pages still failed after fix: {:?}", still_failed.len(), still_failed);
            }
        }

        // Verify + fix nRF51
        if let Some(ref nrf_fw) = nrf51_fw {
            let nrf51_start = loader.nrf51_info().flash_start() as u32 * loader.nrf51_info().page_size() as u32;
            let failed = loader.verify_flash_crc(bootloader::TARGET_NRF51, nrf51_start, nrf_fw).await?;
            if failed.is_empty() {
                println!("  nRF51: all pages OK");
            } else {
                println!("  nRF51: {} pages failed, re-flashing...", failed.len());
                loader.fix_failed_pages(bootloader::TARGET_NRF51, nrf51_start, nrf_fw, &failed).await?;

                let still_failed = loader.verify_flash_crc(bootloader::TARGET_NRF51, nrf51_start, nrf_fw).await?;
                if still_failed.is_empty() {
                    println!("  nRF51: fixed, all pages now OK");
                } else {
                    println!("  nRF51: ERROR - {} pages still failed after fix: {:?}", still_failed.len(), still_failed);
                }
            }
        }

        // Reset to firmware
        loader.reset_to_firmware().await?;
        println!("  Device {} done.", i + 1);
    }

    println!("\nBroadcast flash complete for {} devices.", args.count);
    Ok(())
}
