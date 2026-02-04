use cfloader::{Bllink, Bootloader};
use std::time::{Duration, Instant};
use anyhow::Result;
use std::env;
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    let num_progressive_tests = if args.len() > 1 {
        args[1].parse().unwrap_or(10)
    } else {
        10
    };
    let num_stress_tests = if args.len() > 2 {
        args[2].parse().unwrap_or(50)
    } else {
        50
    };
    
    println!("=== CFLoader Connectivity Test ===");
    println!("Progressive tests: {}, Stress tests: {}\n", num_progressive_tests, num_stress_tests);
    
    // Test 1: Radio Initialization
    println!("1. Testing radio initialization...");
    let start = Instant::now();
    match Bllink::new(None).await {
        Ok(mut bllink) => {
            println!("   ✅ Radio initialized successfully ({:.2}ms)", start.elapsed().as_millis());
            
            // Test 2: Basic bootloader connectivity
            println!("\n2. Testing bootloader connectivity...");
            test_bootloader_connectivity(&mut bllink).await?;
            
            // Test 3: Progressive communication tests
            println!("\n3. Running progressive communication tests...");
            test_progressive_communication(&mut bllink, num_progressive_tests).await?;
            
            // Test 4: Stress test
            println!("\n4. Running communication stress test...");
            test_communication_stress(&mut bllink, num_stress_tests).await?;
            
        }
        Err(e) => {
            println!("   ❌ Failed to initialize radio: {}", e);
            println!("   Make sure Crazyradio PA is connected and accessible");
            return Ok(());
        }
    }
    
    println!("\n=== All tests completed successfully! ===");
    Ok(())
}

async fn test_bootloader_connectivity(bllink: &mut Bllink) -> Result<()> {
    let nrf51 = Bootloader::nrf51();
    let stm32 = Bootloader::stm32();
    
    // Test nRF51 connectivity
    println!("   Testing nRF51 bootloader...");
    let start = Instant::now();
    match nrf51.get_info(bllink).await {
        Ok(info) => {
            println!("   ✅ nRF51 connected ({:.2}ms)", start.elapsed().as_millis());
            println!("      Page size: {} bytes", info.page_size());
            println!("      Buffer pages: {}", info.n_buff_page());
            println!("      Flash pages: {}", info.n_flash_page());
            println!("      Version: 0x{:02X}", info.version());
        }
        Err(e) => {
            println!("   ❌ nRF51 connection failed: {}", e);
        }
    }
    
    // Test STM32 connectivity
    println!("   Testing STM32 bootloader...");
    let start = Instant::now();
    match stm32.get_info(bllink).await {
        Ok(info) => {
            println!("   ✅ STM32 connected ({:.2}ms)", start.elapsed().as_millis());
            println!("      Page size: {} bytes", info.page_size());
            println!("      Buffer pages: {}", info.n_buff_page());
            println!("      Flash pages: {}", info.n_flash_page());
            println!("      Version: 0x{:02X}", info.version());
        }
        Err(e) => {
            println!("   ❌ STM32 connection failed: {}", e);
        }
    }
    
    Ok(())
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

async fn test_progressive_communication(bllink: &mut Bllink, num_tests: usize) -> Result<()> {
    let stm32 = Bootloader::stm32();
    
    // Get bootloader info first
    let info = match stm32.get_info(bllink).await {
        Ok(info) => {
            println!("   ✅ Initial info request successful");
            info
        }
        Err(e) => {
            println!("   ❌ Cannot proceed with progressive tests: {}", e);
            return Ok(());
        }
    };
    
    // Test 1: Single byte read
    println!("   Testing single byte read...");
    let _start_address = info.flash_start() as u32 * info.page_size() as u32;
    let start = Instant::now();
    
    match stm32.read_flash(bllink, info.flash_start(), 0).await {
        Ok(flash_data) => {
            let data_size = flash_data.data.len();
            println!("   ✅ Single read successful: {} bytes ({:.2}ms)", 
                     data_size, start.elapsed().as_millis());
            if !flash_data.data.is_empty() {
                println!("      First bytes: {:02X?}", &flash_data.data[..data_size.min(8)]);
            }
        }
        Err(e) => {
            println!("   ❌ Single read failed: {}", e);
        }
    }
    
    // Test 2: Multiple small reads
    println!("   Testing multiple small reads ({}x)...", num_tests);
    let start = Instant::now();
    let mut success_count = 0;
    
    for i in 0..num_tests {
        draw_progress_bar(i, num_tests, 30);
        match stm32.read_flash(bllink, info.flash_start(), (i * 8) as u16).await {
            Ok(_) => success_count += 1,
            Err(_) => {}, // Silent failure for progress bar
        }
    }
    draw_progress_bar(num_tests, num_tests, 30);
    
    println!("   Results: {}/{} successful ({:.2}ms total)", 
             success_count, num_tests, start.elapsed().as_millis());
    
    // Test 3: Different page reads
    println!("   Testing reads from different pages...");
    let pages_to_test = [info.flash_start(), info.flash_start() + 1, info.flash_start() + 2];
    
    for &page in &pages_to_test {
        let start = Instant::now();
        match stm32.read_flash(bllink, page, 0).await {
            Ok(flash_data) => {
                println!("   ✅ Page {} read: {} bytes ({:.2}ms)", 
                         page, flash_data.data.len(), start.elapsed().as_millis());
            }
            Err(e) => {
                println!("   ❌ Page {} read failed: {}", page, e);
            }
        }
    }
    
    Ok(())
}

async fn test_communication_stress(bllink: &mut Bllink, num_tests: usize) -> Result<()> {
    let stm32 = Bootloader::stm32();
    
    // Get bootloader info
    let _info = match stm32.get_info(bllink).await {
        Ok(info) => info,
        Err(e) => {
            println!("   ❌ Cannot run stress test: {}", e);
            return Ok(());
        }
    };
    
    println!("   Running {} consecutive get_info requests...", num_tests);
    let start = Instant::now();
    let mut success_count = 0;
    let mut total_time = Duration::ZERO;
    
    for i in 0..num_tests {
        draw_progress_bar(i, num_tests, 40);
        let req_start = Instant::now();
        match stm32.get_info(bllink).await {
            Ok(_) => {
                success_count += 1;
                let req_time = req_start.elapsed();
                total_time += req_time;
            }
            Err(_) => {
                // Silent failure for progress bar
            }
        }
    }
    draw_progress_bar(num_tests, num_tests, 40);
    
    let total_elapsed = start.elapsed();
    println!("   Results: {}/{} successful", success_count, num_tests);
    println!("   Total time: {:.2}ms", total_elapsed.as_millis());
    println!("   Average per request: {:.1}ms", 
             total_elapsed.as_millis() as f64 / num_tests as f64);
    
    let success_rate = success_count as f64 / num_tests as f64;
    if success_rate >= 0.9 {
        println!("   ✅ Communication appears stable ({:.1}% success)", success_rate * 100.0);
    } else if success_rate >= 0.6 {
        println!("   ⚠️  Communication has some issues but is partially working ({:.1}% success)", success_rate * 100.0);
    } else {
        println!("   ❌ Communication is unstable ({:.1}% success)", success_rate * 100.0);
    }
    
    Ok(())
}