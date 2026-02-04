use cfloader::{Bllink, CFLoader, bootloader};
use std::time::Instant;
use anyhow::Result;
use std::env;
use std::fs;
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 3 {
        println!("Usage: {} <binary_file.bin> <target>", args[0]);
        println!("  target: 'stm32' or 'nrf51'");
        println!("Example: {} cf2-2025.02.bin stm32", args[0]);
        return Ok(());
    }
    
    let bin_file = &args[1];
    let target_name = &args[2];
    
    // Determine target
    let target = match target_name.to_lowercase().as_str() {
        "stm32" => bootloader::TARGET_STM32,
        "nrf51" | "nrf" => bootloader::TARGET_NRF51,
        _ => {
            println!("❌ Invalid target '{}'. Use 'stm32' or 'nrf51'", target_name);
            return Ok(());
        }
    };
    
    // Read binary file
    println!("=== CFLoader Flash Verification ===");
    println!("Binary file: {}", bin_file);
    println!("Target: {} (0x{:02X})\n", target_name, target);
    
    let bin_data = match fs::read(bin_file) {
        Ok(data) => {
            println!("✅ Loaded binary file: {} bytes", data.len());
            data
        }
        Err(e) => {
            println!("❌ Failed to read binary file: {}", e);
            return Ok(());
        }
    };
    
    // Initialize CFLoader
    println!("Initializing radio and bootloaders...");
    let bllink = match Bllink::new(None).await {
        Ok(bllink) => bllink,
        Err(e) => {
            println!("❌ Failed to initialize radio: {}", e);
            return Ok(());
        }
    };
    
    let mut cfloader = match CFLoader::new(bllink).await {
        Ok(cfloader) => {
            println!("✅ Connected to bootloaders");
            cfloader
        }
        Err(e) => {
            println!("❌ Failed to connect to bootloaders: {}", e);
            return Ok(());
        }
    };
    
    // Get bootloader info for the target
    let (page_size, flash_start) = match target {
        bootloader::TARGET_STM32 => {
            let info = cfloader.stm32_info();
            (info.page_size() as u32, info.flash_start() as u32)
        }
        bootloader::TARGET_NRF51 => {
            let info = cfloader.nrf51_info();
            (info.page_size() as u32, info.flash_start() as u32)
        }
        _ => unreachable!(),
    };
    
    let start_address = flash_start * page_size;
    
    println!("\nTarget Information:");
    println!("  Page size: {} bytes", page_size);
    println!("  Flash start: page {} (address 0x{:08X})", flash_start, start_address);
    println!("  Binary size: {} bytes", bin_data.len());
    
    // Start verification
    println!("\nStarting flash verification...");
    let start_time = Instant::now();
    
    match verify_flash(&mut cfloader, target, start_address, &bin_data).await {
        Ok(true) => {
            println!("\n✅ Flash verification PASSED!");
            println!("   All {} bytes match the binary file", bin_data.len());
            println!("   Verification completed in {:.2}ms", start_time.elapsed().as_millis());
        }
        Ok(false) => {
            println!("\n❌ Flash verification FAILED!");
            println!("   Verification stopped at first mismatch");
            println!("   Time elapsed: {:.2}ms", start_time.elapsed().as_millis());
        }
        Err(e) => {
            println!("\n❌ Verification error: {}", e);
        }
    }
    
    Ok(())
}

async fn verify_flash(cfloader: &mut CFLoader, target: u8, start_address: u32, bin_data: &[u8]) -> Result<bool> {
    const CHUNK_SIZE: u32 = 256; // Read in 256-byte chunks for efficiency
    let total_bytes = bin_data.len() as u32;
    let mut bytes_verified = 0u32;
    
    println!("Reading and comparing {} bytes starting at 0x{:08X}...", total_bytes, start_address);
    
    // Get target info
    let (_page_size, _target_name) = match target {
        bootloader::TARGET_STM32 => (cfloader.stm32_info().page_size(), "STM32"),
        bootloader::TARGET_NRF51 => (cfloader.nrf51_info().page_size(), "nRF51"),
        _ => return Err(anyhow::anyhow!("Invalid target")),
    };
    
    while bytes_verified < total_bytes {
        let remaining = total_bytes - bytes_verified;
        let chunk_size = remaining.min(CHUNK_SIZE);
        let current_address = start_address + bytes_verified;
        
        // Draw progress bar
        draw_progress_bar(bytes_verified as usize, total_bytes as usize, 50);
        
        // Read flash chunk
        let flash_data = match target {
            bootloader::TARGET_STM32 => {
                cfloader.read_stm32_flash(current_address, chunk_size).await?
            }
            bootloader::TARGET_NRF51 => {
                cfloader.read_nrf51_flash(current_address, chunk_size).await?
            }
            _ => return Err(anyhow::anyhow!("Invalid target")),
        };
        
        // Compare with binary data
        let bin_chunk = &bin_data[bytes_verified as usize..(bytes_verified + chunk_size) as usize];
        let compare_len = flash_data.len().min(bin_chunk.len());
        
        // Compare byte by byte
        for (i, (&flash_byte, &bin_byte)) in flash_data[..compare_len].iter().zip(bin_chunk[..compare_len].iter()).enumerate() {
            if flash_byte != bin_byte {
                println!("❌ MISMATCH at address 0x{:08X} (offset {}, chunk offset {})", 
                         current_address + i as u32, bytes_verified + i as u32, i);
                println!("   Flash: 0x{:02X}, Binary: 0x{:02X}", flash_byte, bin_byte);
                
                // Show context around the mismatch
                let context_start = i.saturating_sub(8);
                let context_end = (i + 16).min(compare_len);
                println!("   Flash context:  {:02X?}", &flash_data[context_start..context_end]);
                println!("   Binary context: {:02X?}", &bin_chunk[context_start..context_end]);
                
                
                return Ok(false);
            }
        }
        
        bytes_verified += compare_len as u32;
        
        // Draw progress bar
        draw_progress_bar(bytes_verified as usize, total_bytes as usize, 50);
        
        // Add a small delay to prevent overwhelming the bootloader
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
    
    // Complete progress bar
    draw_progress_bar(total_bytes as usize, total_bytes as usize, 50);
    
    Ok(true)
}

fn draw_progress_bar(current: usize, total: usize, width: usize) {
    let progress = (current as f64 / total as f64) * width as f64;
    let filled = progress as usize;
    let empty = width - filled;
    
    // Clear the entire line first to handle any debug output
    print!("\r\x1B[2K");
    print!("   [");
    for _ in 0..filled {
        print!("█");
    }
    for _ in 0..empty {
        print!("░");
    }
    print!("] {}/{} ({:.1}%)", current, total, (current as f64 / total as f64) * 100.0);
    
    if current == total {
        println!(); // New line when complete
    } else {
        io::stdout().flush().unwrap();
    }
}