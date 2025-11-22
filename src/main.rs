use anyhow::{Context, Result};
use clap::Parser;
use serialport;
use std::io::{self, Read};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Name of the serial port to use (e.g., /dev/ttyUSB0 or COM3)
    #[arg(short, long)]
    port: String,

    /// Baud rate (default: 9600)
    #[arg(short, long, default_value_t = 9600)]
    baud_rate: u32,
}

#[derive(Debug, Clone)]
struct SensorData {
    eco2: u16,
    ech2o: u16,
    tvoc: u16,
    pm2_5: u16,
    pm10: u16,
    temperature: f32,
    humidity: f32,
}

const FRAME_HEADER_1: u8 = 0x3C;
const FRAME_HEADER_2: u8 = 0x02;
const FRAME_LEN: usize = 17;

fn calculate_checksum(data: &[u8]) -> u8 {
    let mut sum: u16 = 0;
    for &b in data {
        sum = sum.wrapping_add(b as u16);
    }
    (sum & 0xFF) as u8
}

fn parse_frame(buffer: &[u8]) -> Option<SensorData> {
    if buffer.len() < FRAME_LEN {
        return None;
    }

    // Verify headers
    if buffer[0] != FRAME_HEADER_1 || buffer[1] != FRAME_HEADER_2 {
        return None;
    }

    // Verify checksum
    let calculated_sum = calculate_checksum(&buffer[0..16]);
    if calculated_sum != buffer[16] {
        eprintln!(
            "Checksum mismatch: expected {:02X}, got {:02X}",
            calculated_sum, buffer[16]
        );
        return None;
    }

    let eco2 = u16::from_be_bytes([buffer[2], buffer[3]]);
    let ech2o = u16::from_be_bytes([buffer[4], buffer[5]]);
    let tvoc = u16::from_be_bytes([buffer[6], buffer[7]]);
    let pm2_5 = u16::from_be_bytes([buffer[8], buffer[9]]);
    let pm10 = u16::from_be_bytes([buffer[10], buffer[11]]);

    let temp_int = buffer[12];
    let temp_dec = buffer[13];
    let temperature = temp_int as f32 + (temp_dec as f32 / 10.0);

    let hum_int = buffer[14];
    let hum_dec = buffer[15];
    let humidity = hum_int as f32 + (hum_dec as f32 / 10.0);

    Some(SensorData {
        eco2,
        ech2o,
        tvoc,
        pm2_5,
        pm10,
        temperature,
        humidity,
    })
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Opening port {} at {} baud...", args.port, args.baud_rate);

    let mut port = serialport::new(&args.port, args.baud_rate)
        .timeout(Duration::from_millis(1000))
        .open()
        .with_context(|| format!("Failed to open port '{}'", args.port))?;

    println!("Port opened. Waiting for data...");

    let mut serial_buf: Vec<u8> = vec![0; 1000];
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        match port.read(serial_buf.as_mut_slice()) {
            Ok(t) => {
                buffer.extend_from_slice(&serial_buf[..t]);

                // Process buffer
                while buffer.len() >= FRAME_LEN {
                    // Look for header
                    if let Some(start_idx) = buffer.iter().position(|&x| x == FRAME_HEADER_1) {
                        // Remove garbage before header
                        if start_idx > 0 {
                            buffer.drain(0..start_idx);
                        }

                        // Check if we have enough data for a full frame
                        if buffer.len() < FRAME_LEN {
                            break; // Wait for more data
                        }

                        // Check second header byte
                        if buffer[1] != FRAME_HEADER_2 {
                            // Invalid header sequence, remove the first byte and try again
                            buffer.remove(0);
                            continue;
                        }

                        // Try to parse the frame
                        let frame_bytes = &buffer[0..FRAME_LEN];
                        if let Some(data) = parse_frame(frame_bytes) {
                            println!("Received: {:?}", data);
                            // Remove the processed frame
                            buffer.drain(0..FRAME_LEN);
                        } else {
                            // Checksum failed or other error, remove header and try again
                            // (Or maybe just remove the whole frame?
                            // Safest is to remove 1 byte to search for next header,
                            // but if checksum failed it might be a collision.
                            // Let's remove 1 byte to be safe and resync.)
                            buffer.remove(0);
                        }
                    } else {
                        // No header found in the entire buffer, clear it
                        buffer.clear();
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                // Timeout is fine, just continue
                continue;
            }
            Err(e) => {
                eprintln!("Error reading serial port: {:?}", e);
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_checksum() {
        // B1..B16
        // Example: 3C 02 ...
        // Let's make a dummy frame
        let data = vec![
            0x3C, 0x02, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 10, 5, 20, 5,
        ];
        // Sum: 60 + 2 + 1 + 1 + 1 + 1 + 1 + 10 + 5 + 20 + 5 = 107 (0x6B)
        // 0x3C = 60
        // 0x02 = 2
        // 5 * 0x01 = 5
        // 10 = 0x0A
        // 5 = 0x05
        // 20 = 0x14
        // 5 = 0x05
        // Total: 60+2+5+10+5+20+5 = 107

        let checksum = calculate_checksum(&data);
        assert_eq!(checksum, 107);
    }

    #[test]
    fn test_parse_frame_valid() {
        let mut data = vec![
            0x3C, 0x02, // Header
            0x01, 0x90, // eCO2 = 400
            0x00, 0x05, // eCH2O = 5
            0x00, 0x0A, // TVOC = 10
            0x00, 0x14, // PM2.5 = 20
            0x00, 0x1E, // PM10 = 30
            25, 5, // Temp = 25.5
            50, 2, // Hum = 50.2
        ];
        let checksum = calculate_checksum(&data);
        data.push(checksum);

        let result = parse_frame(&data);
        assert!(result.is_some());
        let sensor_data = result.unwrap();

        assert_eq!(sensor_data.eco2, 400);
        assert_eq!(sensor_data.ech2o, 5);
        assert_eq!(sensor_data.tvoc, 10);
        assert_eq!(sensor_data.pm2_5, 20);
        assert_eq!(sensor_data.pm10, 30);
        assert_eq!(sensor_data.temperature, 25.5);
        assert_eq!(sensor_data.humidity, 50.2);
    }

    #[test]
    fn test_parse_frame_invalid_checksum() {
        let mut data = vec![
            0x3C, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let checksum = calculate_checksum(&data);
        data.push(checksum + 1); // Invalid

        assert!(parse_frame(&data).is_none());
    }
}
