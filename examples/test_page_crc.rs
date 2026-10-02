/// Smoke test for CMD_PAGE_CRC on both bootloaders
///
/// For each page: reads flash data via read_flash, computes CRC32 locally,
/// then queries the bootloader's CMD_PAGE_CRC and compares the two.
///
/// Usage: cargo run --example test_page_crc

use cfloader::{Bllink, CFLoader, bootloader};

const TEST_PAGES: u16 = 5;

async fn test_target_crc(loader: &mut CFLoader, target: u8, name: &str) -> anyhow::Result<()> {
    let (page_size, flash_start) = match target {
        bootloader::TARGET_STM32 => (
            loader.stm32_info().page_size() as u32,
            loader.stm32_info().flash_start(),
        ),
        bootloader::TARGET_NRF51 => (
            loader.nrf51_info().page_size() as u32,
            loader.nrf51_info().flash_start(),
        ),
        _ => unreachable!(),
    };

    println!("\n{} PAGE_CRC test (pages {}-{}, page_size={}):",
             name, flash_start, flash_start + TEST_PAGES - 1, page_size);

    let mut pass = 0u16;
    let mut fail = 0u16;

    for page in flash_start..flash_start + TEST_PAGES {
        // Read the full page via read_flash
        let address = page as u32 * page_size;
        let data = loader.read_flash(target, address, page_size).await?;

        // Compute CRC32 locally
        let local_crc = crc32fast::hash(&data);

        // Query bootloader CMD_PAGE_CRC
        let bl = bootloader::Bootloader::new(target);
        let remote = bl.page_crc(loader.bllink_mut(), page).await?;

        let ok = local_crc == remote.crc32;
        println!("  page {:4}: local=0x{:08X}  remote=0x{:08X}  {}",
                 page, local_crc, remote.crc32, if ok { "OK" } else { "MISMATCH" });

        if ok { pass += 1; } else { fail += 1; }
    }

    println!("  {} pass, {} fail", pass, fail);
    if fail > 0 {
        anyhow::bail!("{} CRC verification failed: {} pages mismatched", name, fail);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bllink = Bllink::new(None).await?;
    let mut loader = CFLoader::new(bllink).await?;

    println!("nRF51 version: 0x{:02X}", loader.nrf51_info().version());
    println!("STM32 version: 0x{:02X}", loader.stm32_info().version());

    // Test STM32 PAGE_CRC
    test_target_crc(&mut loader, bootloader::TARGET_STM32, "STM32").await?;

    // Test nRF51 PAGE_CRC (only if version >= 0x11)
    if loader.nrf51_info().version() >= 0x11 {
        test_target_crc(&mut loader, bootloader::TARGET_NRF51, "nRF51").await?;
    } else {
        println!("\nnRF51 version 0x{:02X} < 0x11, skipping PAGE_CRC test",
                 loader.nrf51_info().version());
    }

    println!("\nAll tests passed.");
    Ok(())
}
