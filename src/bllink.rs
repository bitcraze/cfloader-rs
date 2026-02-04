use crazyradio::{Crazyradio, SharedCrazyradio};
use std::time::Duration;


/// # Crazyflie bootloader link
/// 
/// The bootloader link is very similar to the Crazylfie link over ESB except that it is
/// based on a very early iteration and does not implement safelink.
/// 
/// As such the link requires some special handling in order to work properly. Hence this
/// implementation is kept separate from crazyflie-link.
/// 
/// For simplicity, this implementation is used as a half-duplex link, only sending or
/// receiving at any one time
pub struct Bllink {
    radio: SharedCrazyradio,
    address: [u8; 5],
    channel: crazyradio::Channel,
}

const DEFAULT_ADDRESS: [u8; 5] = [0xE7, 0xE7, 0xE7, 0xE7, 0xE7];
const BOOTLOADER_CHANNEL: u8 = 0; // Bootloader channel
const MAX_RETRIES: usize = 10; // Maximum number of retries for packet transmission



impl Bllink {
    /// Create a new Bllink instance
    /// 
    /// This functioon uses the first found Crazyradio USB device to create the link.
    /// 
    /// # Arguments
    /// * `address` - Optional 5-byte address to use for the link. If None, the default address is used.
    ///
    /// # Returns
    /// A Result containing the Bllink instance or an error if the radio could not be opened.
    /// 
    pub async fn new(address: Option<&[u8; 5]>) -> anyhow::Result<Self> {
        let address = address.unwrap_or(&DEFAULT_ADDRESS);

        let radio = Crazyradio::open_first_async().await?;
        let radio = SharedCrazyradio::new(radio);

        Ok(Bllink { radio, channel: crazyradio::Channel::from_number(BOOTLOADER_CHANNEL).unwrap(), address: *address })
    }

    /// Create a new Bllink instance with an existing radio
    /// 
    /// This function creates a Bllink instance using an already-create SharedCrazyradio.
    /// This allows to use the bootloader in a client that already has Crazyradio instances.
    /// 
    /// # Arguments
    /// * `radio` - An existing SharedCrazyradio instance to use for the link
    /// * `address` - Optional 5-byte address to use for the link. If None, the default address is used.
    ///
    /// # Returns
    /// A Result containing the Bllink instance or an error.
    /// 
    pub async fn new_with_radio(radio: SharedCrazyradio,address: Option<&[u8; 5]>) -> anyhow::Result<Self> {
        let address = address.unwrap_or(&DEFAULT_ADDRESS);

        Ok(Bllink { radio, channel: crazyradio::Channel::from_number(BOOTLOADER_CHANNEL).unwrap(), address: *address })
    }


