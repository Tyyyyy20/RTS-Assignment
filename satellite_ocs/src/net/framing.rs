// Simple packet framer used for network communication.
// The first 4 bytes store the payload length (big-endian).
// Format: [length: u32][payload bytes]
#[derive(Default, Clone)]
pub struct Framer;

impl Framer {
    /// Extract a complete framed packet from the buffer.
    /// Checks that the buffer contains at least the header
    /// and the full payload before returning the frame.
    pub fn deframe<'a>(&self, buf: &'a [u8]) -> anyhow::Result<&'a [u8]> {
        // Need at least 4 bytes to read the length field
        if buf.len() < 4 { anyhow::bail!("short"); }
        // Read payload length from the first 4 bytes
        let len = u32::from_be_bytes([buf[0],buf[1],buf[2],buf[3]]) as usize;
        // Ensure the full frame is available
        if buf.len() < 4 + len { anyhow::bail!("incomplete"); }
        // Return the complete frame (header + payload)
        Ok(&buf[..4+len])
    }

    /// Create a framed packet by adding a 4-byte length prefix.
    /// Output format: [length][payload]
    pub fn frame(&self, data: &[u8]) -> Vec<u8> {
        let len = data.len() as u32;
        // Start with the length header
        let mut framed = len.to_be_bytes().to_vec();
        // Append the payload
        framed.extend_from_slice(data);
        framed
    }
}
