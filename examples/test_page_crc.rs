/// Smoke test for CMD_PAGE_CRC on both bootloaders
///
/// For each page: reads flash data via read_flash, computes CRC32 locally,
/// then queries the bootloader's CMD_PAGE_CRC and compares the two. Also
/// checks that a page outside of the flash is answered with an error, and
/// that too short PAGE_CRC and SET_BROADCAST_ADDRESS requests are ignored.
///
/// Usage:
///   cargo run --example test_page_crc                      # Crazyflie already in the bootloader
///   cargo run --example test_page_crc -- --uri <uri> [--uri <uri> ...]

use cfloader::{Bllink, CFLoader, bootloader};
use clap::Parser;
use crazyflie_link::{LinkContext, Packet};
use std::time::{Duration, Instant};

const TEST_PAGES: u16 = 5;

#[derive(Parser)]
struct Args {
    /// Firmware URI to warm-boot into the bootloader from (repeatable)
    #[arg(long = "uri")]
    uris: Vec<String>,
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

async fn test_target_crc(loader: &mut CFLoader, target: u8, name: &str) -> anyhow::Result<()> {
    let (page_size, flash_start, n_flash_pages) = match target {
        bootloader::TARGET_STM32 => (
            loader.stm32_info().page_size() as u32,
            loader.stm32_info().flash_start(),
            loader.stm32_info().n_flash_page(),
        ),
        bootloader::TARGET_NRF51 => (
            loader.nrf51_info().page_size() as u32,
            loader.nrf51_info().flash_start(),
            loader.nrf51_info().n_flash_page(),
        ),
        _ => unreachable!(),
    };

    println!("\n{} PAGE_CRC test (pages {}-{}, page_size={}):",
             name, flash_start, flash_start + TEST_PAGES - 1, page_size);

    let mut pass = 0u16;
    let mut fail = 0u16;
    let bl = bootloader::Bootloader::new(target);

    for page in flash_start..flash_start + TEST_PAGES {
        // Read the full page via read_flash
        let address = page as u32 * page_size;
        let data = loader.read_flash(target, address, page_size).await?;

        // Compute CRC32 locally
        let local_crc = crc32fast::hash(&data);

        // Query bootloader CMD_PAGE_CRC
        let remote = bl.page_crc(loader.bllink_mut(), page).await?;

        let ok = local_crc == remote.crc32;
        println!("  page {:4}: local=0x{:08X}  remote=0x{:08X}  {}",
                 page, local_crc, remote.crc32, if ok { "OK" } else { "MISMATCH" });

        if ok { pass += 1; } else { fail += 1; }
    }

    // The last page is answered, the first one after the flash with an error
    // right away instead of after the retries time out
    let last = n_flash_pages - 1;
    match bl.page_crc(loader.bllink_mut(), last).await {
        Ok(packet) => {
            pass += 1;
            println!("  page {:4} (last): crc=0x{:08X}  OK", last, packet.crc32);
        }
        Err(e) => {
            fail += 1;
            println!("  page {:4} (last): {}  FAIL", last, e);
        }
    }
    for page in [n_flash_pages, 0xFFFF] {
        let start = Instant::now();
        let result = bl.page_crc(loader.bllink_mut(), page).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        match result {
            Err(e) if e.to_string().contains("outside of the flash") && ms < 100.0 => {
                pass += 1;
                println!("  page {:5}: refused in {:.0} ms: {}  OK", page, ms, e);
            }
            Err(e) => {
                fail += 1;
                println!("  page {:5}: {} after {:.0} ms  FAIL", page, e, ms);
            }
            Ok(packet) => {
                fail += 1;
                println!("  page {:5}: NOT refused, crc=0x{:08X}  FAIL", page, packet.crc32);
            }
        }
    }

    // A request with only one byte of the page must not be answered. 0x05 is
    // not the low byte of any page asked for above, so an earlier answer
    // cannot be taken for one.
    let short = [0xFF, target, 0x20, 0x05];
    match loader.bllink_mut().request(&short, Duration::from_millis(50)).await {
        Err(_) => {
            pass += 1;
            println!("  short request: not answered  OK");
        }
        Ok(answer) => {
            fail += 1;
            println!("  short request: answered {:02X?}  FAIL", answer);
        }
    }

    println!("  {} pass, {} fail", pass, fail);
    if fail > 0 {
        anyhow::bail!("{} CRC verification failed: {} checks failed", name, fail);
    }
    Ok(())
}

async fn test_short_set_broadcast_address(loader: &mut CFLoader) -> anyhow::Result<()> {
    // Only two of the five address bytes. Matched on the command only, since
    // the answer to a full request is just [0xFF, 0xFE, 0x21].
    let short = [0xFF, bootloader::TARGET_NRF51, 0x21, 0xBC, 0xE7];
    match loader.bllink_mut().request_match_response(&short, 3, Duration::from_millis(50)).await {
        Err(_) => {
            println!("\nnRF51 short SET_BROADCAST_ADDRESS: not answered  OK");
            Ok(())
        }
        Ok(answer) => anyhow::bail!("nRF51 short SET_BROADCAST_ADDRESS answered {:02X?}", answer),
    }
}

async fn test_crazyflie(address: Option<&[u8; 5]>) -> anyhow::Result<()> {
    let bllink = Bllink::new(address).await?;
    let mut loader = CFLoader::new(bllink).await?;

    println!("nRF51 version: 0x{:02X}", loader.nrf51_info().version());
    println!("STM32 version: 0x{:02X}", loader.stm32_info().version());

    // Test STM32 PAGE_CRC
    let mut result = test_target_crc(&mut loader, bootloader::TARGET_STM32, "STM32").await;

    // Test nRF51 PAGE_CRC (only if version >= 0x11)
    if loader.nrf51_info().version() >= 0x11 {
        result = result
            .and(test_target_crc(&mut loader, bootloader::TARGET_NRF51, "nRF51").await)
            .and(test_short_set_broadcast_address(&mut loader).await);
    } else {
        println!("\nnRF51 version 0x{:02X} < 0x11, skipping PAGE_CRC test",
                 loader.nrf51_info().version());
    }

    if address.is_some() {
        loader.reset_to_firmware().await?;
    }
    result
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.uris.is_empty() {
        test_crazyflie(None).await?;
        println!("\nAll tests passed.");
        return Ok(());
    }

    let mut failed = Vec::new();
    for uri in &args.uris {
        println!("=== {}", uri);
        let result = match warm_boot(uri).await {
            Ok(address) => test_crazyflie(Some(&address)).await,
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            println!("FAILED: {}", e);
            failed.push(uri.clone());
        }
        println!();
    }

    if failed.is_empty() {
        println!("All tests passed on {} Crazyflies.", args.uris.len());
        Ok(())
    } else {
        anyhow::bail!("Failed on {}", failed.join(", "))
    }
}