    /// Send a packet as request, expect one packet as response matching the request data
    ///
    /// This method sends a packet and waits for a response packet that starts with the same data as the request.
    /// If no valid response is received within the timeout duration, the request is retried up to MAX_RETRIES times.
    ///
    /// # Arguments
    ///
    /// * `data` - The packet data to send
    /// * `timeout_duration` - Maximum time to wait for a response
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing the response data
    ///
    /// # Errors
    ///
    /// Returns an error if no valid response is received after MAX_RETRIES attempts
    pub async fn request(&mut self, data: &[u8], timeout_duration: Duration) -> anyhow::Result<Vec<u8>> {
        for attempt in 0..MAX_RETRIES {
            match self.try_request(data, timeout_duration).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if attempt == MAX_RETRIES - 1 {
                        return Err(anyhow::anyhow!(
                            "Failed to get response after {} attempts: {}", 
                            MAX_RETRIES, e
                        ));
                    }
                    // Log retry attempt if desired
                    //eprintln!("Request attempt {} failed: {}, retrying...", attempt + 1, e);
                }
            }
        }
        unreachable!()
    }

    /// Send a packet as request with partial response matching
    ///
    /// Similar to [`request`](Self::request), but allows specifying how many bytes of the response
    /// must match the request. This is useful for cases where the response may contain additional
    /// data after the initial matching bytes.
    ///
    /// # Arguments
    ///
    /// * `data` - The packet data to send
    /// * `match_length` - Number of bytes from the start of the response that must match the request
    /// * `timeout_duration` - Maximum time to wait for a response
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing the response data
    ///
    /// # Errors
    ///
    /// Returns an error if no valid response is received after MAX_RETRIES attempts
    pub async fn request_match_response(&mut self, data: &[u8], match_length: usize, timeout_duration: Duration) -> anyhow::Result<Vec<u8>> {
        for attempt in 0..MAX_RETRIES {
            match self.try_request_match_response(data, match_length, timeout_duration).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if attempt == MAX_RETRIES - 1 {
                        return Err(anyhow::anyhow!(
                            "Failed to get matching response after {} attempts: {}", 
                            MAX_RETRIES, e
                        ));
                    }
                    // Log retry attempt if desired
                    //eprintln!("Request match attempt {} failed: {}, retrying...", attempt + 1, e);
                }
            }
        }
        unreachable!()
    }

    // Internal method to try a single request with partial response matching
    async fn try_request_match_response(&mut self, data: &[u8], match_length: usize, timeout_duration: Duration) -> anyhow::Result<Vec<u8>> {
        let start_time = std::time::Instant::now();
        let mut answer = Vec::new();
        let mut got_initial_ack = false;
        
        // Validate match_length
        if match_length > data.len() {
            return Err(anyhow::anyhow!("match_length {} cannot be greater than data length {}", match_length, data.len()));
        }
        
        let match_data = &data[..match_length];
        
        // First, send the initial request and wait for ACK within timeout window
        while start_time.elapsed() < timeout_duration && !got_initial_ack {
            let (ack, response) = self.radio.send_packet_async(self.channel, self.address, data.to_vec()).await
                .map_err(|e| anyhow::anyhow!("Radio error during initial send: {}", e))?;

            if ack.received {
                got_initial_ack = true;
                answer = response;
            } else {
                // Short delay before retry
                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            }
        }
        
        if !got_initial_ack {
            return Err(anyhow::anyhow!("Timeout: No ACK received for initial packet within {:?}", timeout_duration));
        }

        // Keep polling for valid response with remaining timeout
        while start_time.elapsed() < timeout_duration && (answer.len() < match_length || !answer[..match_length].eq(match_data)) {
            let (new_ack, new_answer) = self.radio.send_packet_async(self.channel, self.address, vec![0xff]).await
                .map_err(|e| anyhow::anyhow!("Radio error during polling: {}", e))?;

            if new_ack.received {
                answer = new_answer;
            }
            
            // Short delay before next poll
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        }
        
        if answer.len() < match_length || !answer[..match_length].eq(match_data) {
            return Err(anyhow::anyhow!(
                "Timeout: No valid response received within {:?}. Expected first {} bytes to match {:02X?}, got {:02X?}", 
                timeout_duration, match_length, match_data, 
                if answer.len() >= match_length { &answer[..match_length] } else { &answer }
            ));
        }

        Ok(answer)
    }

    // Internal method to try a single request with timeout
    async fn try_request(&mut self, data: &[u8], timeout_duration: Duration) -> anyhow::Result<Vec<u8>> {
        let start_time = std::time::Instant::now();
        let mut answer = Vec::new();
        let mut got_initial_ack = false;
        
        // First, send the initial request and wait for ACK within timeout window
        while start_time.elapsed() < timeout_duration && !got_initial_ack {
            let (ack, response) = self.radio.send_packet_async(self.channel, self.address, data.to_vec()).await
                .map_err(|e| anyhow::anyhow!("Radio error during initial send: {}", e))?;

            if ack.received {
                got_initial_ack = true;
                answer = response;
            } else {
                // Short delay before retry
                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            }
        }
        
        if !got_initial_ack {
            return Err(anyhow::anyhow!("Timeout: No ACK received for initial packet within {:?}", timeout_duration));
        }

        // Keep polling for valid response with remaining timeout
        while start_time.elapsed() < timeout_duration && !answer.starts_with(data) {
            let (new_ack, new_answer) = self.radio.send_packet_async(self.channel, self.address, vec![0xff]).await
                .map_err(|e| anyhow::anyhow!("Radio error during polling: {}", e))?;

            if new_ack.received {
                answer = new_answer;
            }
            
            // Short delay before next poll
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        }
        
        if !answer.starts_with(data) {
            return Err(anyhow::anyhow!("Timeout: No valid response received within {:?}", timeout_duration));
        }

        Ok(answer)
    }

    /// Send a packet without expecting a response
    ///
    /// Sends a packet and waits only for acknowledgment (ACK) from the radio.
    /// Uses a default timeout of 1000ms.
    ///
    /// # Arguments
    ///
    /// * `data` - The packet data to send
    ///
    /// # Returns
    ///
    /// An empty result indicating success or failure
    pub async fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.send_with_timeout(data, Duration::from_millis(1000)).await
    }

    /// Send a packet with custom timeout, without expecting a response
    ///
    /// Sends a packet and waits only for acknowledgment (ACK) from the radio.
    /// Retries up to MAX_RETRIES times if no ACK is received.
    ///
    /// # Arguments
    ///
    /// * `data` - The packet data to send
    /// * `timeout_duration` - Maximum time to wait for ACK on each attempt
    ///
    /// # Returns
    ///
    /// An empty result indicating success or failure
    ///
    /// # Errors
    ///
    /// Returns an error if no ACK is received after MAX_RETRIES attempts
    pub async fn send_with_timeout(&mut self, data: &[u8], timeout_duration: Duration) -> anyhow::Result<()> {
        for attempt in 0..MAX_RETRIES {
            match self.try_send(data, timeout_duration).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    if attempt == MAX_RETRIES - 1 {
                        return Err(anyhow::anyhow!(
                            "Failed to send packet after {} attempts: {}", 
                            MAX_RETRIES, e
                        ));
                    }
                }
            }
        }
        unreachable!()
    }

    // Internal method to try a single send with timeout
    async fn try_send(&mut self, data: &[u8], timeout_duration: Duration) -> anyhow::Result<()> {
        let start_time = std::time::Instant::now();
        
        while start_time.elapsed() < timeout_duration {
            let (ack, _answer) = self.radio.send_packet_async(self.channel, self.address, data.to_vec()).await
                .map_err(|e| anyhow::anyhow!("Radio error during send: {}", e))?;

            if ack.received {
                return Ok(());
            }
            
            // Short delay before retry
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        }
        
        Err(anyhow::anyhow!("Timeout: No ACK received within {:?}", timeout_duration))
    }
}