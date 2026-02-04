//! # Crate to interface with the Crazyflie bootloader
//! 
//! This crate provides functionality to communicate with the Crazyflie bootloaders
//! over the Crazyradio. It supports flashing and reading firmware in both the
//! STM32 and nRF51 chip of a Crazyflie 2.x quadcopter.
//! 
//! The supported quadcotpers are:
//! - Crazyflie 2.0
//! - Crazyflie 2.1
//! - Crazyflie Bolt
//! - Crazyflie Brushless 2.1
//! 
//! # Crazyflie bootloader architecture
//! 
//! The Crazyflie 2.x has a radio bootloader which is the main mean by which the
//! it can be programmed and worked with. The radio bootloader gives access to two
//! separate chip bootloaders:
//! - The STM32 bootloader, which is used to program the main flight controller
//!  chip.
//! - The nRF51 bootloader, which is used to program the Crazyradio chip.
//! 
//! The nRF51 bootloader also acts as a proxy between the Crazyradio and the STM32
//! bootloader. It relays all commands to the STM32 bootloader and sends back the
//! responses.
//! 
//! This means that this crate only works over radio and so requires a Crazyradio.
//! Crazyflie 2.x does not implement a USB bootloader mode, and the STM32 USB
//! DFU mode is only available as a recovery mode.
//! 
//! This crate also only works with the Crazyflie in bootloader mode. Entering
//! bootloader mode can be done either by sending special radio commands while
//! the Crazyflie is in firmware mode or by holding the power switch for about
//! 2 seconds when powering on the Crazyflie.
//! 
//! Sending commands to enter bootloader mode is left as an exercise for another
//! crate (such as [cflib](https://github.com/bitcraze/crazyflie-lib-rs)).
//! 
//! See examples in the repository for how to use this crate.

#![deny(missing_docs)]

mod bllink;
pub mod bootloader;
mod cfloader;
pub mod packets;

pub use bllink::Bllink;
pub use bootloader::Bootloader;
pub use cfloader::CFLoader;
