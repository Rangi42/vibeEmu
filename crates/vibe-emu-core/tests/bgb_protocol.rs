//! Tests for the BGB link cable protocol implementation.
//!
//! These tests verify the BGB protocol packet handling based on the bgb-to-bgb.log
//! captured from two real BGB emulator instances communicating.
//!
//! BGB Protocol overview (v1.4):
//! - All packets are 8 bytes: b1 (command), b2, b3, b4, i1 (4-byte LE timestamp)
//! - CMD_VERSION (1): Version handshake
//! - CMD_SYNC1 (104): Master initiates transfer
//! - CMD_SYNC2 (105): Slave responds with data
//! - CMD_SYNC3 (106): Acknowledgment (ack or timestamp)
//! - CMD_STATUS (108): Pause/resume state
//!
//! Flow:
//! 1. Handshake: VERSION -> VERSION -> STATUS -> STATUS
//! 2. Transfer: Master sends SYNC1 -> Slave sends SYNC2 (0xFE when not ready, or data when ready)
//! 3. After SYNC2, master sends SYNC3 with timestamp

/// BGB protocol command codes
const CMD_VERSION: u8 = 1;
const _CMD_JOYPAD: u8 = 101;
const CMD_SYNC1: u8 = 104;
const CMD_SYNC2: u8 = 105;
const CMD_SYNC3: u8 = 106;
const CMD_STATUS: u8 = 108;
const _CMD_WANTDISCONNECT: u8 = 109;

/// BGB status flags
const STATUS_RUNNING: u8 = 0x01;
const STATUS_PAUSED: u8 = 0x02;
const STATUS_SUPPORT_RECONNECT: u8 = 0x04;

/// Represents a BGB protocol packet
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BgbPacket {
    b1: u8, // command
    b2: u8, // data byte (for SYNC1/2) or flags (for STATUS)
    b3: u8, // control byte (for SYNC1/2)
    b4: u8,
    i1: u32, // timestamp
}

impl BgbPacket {
    fn to_bytes(self) -> [u8; 8] {
        let mut buf = [0u8; 8];
        buf[0] = self.b1;
        buf[1] = self.b2;
        buf[2] = self.b3;
        buf[3] = self.b4;
        buf[4..8].copy_from_slice(&self.i1.to_le_bytes());
        buf
    }

    fn from_bytes(buf: &[u8; 8]) -> Self {
        Self {
            b1: buf[0],
            b2: buf[1],
            b3: buf[2],
            b4: buf[3],
            i1: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        }
    }

    fn from_hex(hex: &str) -> Self {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes);
        Self::from_bytes(&buf)
    }

    fn version() -> Self {
        Self {
            b1: CMD_VERSION,
            b2: 1,
            b3: 4,
            b4: 0,
            i1: 0,
        }
    }

    fn status(running: bool, paused: bool, support_reconnect: bool) -> Self {
        let mut flags = 0u8;
        if running {
            flags |= STATUS_RUNNING;
        }
        if paused {
            flags |= STATUS_PAUSED;
        }
        if support_reconnect {
            flags |= STATUS_SUPPORT_RECONNECT;
        }
        Self {
            b1: CMD_STATUS,
            b2: flags,
            b3: 0,
            b4: 0,
            i1: 0,
        }
    }

    fn sync1(data: u8, timestamp: u32) -> Self {
        Self {
            b1: CMD_SYNC1,
            b2: data,
            b3: 0x85, // MASTER control
            b4: 0,
            i1: timestamp,
        }
    }

    fn sync2(data: u8) -> Self {
        Self {
            b1: CMD_SYNC2,
            b2: data,
            b3: 0x80,
            b4: 1,
            i1: 0,
        }
    }

    fn sync3_ack() -> Self {
        Self {
            b1: CMD_SYNC3,
            b2: 1,
            b3: 0,
            b4: 0,
            i1: 0,
        }
    }

    fn sync3_timestamp(timestamp: u32) -> Self {
        Self {
            b1: CMD_SYNC3,
            b2: 0,
            b3: 0,
            b4: 0,
            i1: timestamp,
        }
    }
}

