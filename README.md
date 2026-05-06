# cfloader

Rust crate for flashing firmware to Crazyflie 2.x quadcopters over Crazyradio.

Handles the full flash sequence: boot mode entry, STM32/nRF51 firmware,
softdevice management, and expansion deck firmware updates.

## Supported platforms

- Crazyflie 2.0
- Crazyflie 2.1
- Crazyflie Bolt
- Crazyflie Brushless 2.1

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
cfloader = "0.2"
```

This crate uses `async` and requires Tokio.

## Example

### Flash a firmware zip

```rust
use cfloader::firmware;
use cfloader::flasher::{self, FlashConfig};
use cfloader::boot_entry::BootMode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let zip_data = std::fs::read("firmware-cf2-2024.01.zip")?;
    let (_info, images) = firmware::parse_firmware_zip(&zip_data)?;

    let link_context = crazyflie_link::LinkContext::new();
    flasher::flash(&link_context, FlashConfig {
        boot_mode: BootMode::Warm { uri: "radio://0/80/2M/E7E7E7E7E7".into() },
        uri: Some("radio://0/80/2M/E7E7E7E7E7".into()),
        images,
        progress: None,
        toc_cache: crazyflie_lib::NoTocCache,
    }).await?;

    Ok(())
}
```

### Flash a single binary

```rust
use cfloader::firmware::{self, FlashTarget};
use cfloader::flasher::{self, FlashConfig};
use cfloader::boot_entry::BootMode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bin = std::fs::read("cf2.bin")?;
    let image = firmware::firmware_from_binary(
        bin,
        FlashTarget::Stm32 { start_override: None },
        "cf2.bin".into(),
    );

    let link_context = crazyflie_link::LinkContext::new();
    flasher::flash(&link_context, FlashConfig {
        boot_mode: BootMode::Warm { uri: "radio://0/80/2M/E7E7E7E7E7".into() },
        uri: None,
        images: vec![image],
        progress: None,
        toc_cache: crazyflie_lib::NoTocCache,
    }).await?;

    Ok(())
}
```

### Low-level bootloader access

The low-level API is still available for direct bootloader communication:

```rust
use cfloader::{Bllink, CFLoader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bllink = Bllink::new(None).await?;
    let mut loader = CFLoader::new(bllink).await?;

    let firmware = std::fs::read("firmware.bin")?;
    loader.flash_stm32(0x8000, &firmware).await?;
    loader.reset_to_firmware().await?;
    Ok(())
}
```

### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
</sub>
