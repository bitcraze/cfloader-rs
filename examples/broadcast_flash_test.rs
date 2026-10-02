/// End-to-end broadcast flash test on one Crazyflie
///
/// Warm-boots the Crazyflie into the bootloader, checks the current flash
/// against the images with PAGE_CRC, broadcasts the images, verifies with
/// PAGE_CRC, fixes the failed pages over unicast and resets to firmware.
///
/// Usage:
///   cargo run --example broadcast_flash_test -- --uri radio://0/80/2M/E7E7E7E701 \
///       --stm32-fw cf2.bin [--nrf51-fw cf2_nrf.bin]

use cfloader::{Bllink, CFLoader, bootloader};
use clap::Parser;
use crazyflie_link::{LinkContext, Packet};
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(Parser)]
struct Args {
    /// Firmware URI to warm-boot into the bootloader from
    #[arg(long)]
    uri: Option<String>,

    /// Address of an already running bootloader (hex, radio order)
    #[arg(long)]
    address: Option<String>,

    /// STM32 firmware binary
    #[arg(long)]
    stm32_fw: Option<String>,

    /// nRF51 firmware binary
    #[arg(long)]
    nrf51_fw: Option<String>,

    /// Only check the current flash against the images, don't flash
    #[arg(long)]
    check_only: bool,

    /// Broadcast address (hex, same byte order as radio URIs)
    #[arg(long, default_value = "BCE7E7E7E7")]
    broadcast_address: String,

    /// Pause between broadcast packets in microseconds
    #[arg(long, default_value_t = 0)]
    packet_delay_us: u64,

    /// Stay in the bootloader afterwards (no reset to firmware)
    #[arg(long)]
    stay: bool,

    /// Don't fix failed pages after the broadcast (to see the raw broadcast result)
    #[arg(long)]
    no_fix: bool,
}

fn parse_address(s: &str) -> anyhow::Result<[u8; 5]> {
    let bytes = (0..10)
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()?;
    Ok(bytes.try_into().unwrap())
}

async fn warm_boot(uri: &str) -> anyhow::Result<[u8; 5]> {
    let context = LinkContext::new();
    let separator = if uri.contains('?') { "&" } else { "?" };
    let link = context.open_link(&format!("{}{}safelink=0", uri, separator)).await?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let address = 'found: loop {
        if Instant::now() > deadline {
            anyhow::bail!("no RESET_INIT response from the nRF51 firmware");
        }
        link.send_packet(vec![0xFF, bootloader::TARGET_NRF51, 0xFF].into()).await?;
        let wait_until = Instant::now() + Duration::from_millis(200);
        while let Ok(packet) = tokio::time::timeout_at(wait_until.into(), link.recv_packet()).await {
            let packet: Packet = packet?;
            let data = packet.get_data();
            if data.len() > 6 && data[0..2] == [bootloader::TARGET_NRF51, 0xFF] {
                break 'found [0xB1, data[5], data[4], data[3], data[2]];
            }
        }
    };

    for _ in 0..10 {
        link.send_packet(vec![0xFF, bootloader::TARGET_NRF51, 0xF0, 0x00].into()).await?;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    link.close().await;
    drop(context);
    tokio::time::sleep(Duration::from_millis(1000)).await;

    Ok(address)
}

fn start_address(loader: &CFLoader, target: u8) -> u32 {
    let info = if target == bootloader::TARGET_STM32 { loader.stm32_info() } else { loader.nrf51_info() };
    info.flash_start() as u32 * info.page_size() as u32
}

async fn verify(loader: &mut CFLoader, target: u8, name: &str, image: &[u8]) -> anyhow::Result<Vec<u16>> {
    let start = Instant::now();
    let failed = loader.verify_flash_crc(target, start_address(loader, target), image).await?;
    println!("  {}: {} failed pages {:?} ({:.1}s)", name, failed.len(), failed, start.elapsed().as_secs_f64());
    Ok(failed)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut images = Vec::new();
    if let Some(path) = &args.stm32_fw {
        images.push((bootloader::TARGET_STM32, "STM32", std::fs::read(path)?));
    }
    if let Some(path) = &args.nrf51_fw {
        images.push((bootloader::TARGET_NRF51, "nRF51", std::fs::read(path)?));
    }

    let address = if let Some(uri) = &args.uri {
        let address = warm_boot(uri).await?;
        println!("Warm-booted into bootloader at {:02X?}", address);
        Some(address)
    } else {
        args.address.as_deref().map(parse_address).transpose()?
    };

    let bllink = Bllink::new(address.as_ref()).await?;
    let mut loader = CFLoader::new(bllink).await?;
    println!("nRF51 protocol 0x{:02X}, STM32 protocol 0x{:02X}",
             loader.nrf51_info().version(), loader.stm32_info().version());

    println!("\nCurrent flash vs images:");
    for (target, name, image) in &images {
        verify(&mut loader, *target, name, image).await?;
    }
    if args.check_only {
        return Ok(());
    }

    loader.set_broadcast_packet_delay(Duration::from_micros(args.packet_delay_us));
    loader.set_broadcast_address(&parse_address(&args.broadcast_address)?).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;

    println!("\nBroadcast:");
    for (target, name, image) in &images {
        let start = Instant::now();
        let base = start_address(&loader, *target);
        loader.broadcast_flash_image_with_progress(*target, base, image, Some(|written: usize, total: usize| {
            print!("\r  {}: {}/{}", name, written, total);
            std::io::stdout().flush().unwrap();
        })).await?;
        println!("  ({:.1}s)", start.elapsed().as_secs_f64());
    }

    println!("\nVerify after broadcast:");
    let mut all_ok = true;
    for (target, name, image) in &images {
        let failed = verify(&mut loader, *target, name, image).await?;
        if !failed.is_empty() && !args.no_fix {
            let start = Instant::now();
            loader.fix_failed_pages(*target, start_address(&loader, *target), image, &failed).await?;
            println!("  {}: fixed in {:.1}s", name, start.elapsed().as_secs_f64());
            let failed = verify(&mut loader, *target, name, image).await?;
            all_ok &= failed.is_empty();
        } else {
            all_ok &= failed.is_empty();
        }
    }

    if !args.stay {
        loader.reset_to_firmware().await?;
    }
    if all_ok {
        println!("\nPASS");
        Ok(())
    } else {
        anyhow::bail!("pages still differ")
    }
}