#[test]
fn version_packet_format() {
    let packet = BgbPacket::version();
    let bytes = packet.to_bytes();
    // From bgb-to-bgb.log: [0101040000000000]
    assert_eq!(bytes, [0x01, 0x01, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]);
}

#[test]
fn status_packet_running_paused_reconnect() {
    let packet = BgbPacket::status(true, true, true);
    assert_eq!(packet.b1, CMD_STATUS);
    assert_eq!(
        packet.b2,
        STATUS_RUNNING | STATUS_PAUSED | STATUS_SUPPORT_RECONNECT
    );
    // From log: flags=RUNNING,PAUSED,SUPPORT_RECONNECT [6c07000000000000]
    assert_eq!(packet.b2, 0x07);
}

#[test]
fn status_packet_running_reconnect() {
    let packet = BgbPacket::status(true, false, true);
    // From log: flags=RUNNING,SUPPORT_RECONNECT [6c05000000000000]
    assert_eq!(packet.b2, 0x05);
}

#[test]
fn sync1_packet_from_log() {
    // From bgb-to-bgb.log: SERVER <- SYNC1 data=0x01 ctrl=0x85 ts=49667669 [6801850055def502]
    let packet = BgbPacket::from_hex("6801850055def502");
    assert_eq!(packet.b1, CMD_SYNC1);
    assert_eq!(packet.b2, 0x01); // data
    assert_eq!(packet.b3, 0x85); // MASTER control
    assert_eq!(packet.i1, 49667669);
}

#[test]
fn sync2_packet_from_log() {
    // From bgb-to-bgb.log: CLIENT -> SYNC2 data=0x02 ctrl=0x80 [6902800100000000]
    let packet = BgbPacket::from_hex("6902800100000000");
    assert_eq!(packet.b1, CMD_SYNC2);
    assert_eq!(packet.b2, 0x02); // data
    assert_eq!(packet.b3, 0x80); // control
    assert_eq!(packet.b4, 0x01);
}

#[test]
fn sync3_ack_packet_from_log() {
    // From bgb-to-bgb.log: CLIENT -> SYNC3 type=ack [6a01000000000000]
    let packet = BgbPacket::from_hex("6a01000000000000");
    assert_eq!(packet.b1, CMD_SYNC3);
    assert_eq!(packet.b2, 0x01); // ack type
    assert_eq!(packet.i1, 0);
}

#[test]
fn sync1_creates_correct_packet() {
    let packet = BgbPacket::sync1(0x42, 12345678);
    assert_eq!(packet.b1, CMD_SYNC1);
    assert_eq!(packet.b2, 0x42);
    assert_eq!(packet.b3, 0x85);
    assert_eq!(packet.i1, 12345678);
}

#[test]
fn sync2_creates_correct_packet() {
    let packet = BgbPacket::sync2(0xAB);
    assert_eq!(packet.b1, CMD_SYNC2);
    assert_eq!(packet.b2, 0xAB);
    assert_eq!(packet.b3, 0x80);
    assert_eq!(packet.b4, 1);
}

