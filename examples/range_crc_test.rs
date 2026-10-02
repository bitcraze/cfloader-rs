/// Range CRC test
///
/// Compares the bootloaders' CMD_RANGE_CRC against CRC32s calculated from flash
/// dumps (aligned, unaligned, large and empty ranges) and checks that ranges
/// outside of the flash are refused. Only ranges where the dump is still
/// valid are used (the bootloaders themselves may have been reflashed).
///
/// Usage:
///   cargo run --example range_crc_test -- --uri radio://0/80/2M/E7E7E7E701 \
///       --stm32-dump stm32.bin --nrf51-dump nrf51.bin

use cfloader::{Bllink, CFLoader, bootloader};
use clap::Parser;
use crazyflie_link::{LinkContext, Packet};
use std::time::{Duration, Instant};

#[derive(Parser)]
struct Args {
    /// Firmware URI to warm-boot into the bootloader from
    #[arg(long)]
    uri: Option<String>,

    /// Address of an already running bootloader (hex, radio order)
    #[arg(long)]
    address: Option<String>,

    /// Dump of the whole STM32 flash, from 0x08000000
    #[arg(long)]
    stm32_dump: String,

    /// Dump of the whole nRF51 flash, from 0
    #[arg(long)]
    nrf51_dump: String,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let stm32_dump = std::fs::read(&args.stm32_dump)?;
    let nrf51_dump = std::fs::read(&args.nrf51_dump)?;

    let address = if let Some(uri) = &args.uri {
        Some(warm_boot(uri).await?)
    } else {
        args.address.as_deref().map(parse_address).transpose()?
    };
    let bllink = Bllink::new(address.as_ref()).await?;
    let mut loader = CFLoader::new(bllink).await?;

    // (target, name, dump, address, length) with the expected CRC taken from the dump
    let checks: Vec<(u8, &str, &[u8], u32, u32)> = vec![
        (bootloader::TARGET_STM32, "STM32", &stm32_dump, 0x4000, 1024),
        (bootloader::TARGET_STM32, "STM32", &stm32_dump, 0x4001, 1),
        (bootloader::TARGET_STM32, "STM32", &stm32_dump, 0x4003, 999),
        (bootloader::TARGET_STM32, "STM32", &stm32_dump, 0x12345, 77777),
        (bootloader::TARGET_STM32, "STM32", &stm32_dump, 0x4000, 0xFC000),
        (bootloader::TARGET_STM32, "STM32", &stm32_dump, 0x80000, 0),
        (bootloader::TARGET_NRF51, "nRF51", &nrf51_dump, 0x1B000, 1024),
        (bootloader::TARGET_NRF51, "nRF51", &nrf51_dump, 0x1B001, 1),
        (bootloader::TARGET_NRF51, "nRF51", &nrf51_dump, 0x1B003, 999),
        (bootloader::TARGET_NRF51, "nRF51", &nrf51_dump, 0x1B000, 0x1F000),
        (bootloader::TARGET_NRF51, "nRF51", &nrf51_dump, 0, 0x3A000),
        (bootloader::TARGET_NRF51, "nRF51", &nrf51_dump, 0x20000, 0),
    ];

    let mut failures = 0;
    for (target, name, dump, address, length) in checks {
        let expected = crc32fast::hash(&dump[address as usize..(address + length) as usize]);
        let start = Instant::now();
        let crc = loader.range_crc(target, address, length).await?;
        let ok = crc == expected;
        failures += !ok as u32;
        println!("  {} 0x{:06X}+{:<7} crc=0x{:08X} expected=0x{:08X} {} ({:.0} ms)",
                 name, address, length, crc, expected, if ok { "OK" } else { "MISMATCH" },
                 start.elapsed().as_secs_f64() * 1000.0);
    }

    // Whole flash, for the timing (the bootloader areas differ from the dumps)
    for (target, name, size) in [(bootloader::TARGET_STM32, "STM32", 0x100000), (bootloader::TARGET_NRF51, "nRF51", 0x40000)] {
        let start = Instant::now();
        let crc = loader.range_crc(target, 0, size).await?;
        println!("  {} whole flash ({} KB) crc=0x{:08X} ({:.0} ms)", name, size / 1024, crc, start.elapsed().as_secs_f64() * 1000.0);

        // One byte past the end and an address + length that wraps must be refused
        for (address, length) in [(1, size), (size, 1), (0xFFFFFFFF, 2)] {
            match loader.range_crc(target, address, length).await {
                Err(e) => println!("  {} 0x{:08X}+{} refused: {}", name, address, length, e),
                Ok(crc) => {
                    failures += 1;
                    println!("  {} 0x{:08X}+{} NOT refused, crc=0x{:08X}", name, address, length, crc);
                }
            }
        }
    }

    loader.reset_to_firmware().await?;
    if failures == 0 {
        println!("PASS");
        Ok(())
    } else {
        anyhow::bail!("{} checks failed", failures)
    }
}
