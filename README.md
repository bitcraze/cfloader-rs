# cfloader-rs

Rust library for interfacing with the Crazyflie 2.x bootloader over Crazyradio.

## Supported platforms

- Crazyflie 2.0
- Crazyflie 2.1
- Crazyflie Bolt
- Crazyflie Brushless 2.1

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
cfloader = "0.1"
```

This crate is using `async` and requires `Tokio`.

## Example

```rust
use cfloader::{Bllink, CFLoader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bllink = Bllink::new(None).await?;
    let mut loader = CFLoader::new(bllink).await?;

    // Flash firmware to STM32
    let firmware = std::fs::read("firmware.bin")?;
    loader.flash_stm32(0x8000, &firmware).await?;

    // Reset to normal operation
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