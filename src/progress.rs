//! Progress reporting types for the flash process.
//!
//! The [`FlashProgress`] enum provides multi-stage progress events so callers
//! can display meaningful status during the full flash sequence.

/// Progress events emitted during the flash process.
#[derive(Debug, Clone)]
pub enum FlashProgress {
    /// Entering bootloader mode (warm or cold boot)
    EnteringBootloader,
    /// Successfully connected to bootloader
    BootloaderConnected,
    /// Flashing a target (STM32 or nRF51)
    FlashingTarget {
        /// Target name (e.g. "stm32", "nrf51")
        target: String,
        /// Bytes written so far
        bytes_written: usize,
        /// Total bytes to write
        total_bytes: usize,
    },
    /// Finished flashing a target
    FlashComplete {
        /// Target name
        target: String,
    },
    /// Resetting to firmware after bootloader phase
    ResettingToFirmware,
    /// Waiting for Crazyflie to reboot
    WaitingForReboot {
        /// Estimated wait time in seconds
        estimated_seconds: u32,
    },
    /// Connecting to running firmware for deck phase
    ConnectingForDeckPhase,
    /// Discovered attached decks
    DiscoveringDecks {
        /// Names of discovered deck sections
        found: Vec<String>,
    },
    /// Flashing deck firmware
    FlashingDeck {
        /// Deck section name
        name: String,
        /// Bytes written so far
        bytes_written: usize,
        /// Total bytes to write
        total_bytes: usize,
    },
    /// Finished flashing a deck
    DeckFlashComplete {
        /// Deck section name
        name: String,
    },
    /// All flash operations complete
    Complete,
}
