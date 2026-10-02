/// Broadcast reception test
///
/// Checks that LOAD_BUFFER packets sent on the broadcast address reach the
/// bootloader of both the nRF51 and the STM32, without touching flash:
///   1. Fill buffer page 0 with a pattern over unicast and read it back
///   2. Arm broadcast reception with SET_BROADCAST_ADDRESS
///   3. Broadcast a different pattern into buffer page 0
///   4. Read the buffer back over unicast and count how much arrived
///
/// Usage:
///   cargo run --example broadcast_rx_test -- [--uri radio://0/80/2M/E7E7E7E701] [--address B1xxxxxxxx]
///
/// With --uri the Crazyflie is warm-booted from its firmware into the bootloader.
/// With --address an already running bootloader is used. Without either the
/// cold-boot default address E7E7E7E7E7 is used.

use cfloader::{Bllink, CFLoader, bootloader, BROADCAST_ADDRESS};
use clap::Parser;
use crazyflie_link::{LinkContext, Packet};
use std::time::Duration;

#[derive(Parser)]
struct Args {
    /// Firmware URI to warm-boot into the bootloader from
    #[arg(long)]
    uri: Option<String>,

    /// Address of an already running bootloader (hex, radio order)
    #[arg(long)]
    address: Option<String>,

    /// Broadcast address to arm (hex, radio order)
    #[arg(long)]
    broadcast: Option<String>,

    /// Number of bytes to broadcast to the nRF51 (one 1 KB buffer page)
    #[arg(long, default_value_t = 1024)]
    bytes: usize,

    /// Number of bytes to broadcast to the STM32 (up to its 10 KB buffer)
    #[arg(long, default_value_t = 10240)]
    stm32_bytes: usize,

    /// Times each broadcast packet is sent
    #[arg(long, default_value_t = 1)]
    repeat: usize,

    /// Delay between broadcast packets in ms
    #[arg(long, default_value_t = 1)]
    delay_ms: u64,

    /// Stay in the bootloader afterwards (no reset to firmware)
    #[arg(long)]
    stay: bool,

    /// Send the no-ack packets to this address instead of the armed broadcast address
    #[arg(long)]
    send_to: Option<String>,

    /// Move the bootloader to this unicast address (hex, radio order) and reconnect there
    #[arg(long)]
    set_address: Option<String>,

    /// Stop early: 0 = after entering the bootloader, 1 = after arming broadcast,
    /// 2 = after only broadcasting (no readback, so the radio's RXMATCH shows the last broadcast)
    #[arg(long)]
    steps: Option<u8>,
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

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let address = 'found: loop {
        if std::time::Instant::now() > deadline {
            anyhow::bail!("no RESET_INIT response from the nRF51 firmware");
        }
        link.send_packet(vec![0xFF, bootloader::TARGET_NRF51, 0xFF].into()).await?;
        let wait_until = std::time::Instant::now() + Duration::from_millis(200);
        while let Ok(packet) = tokio::time::timeout_at(wait_until.into(), link.recv_packet()).await {
            let packet: Packet = packet?;
            let data = packet.get_data();
            if std::env::var("WB_DEBUG").is_ok() {
                eprintln!("rx header={:02X} data={:02X?}", packet.get_header(), data);
            }
            if data.len() > 6 && data[0..2] == [bootloader::TARGET_NRF51, 0xFF] {
                break 'found [0xB1, data[5], data[4], data[3], data[2]];
            }
        }
    };

    for _ in 0..10 {
        link.send_packet(vec![0xFF, bootloader::TARGET_NRF51, 0xF0, 0x00].into()).await?;
    }
    // Let the queued reset packets go out before closing the link
    tokio::time::sleep(Duration::from_millis(500)).await;
    link.close().await;
    drop(context);
    tokio::time::sleep(Duration::from_millis(1000)).await;

    Ok(address)
}

async fn read_buffer_page(loader: &mut CFLoader, target: u8, len: usize) -> anyhow::Result<Vec<u8>> {
    let bl = bootloader::Bootloader::new(target);
    let mut data = Vec::new();
    while data.len() < len {
        let packet = bl.read_buffer(loader.bllink_mut(), 0, data.len() as u16).await?;
        data.extend_from_slice(&packet.data);
    }
    data.truncate(len);
    Ok(data)
}

