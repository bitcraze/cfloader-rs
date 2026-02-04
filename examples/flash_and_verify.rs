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
        println!("Usage: {} <binary_file.bin> <target> [iterations]", args[0]);
        println!("  target: 'stm32' or 'nrf51'");
        println!("  iterations: Number of flash+verify cycles (default: 1)");
        println!("Example: {} cf2-2025.02.bin stm32", args[0]);
        println!("Example: {} cf2_nrf-2025.02.bin nrf51 3", args[0]);
        return Ok(());
    }
    
    let bin_file = &args[1];
    let target_name = &args[2];
    let iterations: u32 = args.get(3)
        .map(|s| s.parse().unwrap_or(1))
        .unwrap_or(1);
    
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
    println!("=== CFLoader Flash and Verify Test ===");
    println!("Binary file: {}", bin_file);
    println!("Target: {} (0x{:02X})", target_name, target);
    println!("Iterations: {}\n", iterations);
    
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
    
    // Run flash and verify iterations
    let mut total_success = 0u32;
    let mut total_failures = 0u32;
    let overall_start = Instant::now();
    
    for iteration in 1..=iterations {
        println!("\n{}", "=".repeat(60));
        println!("ITERATION {}/{}", iteration, iterations);
        println!("{}", "=".repeat(60));
        
        let iteration_start = Instant::now();
        let mut iteration_success = true;
        
        // Flash the binary
        println!("\n🔥 Flashing {} binary ({} bytes at 0x{:08X})...", target_name, bin_data.len(), start_address);
        println!("   📋 Flash parameters: page_size={}, flash_start_page={}", page_size, flash_start);
        let flash_start_time = Instant::now();
        
        let flash_result = match target {
            bootloader::TARGET_STM32 => {
                println!("   🎯 Targeting STM32 bootloader (0x{:02X})", bootloader::TARGET_STM32);
                cfloader.flash_stm32(start_address, &bin_data).await
            }
            bootloader::TARGET_NRF51 => {
                println!("   🎯 Targeting nRF51 bootloader (0x{:02X})", bootloader::TARGET_NRF51);
                cfloader.flash_nrf51(start_address, &bin_data).await
            }
            _ => unreachable!(),
        };
        
        match flash_result {
            Ok(()) => {
                let flash_time = flash_start_time.elapsed();
                println!("✅ Flash operation completed in {:.2}s ({:.1} KB/s)", 
                         flash_time.as_secs_f64(),
                         (bin_data.len() as f64 / 1024.0) / flash_time.as_secs_f64());
            }
            Err(e) => {
                println!("❌ Flash operation failed after {:.2}s: {}", flash_start_time.elapsed().as_secs_f64(), e);
                iteration_success = false;
            }
        }
        
        // Only verify if flash succeeded
        if iteration_success {
            // Small delay after flashing
            println!("   ⏳ Waiting 200ms for flash to settle...");
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            
            // Verify the flash
            println!("\n🔍 Verifying {} flash ({} bytes from 0x{:08X})...", target_name, bin_data.len(), start_address);
            let verify_start_time = Instant::now();
            
            match verify_flash(&mut cfloader, target, start_address, &bin_data, target_name).await {
                Ok(true) => {
                    let verify_time = verify_start_time.elapsed();
                    println!("✅ Verification PASSED in {:.2}s ({:.1} KB/s)", 
                             verify_time.as_secs_f64(),
                             (bin_data.len() as f64 / 1024.0) / verify_time.as_secs_f64());
                }
                Ok(false) => {
                    println!("❌ Verification FAILED after {:.2}s", verify_start_time.elapsed().as_secs_f64());
                    iteration_success = false;
                }
                Err(e) => {
                    println!("❌ Verification ERROR after {:.2}s: {}", verify_start_time.elapsed().as_secs_f64(), e);
                    iteration_success = false;
                }
            }
        } else {
            println!("   ⏭️  Skipping verification due to flash failure");
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
            println!("⏳ Waiting before next iteration...");
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
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
        println!("\n🎉 ALL FLASH AND VERIFY CYCLES PASSED!");
    } else {
        println!("\n⚠️  {} out of {} cycles failed", total_failures, iterations);
    }
    
    Ok(())
}