#[test]
fn sync3_ack_creates_correct_packet() {
    let packet = BgbPacket::sync3_ack();
    let bytes = packet.to_bytes();
    assert_eq!(bytes, [0x6A, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
}

#[test]
fn sync3_timestamp_creates_correct_packet() {
    let packet = BgbPacket::sync3_timestamp(0x12345678);
    assert_eq!(packet.b1, CMD_SYNC3);
    assert_eq!(packet.b2, 0);
    assert_eq!(packet.i1, 0x12345678);
}

/// Simulates the expected packet exchange from bgb-to-bgb.log
/// when both emulators start but only the server (master) is ready.
#[test]
fn handshake_sequence() {
    // Expected sequence from bgb-to-bgb.log:
    // CLIENT -> VERSION [0101040000000000]
    // SERVER <- VERSION [0101040000000000]
    // SERVER <- STATUS [6c07000000000000] flags=RUNNING,PAUSED,SUPPORT_RECONNECT
    // SERVER <- STATUS [6c05000000000000] flags=RUNNING,SUPPORT_RECONNECT
    // CLIENT -> STATUS [6c05000000000000]

    let client_version = BgbPacket::version();
    let server_version = BgbPacket::version();

    assert_eq!(client_version, server_version);

    let server_status_paused = BgbPacket::status(true, true, true);
    assert_eq!(server_status_paused.b2, 0x07);

    let server_status_running = BgbPacket::status(true, false, true);
    assert_eq!(server_status_running.b2, 0x05);

    let client_status = BgbPacket::status(true, false, true);
    assert_eq!(client_status.b2, 0x05);
}

/// Tests the SYNC1 -> SYNC3 ack pattern when slave is not ready.
/// This is observed in the bgb-to-bgb.log before the game reaches the trade lobby.
#[test]
fn sync1_sync3_ack_when_slave_not_ready() {
    // From bgb-to-bgb.log:
    // SERVER <- SYNC1 data=0x01 ctrl=0x85 ts=49667669 [6801850055def502]
    // CLIENT -> SYNC3 type=ack [6a01000000000000]
    // SERVER <- SYNC1 data=0x01 ctrl=0x85 ts=49702721 [680185004167f602]
    // CLIENT -> SYNC3 type=ack [6a01000000000000]
    // ... repeats until client is ready

    let sync1 = BgbPacket::sync1(0x01, 49667669);
    let sync3_ack = BgbPacket::sync3_ack();

    // When slave receives SYNC1 but game isn't ready, it sends SYNC3 ack
    assert_eq!(sync1.b1, CMD_SYNC1);
    assert_eq!(sync1.b2, 0x01);

    assert_eq!(sync3_ack.b1, CMD_SYNC3);
    assert_eq!(sync3_ack.b2, 1); // ack type
}

/// Tests the SYNC1 -> SYNC2 pattern when slave is ready.
#[test]
fn sync1_sync2_when_slave_ready() {
    // From bgb-to-bgb.log:
    // SERVER <- SYNC1 data=0x01 ctrl=0x85 ts=50615653 [6801850065550403]
    // CLIENT -> SYNC2 data=0x02 ctrl=0x80 [6902800100000000]

    let sync1 = BgbPacket::sync1(0x01, 50615653);
    let sync2 = BgbPacket::sync2(0x02);

    assert_eq!(sync1.b2, 0x01); // master sends 0x01
    assert_eq!(sync2.b2, 0x02); // slave responds with 0x02
}

/// Tests the 0xFE (SERIAL_NO_DATA_BYTE) pattern from Pokemon games.
/// When a game isn't ready, it responds with 0xFE.
#[test]
fn serial_no_data_byte_pattern() {
    // From bgb-to-bgb.log:
    // SERVER <- SYNC1 data=0x00 ctrl=0x85 ts=50685845 [6800850095670503]
    // CLIENT -> SYNC2 data=0xFE ctrl=0x80 [69fe800100000000]
    // ... long gap ...
    // SERVER <- SYNC1 data=0x00 ctrl=0x85 ts=61007317 [68008500d5e5a203]
    // CLIENT -> SYNC2 data=0xFE ctrl=0x80 [69fe800100000000]

    let sync2_not_ready = BgbPacket::sync2(0xFE);
    assert_eq!(sync2_not_ready.b2, 0xFE);
}

/// Tests the Pokemon trade handshake pattern (0x81 exchange).
#[test]
fn pokemon_trade_handshake_81() {
    // From bgb-to-bgb.log during trade setup:
    // SERVER <- SYNC1 data=0x81 ctrl=0x85 ...
    // CLIENT -> SYNC2 data=0x81 ctrl=0x80 ...

    let sync1 = BgbPacket::sync1(0x81, 62271281);
    let sync2 = BgbPacket::sync2(0x81);

    assert_eq!(sync1.b2, 0x81);
    assert_eq!(sync2.b2, 0x81);
}

/// Tests the rapid-fire data exchange pattern during actual Pokemon trading.
/// Multiple SYNC1/SYNC2 pairs in quick succession.
#[test]
fn rapid_data_exchange() {
    // From bgb-to-bgb.log during data transfer:
    // Very quick succession of transfers (~1-2ms apart)
    let transfers = [
        (0xFD, 0xFD),
        (0xFB, 0xFD),
        (0x02, 0xFB),
        (0x00, 0x02),
        (0x00, 0x00),
        (0xFD, 0x00),
        (0xFD, 0xFD),
    ];

    for (master_data, slave_data) in transfers {
        let sync1 = BgbPacket::sync1(master_data, 0);
        let sync2 = BgbPacket::sync2(slave_data);

        assert_eq!(sync1.b2, master_data);
        assert_eq!(sync2.b2, slave_data);
    }
}

/// Verifies timestamp encoding/decoding.
#[test]
fn timestamp_encoding() {
    // Timestamps are 31-bit values in 2 MiHz clocks
    let ts: u32 = 49667669; // From first SYNC1 in log
    let packet = BgbPacket::sync1(0x01, ts);
    let bytes = packet.to_bytes();

    // Reconstruct from bytes
    let restored = BgbPacket::from_bytes(&bytes);
    assert_eq!(restored.i1, ts);
}

/// Tests that SYNC3 with timestamp has b2=0 and i1=timestamp.
#[test]
fn sync3_timestamp_vs_ack_distinction() {
    let ack = BgbPacket::sync3_ack();
    let ts = BgbPacket::sync3_timestamp(12345);

    // Ack has b2=1, i1=0
    assert_eq!(ack.b2, 1);
    assert_eq!(ack.i1, 0);

    // Timestamp has b2=0, i1=timestamp
    assert_eq!(ts.b2, 0);
    assert_eq!(ts.i1, 12345);
}

/// Simulates a complete transfer cycle as observed in the log.
#[test]
fn complete_transfer_cycle() {
    // Master: sends SYNC1 with data
    let sync1 = BgbPacket::sync1(0x42, 50000000);

    // Slave: receives SYNC1, sends SYNC3 ack (game not ready yet)
    let sync3_ack = BgbPacket::sync3_ack();

    // Master: retries SYNC1
    let sync1_retry = BgbPacket::sync1(0x42, 50035000);

    // Slave: now ready, sends SYNC2 with response
    let sync2 = BgbPacket::sync2(0x24);

    // Master: receives SYNC2, sends SYNC3 with timestamp
    let sync3_ts = BgbPacket::sync3_timestamp(50070000);

    // Verify packet types
    assert_eq!(sync1.b1, CMD_SYNC1);
    assert_eq!(sync3_ack.b1, CMD_SYNC3);
    assert_eq!(sync1_retry.b1, CMD_SYNC1);
    assert_eq!(sync2.b1, CMD_SYNC2);
    assert_eq!(sync3_ts.b1, CMD_SYNC3);
}

/// Tests packet roundtrip through bytes.
#[test]
fn packet_roundtrip() {
    let packets = [
        BgbPacket::version(),
        BgbPacket::status(true, false, true),
        BgbPacket::sync1(0xAB, 0x12345678),
        BgbPacket::sync2(0xCD),
        BgbPacket::sync3_ack(),
        BgbPacket::sync3_timestamp(0xDEADBEEF),
    ];

    for packet in packets {
        let bytes = packet.to_bytes();
        let restored = BgbPacket::from_bytes(&bytes);
        assert_eq!(packet, restored);
    }
}

/// Tests parsing real packets from the bgb-to-bgb.log.
#[test]
fn parse_real_log_packets() {
    let test_cases = [
        ("0101040000000000", CMD_VERSION, 1, 4),     // VERSION
        ("6c07000000000000", CMD_STATUS, 0x07, 0),   // STATUS running+paused+reconnect
        ("6c05000000000000", CMD_STATUS, 0x05, 0),   // STATUS running+reconnect
        ("6801850055def502", CMD_SYNC1, 0x01, 0x85), // SYNC1 data=0x01
        ("6a01000000000000", CMD_SYNC3, 0x01, 0),    // SYNC3 ack
        ("6902800100000000", CMD_SYNC2, 0x02, 0x80), // SYNC2 data=0x02
        ("69fe800100000000", CMD_SYNC2, 0xFE, 0x80), // SYNC2 data=0xFE (not ready)
        ("6981800100000000", CMD_SYNC2, 0x81, 0x80), // SYNC2 data=0x81 (trade handshake)
    ];

    for (hex, expected_cmd, expected_b2, expected_b3) in test_cases {
        let packet = BgbPacket::from_hex(hex);
        assert_eq!(packet.b1, expected_cmd, "Command mismatch for {}", hex);
        assert_eq!(packet.b2, expected_b2, "b2 mismatch for {}", hex);
        assert_eq!(packet.b3, expected_b3, "b3 mismatch for {}", hex);
    }
}

// ============================================================================
// Tests documenting the BGB vs Vibe protocol behavior differences
// ============================================================================

/// Documents BGB's SYNC3 behavior from bgb-to-bgb.log:
/// - SYNC3 ack is sent ONLY when slave is NOT ready (hasn't received response yet)
/// - Once slave sends SYNC2, no more SYNC3 acks are sent
///
/// From bgb-to-bgb.log lines 7-62:
/// - Lines 7-61: SYNC1 -> SYNC3 (slave not ready, just acknowledging)
/// - Line 62: SYNC1 -> SYNC2 (slave IS ready, responding with data)
/// - Lines 63+: All SYNC1 -> SYNC2 (no more SYNC3 acks)
#[test]
fn bgb_sync3_sent_only_when_not_ready() {
    // Before slave game has initiated serial transfer:
    // Master sends SYNC1, slave sends SYNC3 ack
    let exchanges_before_ready = [
        ("6801850055def502", "6a01000000000000"), // SYNC1 -> SYNC3
        ("680185004167f602", "6a01000000000000"), // SYNC1 -> SYNC3
        ("6801850065f0f602", "6a01000000000000"), // SYNC1 -> SYNC3
    ];

    for (sync1_hex, response_hex) in exchanges_before_ready {
        let sync1 = BgbPacket::from_hex(sync1_hex);
        let response = BgbPacket::from_hex(response_hex);
        assert_eq!(sync1.b1, CMD_SYNC1, "Expected SYNC1");
        assert_eq!(response.b1, CMD_SYNC3, "Expected SYNC3 ack when not ready");
        assert_eq!(response.b2, 1, "SYNC3 ack should have b2=1");
    }

    // After slave game has initiated serial transfer:
    // Master sends SYNC1, slave sends SYNC2 directly
    let exchanges_after_ready = [
        ("6801850065550403", "6902800100000000"), // SYNC1 -> SYNC2 data=0x02
        ("6800850075de0403", "6900800100000000"), // SYNC1 -> SYNC2 data=0x00
        ("6800850095670503", "69fe800100000000"), // SYNC1 -> SYNC2 data=0xFE
    ];

    for (sync1_hex, response_hex) in exchanges_after_ready {
        let sync1 = BgbPacket::from_hex(sync1_hex);
        let response = BgbPacket::from_hex(response_hex);
        assert_eq!(sync1.b1, CMD_SYNC1, "Expected SYNC1");
        assert_eq!(response.b1, CMD_SYNC2, "Expected SYNC2 directly when ready");
    }
}

/// Documents the WRONG behavior seen in vibe-to-vibe-9.log:
/// - Vibe sends SYNC3 ack for EVERY SYNC1
/// - Then later sends SYNC2
///
/// This is incorrect - causes extra round-trips and timing issues.
///
/// From vibe-to-vibe-9.log:
/// - SYNC1 -> SYNC3 ack -> SYNC2 (always, even when ready)
/// - Pattern repeats: SYNC1, SYNC3, SYNC2, SYNC1, SYNC3, SYNC2...
#[test]
fn vibe_incorrect_sync3_for_every_sync1() {
    // Vibe's incorrect pattern (from vibe-to-vibe-9.log lines 6-14):
    // 1. Master sends SYNC1
    // 2. Slave sends SYNC3 ack
    // 3. (Later) Slave sends SYNC2
    //
    // This is wrong because:
    // - BGB only sends SYNC3 when NOT ready (no pending response)
    // - Once ready, BGB sends SYNC2 directly (no SYNC3)
    //
    // The extra SYNC3 causes:
    // 1. Extra network round-trip latency
    // 2. Potential timing desync
    // 3. Different packet sequence than BGB expects

    // These are actual packets from vibe-to-vibe-9.log showing the bug:
    let vibe_pattern = [
        // Line 6-8: SYNC1, SYNC3, SYNC2
        ("680181002c889403", CMD_SYNC1), // Master sends SYNC1
        ("6a01000000000000", CMD_SYNC3), // Slave sends SYNC3 ack (WRONG!)
        ("6902800100000000", CMD_SYNC2), // Slave sends SYNC2

                                         // This pattern repeats for EVERY exchange in vibe-to-vibe-9.log
                                         // Even when slave has data ready, it still sends SYNC3 first
    ];

    let sync1 = BgbPacket::from_hex(vibe_pattern[0].0);
    let sync3 = BgbPacket::from_hex(vibe_pattern[1].0);
    let sync2 = BgbPacket::from_hex(vibe_pattern[2].0);

    assert_eq!(sync1.b1, CMD_SYNC1);
    assert_eq!(sync3.b1, CMD_SYNC3);
    assert_eq!(sync2.b1, CMD_SYNC2);

    // NOTE: This test documents the BUG.
    // The fix should make vibe behave like BGB:
    // - Only send SYNC3 when slave game has NOT initiated a serial transfer
    // - Send SYNC2 directly when slave game HAS data to respond with
}

/// Demonstrates the correct BGB SYNC1 response logic:
/// - If slave has pending outgoing transfer (game wrote to SB and set SC): send SYNC2
/// - If slave has no pending transfer: send SYNC3 ack
#[test]
fn correct_sync1_response_logic() {
    // Scenario 1: Slave game has NOT initiated transfer
    // (Serial port SB not written, SC transfer not started)
    // -> Slave should send SYNC3 ack
    // -> Later, when game initiates transfer, slave sends SYNC2

    // Scenario 2: Slave game HAS initiated transfer
    // (Game wrote to SB and set SC bit 7, waiting for external clock)
    // -> Slave should send SYNC2 immediately with the SB data
    // -> No SYNC3 ack needed

    // This is how BGB works. To implement correctly:
    // 1. On receiving SYNC1, check if serial port has external clock transfer pending
    // 2. If pending: immediately get SB value and send SYNC2
    // 3. If not pending: send SYNC3 ack, store the incoming data
    //    Later when game initiates transfer, send SYNC2

    // The key difference:
    // - BGB: SYNC2 response is immediate when ready
    // - Vibe: Always SYNC3 first, then SYNC2 on next main loop iteration

    // This test exists as documentation of the expected response logic.
}
