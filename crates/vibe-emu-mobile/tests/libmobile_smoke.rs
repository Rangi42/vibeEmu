#![cfg(feature = "bundled")]

use vibe_emu_mobile::{
    MOBILE_CONFIG_SIZE, MobileAdapter, MobileAddr, MobileHost, MobileNumber, MobileSockType,
};

struct MemHost {
    config: Vec<u8>,
}

impl Default for MemHost {
    fn default() -> Self {
        Self {
            config: vec![0u8; MOBILE_CONFIG_SIZE],
        }
    }
}

impl MobileHost for MemHost {
    fn config_read(&mut self, dest: &mut [u8], offset: usize) -> bool {
        if offset + dest.len() > self.config.len() {
            return false;
        }
        dest.copy_from_slice(&self.config[offset..offset + dest.len()]);
        true
    }

    fn config_write(&mut self, src: &[u8], offset: usize) -> bool {
        if offset + src.len() > self.config.len() {
            return false;
        }
        self.config[offset..offset + src.len()].copy_from_slice(src);
        true
    }

    fn sock_open(
        &mut self,
        _conn: u32,
        _socktype: MobileSockType,
        _addr: &MobileAddr,
        _bind_port: u16,
    ) -> bool {
        false
    }

    fn sock_close(&mut self, _conn: u32) {}

    fn sock_connect(&mut self, _conn: u32, _addr: &MobileAddr) -> i32 {
        -1
    }

    fn sock_listen(&mut self, _conn: u32) -> bool {
        false
    }

    fn sock_accept(&mut self, _conn: u32) -> bool {
        false
    }

    fn sock_send(&mut self, _conn: u32, _data: &[u8], _addr: Option<&MobileAddr>) -> i32 {
        -1
    }

    fn sock_recv(
        &mut self,
        _conn: u32,
        _data: Option<&mut [u8]>,
        _addr_out: Option<&mut MobileAddr>,
    ) -> i32 {
        0
    }

    fn update_number(&mut self, _which: MobileNumber, _number: Option<&str>) {}
}

fn checksum16_sum(bytes: &[u8]) -> u16 {
    let mut sum: u16 = 0;
    for &b in bytes {
        sum = sum.wrapping_add(b as u16);
    }
    sum
}

fn build_request_frame(command: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= 0xFF);

    let mut buf = Vec::with_capacity(2 + 4 + payload.len() + 2);
    buf.push(0x99);
    buf.push(0x66);

    let header = [command, 0x00, 0x00, payload.len() as u8];
    buf.extend_from_slice(&header);
    buf.extend_from_slice(payload);

    let sum = checksum16_sum(&buf[2..]);
    buf.push((sum >> 8) as u8);
    buf.push((sum & 0xFF) as u8);

    buf
}

#[test]
#[cfg(feature = "bundled")]
fn begin_session_roundtrip_emits_response_frame() {
    let host = Box::new(MemHost::default());
    let mut adapter = MobileAdapter::new(host).expect("create adapter");
    adapter.start().expect("start");

    // BEGIN_SESSION with payload "NINTENDO".
    let req = build_request_frame(0x10, b"NINTENDO");

    // Send request bytes. libmobile stays idle until checksum completes.
    let mut last = 0u8;
    for &b in &req {
        last = adapter.transfer_byte(b).expect("transfer");
    }

    // On the last checksum byte, the adapter should respond with device|0x80.
    assert_eq!(last, 0x88, "expected BLUE adapter device|0x80");

    // Send acknowledgement byte from client.
    let cmd_ack = adapter.transfer_byte(0x80).expect("transfer");
    assert_eq!(cmd_ack, 0x90, "expected BEGIN_SESSION^0x80");

    // One-byte delay, then send idle byte 0x4B to trigger command processing.
    let _ = adapter.transfer_byte(0x00).expect("transfer");
    let _ = adapter.transfer_byte(0x4B).expect("transfer");

    // Drive libmobile to process the command and craft a response.
    adapter.poll(0).expect("poll");

    // Read response stream by clocking dummy bytes.
    let mut resp = Vec::new();
    for _ in 0..16 {
        resp.push(adapter.transfer_byte(0x00).expect("transfer"));
    }

    assert_eq!(&resp[0..2], &[0x99, 0x66], "response frame start");

    // Header starts at resp[2..6]
    assert_eq!(resp[2], 0x90, "response command should be 0x10|0x80");
    assert_eq!(resp[3], 0x00);
    assert_eq!(resp[4], 0x00);
    assert_eq!(resp[5], 8);
    assert_eq!(&resp[6..14], b"NINTENDO");

    // Validate checksum.
    let expected = checksum16_sum(&resp[2..14]);
    let got = ((resp[14] as u16) << 8) | (resp[15] as u16);
    assert_eq!(got, expected);
}