async fn verify_flash(cfloader: &mut CFLoader, target: u8, start_address: u32, bin_data: &[u8], target_name: &str) -> Result<bool> {
    const CHUNK_SIZE: u32 = 256; // Read in 256-byte chunks for efficiency
    let total_bytes = bin_data.len() as u32;
    let mut bytes_verified = 0u32;
    let mut read_operations = 0u32;
    
    let start_time = Instant::now();
    println!("   📖 Starting verification with {} byte chunks...", CHUNK_SIZE);
    
    while bytes_verified < total_bytes {
        let remaining = total_bytes - bytes_verified;
        let chunk_size = remaining.min(CHUNK_SIZE);
        let current_address = start_address + bytes_verified;
        
        // Show progress every 5%
        let progress = (bytes_verified as f64 / total_bytes as f64) * 100.0;
        if bytes_verified % (total_bytes / 20).max(1) == 0 || bytes_verified == 0 {
            print!("\r   {} verification: {:.1}% ({}/{} bytes, {} ops)", 
                   target_name, progress, bytes_verified, total_bytes, read_operations);
            io::stdout().flush().unwrap();
        }
        
        // Read flash chunk with timing
        let read_start = Instant::now();
        let flash_data = match target {
            bootloader::TARGET_STM32 => {
                cfloader.read_stm32_flash(current_address, chunk_size).await?
            }
            bootloader::TARGET_NRF51 => {
                cfloader.read_nrf51_flash(current_address, chunk_size).await?
            }
            _ => return Err(anyhow::anyhow!("Invalid target")),
        };
        read_operations += 1;
        
        // Validate read result
        if flash_data.len() != chunk_size as usize {
            print!("\r"); // Clear progress line
            println!("   ⚠️  Read size mismatch at 0x{:08X}: expected {} bytes, got {}", 
                     current_address, chunk_size, flash_data.len());
        }
        
        // Log slow reads
        let read_time = read_start.elapsed();
        if read_time.as_millis() > 50 {
            print!("\r"); // Clear progress line  
            println!("   🐌 Slow read: {}ms for {} bytes at 0x{:08X}", 
                     read_time.as_millis(), flash_data.len(), current_address);
        }
        
        // Compare with binary data
        let bin_chunk = &bin_data[bytes_verified as usize..(bytes_verified + chunk_size) as usize];
        let compare_len = flash_data.len().min(bin_chunk.len());
        
        // Compare byte by byte
        for (i, (&flash_byte, &bin_byte)) in flash_data[..compare_len].iter().zip(bin_chunk[..compare_len].iter()).enumerate() {
            if flash_byte != bin_byte {
                print!("\r"); // Clear progress line
                println!("   ❌ MISMATCH at address 0x{:08X} (offset {}, operation #{})", 
                         current_address + i as u32, bytes_verified + i as u32, read_operations);
                println!("      Flash: 0x{:02X}, Binary: 0x{:02X}", flash_byte, bin_byte);
                
                // Show context around the mismatch
                let context_start = i.saturating_sub(8);
                let context_end = (i + 16).min(compare_len);
                println!("      Flash context:  {:02X?}", &flash_data[context_start..context_end]);
                println!("      Binary context: {:02X?}", &bin_chunk[context_start..context_end]);
                
                // Show timing info
                println!("      Read timing: {:.1}ms for this chunk", read_time.as_millis());
                println!("      Verification progress: {:.1}% complete", progress);
                
                return Ok(false);
            }
        }
        
        bytes_verified += compare_len as u32;
        
        // Small delay to prevent overwhelming the bootloader
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
    }
    
    // Complete progress line
    let total_time = start_time.elapsed();
    print!("\r   {} verification: 100.0%", target_name);
    println!(" - {} bytes verified in {:.2}s ({} ops, avg {:.1}ms/op)", 
             total_bytes, total_time.as_secs_f64(), read_operations,
             total_time.as_millis() as f64 / read_operations as f64);
    
    Ok(true)
}