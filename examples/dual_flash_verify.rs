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
    
    if args.len() < 2 {
        println!("Usage: {} <iterations> [stm32_file.bin] [nrf51_file.bin]", args[0]);
        println!("  iterations: Number of verification cycles to run");
        println!("  stm32_file.bin: STM32 binary file (default: cf2-2025.02.bin)");
        println!("  nrf51_file.bin: nRF51 binary file (default: cf2_nrf-2025.02.bin)");
        println!("Example: {} 10", args[0]);
        println!("Example: {} 5 my_stm32.bin my_nrf51.bin", args[0]);
        return Ok(());
    }
    
    let iterations: u32 = args[1].parse()
        .map_err(|_| anyhow::anyhow!("Invalid iterations count: '{}'", args[1]))?;
    
    let stm32_file = args.get(2).map(|s| s.as_str()).unwrap_or("cf2-2025.02.bin");
    let nrf51_file = args.get(3).map(|s| s.as_str()).unwrap_or("cf2_nrf-2025.02.bin");
    
    println!("=== CFLoader Dual Flash Verification ===");
    println!("STM32 binary: {}", stm32_file);
    println!("nRF51 binary: {}", nrf51_file);
    println!("Iterations: {}\n", iterations);
    
    // Read both binary files
    let stm32_data = match fs::read(stm32_file) {
        Ok(data) => {
            println!("✅ Loaded STM32 binary: {} bytes", data.len());
            data
        }
        Err(e) => {
            println!("❌ Failed to read STM32 binary '{}': {}", stm32_file, e);
            return Ok(());
        }
    };
    
    let nrf51_data = match fs::read(nrf51_file) {
        Ok(data) => {
            println!("✅ Loaded nRF51 binary: {} bytes", data.len());
            data
        }
        Err(e) => {
            println!("❌ Failed to read nRF51 binary '{}': {}", nrf51_file, e);
            return Ok(());
        }
    };
    
    // Initialize CFLoader
    println!("\nInitializing radio and bootloaders...");
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
    
    // Display bootloader info
    println!("\n{}", cfloader.get_bootloader_summary());
    
    // Calculate flash addresses
    let stm32_page_size = cfloader.stm32_info().page_size() as u32;
    let stm32_flash_start = cfloader.stm32_info().flash_start() as u32;
    let stm32_start_address = stm32_flash_start * stm32_page_size;
    
    let nrf51_page_size = cfloader.nrf51_info().page_size() as u32;
    let nrf51_flash_start = cfloader.nrf51_info().flash_start() as u32;
    let nrf51_start_address = nrf51_flash_start * nrf51_page_size;
    
    println!("\nFlash Configuration:");
    println!("STM32 - Page size: {} bytes, Start address: 0x{:08X}", stm32_page_size, stm32_start_address);
    println!("nRF51 - Page size: {} bytes, Start address: 0x{:08X}", nrf51_page_size, nrf51_start_address);
    
    // Run verification iterations
    let mut total_success = 0u32;
    let mut total_failures = 0u32;
    let overall_start = Instant::now();
    
    for iteration in 1..=iterations {
        println!("\n{}", "=".repeat(60));
        println!("ITERATION {}/{}", iteration, iterations);
        println!("{}", "=".repeat(60));
        
        let iteration_start = Instant::now();
        let mut iteration_success = true;
        
        // Verify STM32
        println!("\n🔍 Verifying STM32 flash...");
        match verify_flash(&mut cfloader, bootloader::TARGET_STM32, stm32_start_address, &stm32_data, "STM32").await {
            Ok(true) => {
                println!("✅ STM32 verification PASSED");
            }
            Ok(false) => {
                println!("❌ STM32 verification FAILED");
                iteration_success = false;
            }
            Err(e) => {
                println!("❌ STM32 verification ERROR: {}", e);
                iteration_success = false;
            }
        }
        
        // Small delay between targets
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        // Verify nRF51
        println!("\n🔍 Verifying nRF51 flash...");
        match verify_flash(&mut cfloader, bootloader::TARGET_NRF51, nrf51_start_address, &nrf51_data, "nRF51").await {
            Ok(true) => {
                println!("✅ nRF51 verification PASSED");
            }
            Ok(false) => {
                println!("❌ nRF51 verification FAILED");
                iteration_success = false;
            }
            Err(e) => {
                println!("❌ nRF51 verification ERROR: {}", e);
                iteration_success = false;
            }
        }
        
        // Update counters
        if iteration_success {
            total_success += 1;
        } else {
            total_failures += 1;
        }
        
        let iteration_time = iteration_start.elapsed();
        println!("\n📊 Iteration {} completed in {:.2}s - {}", 
                 iteration, iteration_time.as_secs_f64(),
                 if iteration_success { "SUCCESS" } else { "FAILED" });
        
        // Delay between iterations (except the last one)
        if iteration < iterations {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }
    
    // Final summary
    let total_time = overall_start.elapsed();
    println!("\n{}", "=".repeat(60));
    println!("FINAL RESULTS");
    println!("{}", "=".repeat(60));
    println!("Total iterations: {}", iterations);
    println!("Successful: {} ({:.1}%)", total_success, (total_success as f64 / iterations as f64) * 100.0);
    println!("Failed: {} ({:.1}%)", total_failures, (total_failures as f64 / iterations as f64) * 100.0);
    println!("Total time: {:.2}s", total_time.as_secs_f64());
    println!("Average time per iteration: {:.2}s", total_time.as_secs_f64() / iterations as f64);
    
    if total_failures == 0 {
        println!("\n🎉 ALL VERIFICATIONS PASSED!");
    } else {
        println!("\n⚠️  {} out of {} iterations failed", total_failures, iterations);
    }
    
    Ok(())
}

async fn verify_flash(cfloader: &mut CFLoader, target: u8, start_address: u32, bin_data: &[u8], target_name: &str) -> Result<bool> {
    const CHUNK_SIZE: u32 = 256; // Read in 256-byte chunks for efficiency
    let total_bytes = bin_data.len() as u32;
    let mut bytes_verified = 0u32;
    
    let start_time = Instant::now();
    
    while bytes_verified < total_bytes {
        let remaining = total_bytes - bytes_verified;
        let chunk_size = remaining.min(CHUNK_SIZE);
        let current_address = start_address + bytes_verified;
        
        // Show progress every 10%
        let progress = (bytes_verified as f64 / total_bytes as f64) * 100.0;
        if bytes_verified % (total_bytes / 10).max(1) == 0 || bytes_verified == 0 {
            print!("\r   {} progress: {:.1}%", target_name, progress);
            io::stdout().flush().unwrap();
        }
        
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
                print!("\r"); // Clear progress line
                println!("   ❌ MISMATCH at address 0x{:08X} (offset {})", 
                         current_address + i as u32, bytes_verified + i as u32);
                println!("      Flash: 0x{:02X}, Binary: 0x{:02X}", flash_byte, bin_byte);
                
                // Show context around the mismatch
                let context_start = i.saturating_sub(8);
                let context_end = (i + 16).min(compare_len);
                println!("      Flash context:  {:02X?}", &flash_data[context_start..context_end]);
                println!("      Binary context: {:02X?}", &bin_chunk[context_start..context_end]);
                
                return Ok(false);
            }
        }
        
        bytes_verified += compare_len as u32;
        
        // Small delay to prevent overwhelming the bootloader
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
    }
    
    // Complete progress line
    print!("\r   {} progress: 100.0%", target_name);
    println!(" - {} bytes verified in {:.2}ms", total_bytes, start_time.elapsed().as_millis());
    
    Ok(true)
}