async fn test_target(loader: &mut CFLoader, target: u8, name: &str, args: &Args, len: usize) -> anyhow::Result<bool> {
    let bl = bootloader::Bootloader::new(target);

    // Baseline over unicast so stale buffer contents can't fake a pass
    let baseline: Vec<u8> = (0..len).map(|i| (i as u8) ^ 0x55).collect();
    for (i, chunk) in baseline.chunks(25).enumerate() {
        bl.load_buffer(loader.bllink_mut(), 0, (i * 25) as u16, chunk).await?;
    }
    let readback = read_buffer_page(loader, target, len).await?;
    if readback != baseline {
        println!("  {}: unicast baseline readback FAILED", name);
        return Ok(false);
    }

    // Broadcast a different pattern
    let pattern: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(7).wrapping_add(0x21)).collect();
    for (i, chunk) in pattern.chunks(25).enumerate() {
        for _ in 0..args.repeat {
            bl.broadcast_load_buffer(loader.bllink_mut(), 0, (i * 25) as u16, chunk).await?;
            tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;
        }
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let readback = read_buffer_page(loader, target, len).await?;
    let chunks = pattern.len().div_ceil(25);
    let mut received = 0;
    for i in 0..chunks {
        let end = ((i + 1) * 25).min(len);
        if readback[i * 25..end] == pattern[i * 25..end] {
            received += 1;
        }
    }
    println!("  {}: {}/{} broadcast packets landed in the buffer", name, received, chunks);
    Ok(received == chunks)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let address = if let Some(uri) = &args.uri {
        let address = warm_boot(uri).await?;
        println!("Warm-booted into bootloader at {:02X?}", address);
        Some(address)
    } else {
        args.address.as_deref().map(parse_address).transpose()?
    };
    let broadcast = args.broadcast.as_deref().map(parse_address).transpose()?.unwrap_or(BROADCAST_ADDRESS);

    let bllink = Bllink::new(address.as_ref()).await?;
    let mut loader = CFLoader::new(bllink).await?;
    println!("nRF51 protocol 0x{:02X}, STM32 protocol 0x{:02X}",
             loader.nrf51_info().version(), loader.stm32_info().version());

    if args.steps == Some(0) {
        return Ok(());
    }

    if let Some(new_address) = args.set_address.as_deref().map(parse_address).transpose()? {
        bootloader::Bootloader::nrf51().set_address(loader.bllink_mut(), &new_address).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let radio = loader.bllink_mut().radio().clone();
        let bllink = Bllink::new_with_radio(radio, Some(&new_address)).await?;
        loader = CFLoader::new(bllink).await?;
        println!("Reconnected at {:02X?}: nRF51 protocol 0x{:02X}", new_address, loader.nrf51_info().version());
    }

    loader.set_broadcast_address(&broadcast).await?;
    println!("Broadcast armed on {:02X?}", broadcast);
    tokio::time::sleep(Duration::from_millis(50)).await;

    if args.steps == Some(1) {
        return Ok(());
    }

    // Send to another address than the armed one, if asked to
    if let Some(send_to) = args.send_to.as_deref().map(parse_address).transpose()? {
        loader.bllink_mut().set_broadcast_address(&send_to);
    }
    println!("Broadcasting to {:02X?}", loader.bllink_mut().broadcast_address());

    if args.steps == Some(2) {
        let bl = bootloader::Bootloader::nrf51();
        for i in 0..args.bytes.div_ceil(25) {
            bl.broadcast_load_buffer(loader.bllink_mut(), 0, (i * 25) as u16, &[0xA5; 25]).await?;
            tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;
        }
        return Ok(());
    }

    let nrf_ok = test_target(&mut loader, bootloader::TARGET_NRF51, "nRF51", &args, args.bytes).await?;
    let stm_ok = test_target(&mut loader, bootloader::TARGET_STM32, "STM32", &args, args.stm32_bytes).await?;

    if !args.stay {
        loader.reset_to_firmware().await?;
    }

    if nrf_ok && stm_ok {
        println!("PASS");
        Ok(())
    } else {
        anyhow::bail!("broadcast data missing")
    }
}
