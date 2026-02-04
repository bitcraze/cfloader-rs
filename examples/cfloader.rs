use cfloader::{Bllink, CFLoader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bllink = Bllink::new(None).await.expect("Failed to create Bllink");
    let mut cfloader = CFLoader::new(bllink).await.expect("Failed to create CFLoader");
    
    // Print bootloader information
    println!("{}", cfloader.get_bootloader_summary());
    
    // Show individual bootloader info
    println!("\n--- Individual Bootloader Info ---");
    println!("nRF51 Info: {}", cfloader.nrf51_info());
    println!("STM32 Info: {}", cfloader.stm32_info());

    // Example: Read a small portion of STM32 flash for testing
    println!("\n--- Flash Read Test ---");
    let stm32_start_address = cfloader.stm32_info().flash_start() as u32 * cfloader.stm32_info().page_size() as u32;
    let read_length = 1024u32; // Read 1KB for testing

    println!("Reading {} bytes from STM32 at address 0x{:08X}",
             read_length, stm32_start_address);

    println!("\n--- Using convenience method ---");
    let read_back = cfloader.read_stm32_flash(stm32_start_address, 276216).await?;
    println!("Read {} bytes using convenience method", read_back.len());
    std::fs::write("stm32_flash_dump.bin", &read_back)?;
    println!("Read {} bytes from STM32 flash and saved to stm32_flash_dump.bin", read_back.len());

    Ok(())
}
