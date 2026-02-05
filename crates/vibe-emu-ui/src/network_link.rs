//! Network-based Game Boy link cable emulation using the BGB protocol.
//!
//! Implements the BGB 1.4 link protocol for compatibility with BGB and other
//! emulators. See: <https://bgb.bircd.org/bgblink.html>

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel as cb;
use log::{debug, info, trace, warn};
use vibe_emu_core::serial::{LinkPort, SerialTransferClock, serial_dot_cycles_per_bit};

const CMD_VERSION: u8 = 1;
const CMD_JOYPAD: u8 = 101;
const CMD_SYNC1: u8 = 104;
const CMD_SYNC2: u8 = 105;
const CMD_SYNC3: u8 = 106;
const CMD_STATUS: u8 = 108;
const CMD_WANTDISCONNECT: u8 = 109;

const STATUS_RUNNING: u8 = 0x01;
const STATUS_PAUSED: u8 = 0x02;
const STATUS_SUPPORT_RECONNECT: u8 = 0x04;

const LINK_TRANSFER_TIMEOUT: Duration = Duration::from_millis(1000);
const MASTER_RETRY_INTERVAL: Duration = Duration::from_millis(16);
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(300);
const CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(800);
const CONNECT_RETRY_WINDOW: Duration = Duration::from_secs(12);
const RX_PACKET_SIZE: usize = 8;
const DEFAULT_EXTERNAL_DOT_CYCLES_PER_BIT: u32 = 512;

#[derive(Clone, Copy, Default)]
struct BgbPacket {
    b1: u8,
    b2: u8,
    b3: u8,
    b4: u8,
    i1: u32,
}

impl BgbPacket {
    fn to_bytes(self) -> [u8; RX_PACKET_SIZE] {
        let mut buf = [0u8; RX_PACKET_SIZE];
        buf[0] = self.b1;
        buf[1] = self.b2;
        buf[2] = self.b3;
        buf[3] = self.b4;
        buf[4..8].copy_from_slice(&self.i1.to_le_bytes());
        buf
    }

    fn from_bytes(buf: &[u8; RX_PACKET_SIZE]) -> Self {
        Self {
            b1: buf[0],
            b2: buf[1],
            b3: buf[2],
            b4: buf[3],
            i1: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        }
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

    fn sync1(data: u8, control: u8, timestamp: u32) -> Self {
        Self {
            b1: CMD_SYNC1,
            b2: data,
            b3: sanitize_sync1_control(control),
            b4: 0,
            i1: timestamp,
        }
    }

    fn sync2(data: u8, response_to_sync1: bool) -> Self {
        Self {
            b1: CMD_SYNC2,
            b2: data,
            b3: 0x80,
            b4: u8::from(response_to_sync1),
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
            i1: timestamp & 0x7FFF_FFFF,
        }
    }

    fn want_disconnect() -> Self {
        Self {
            b1: CMD_WANTDISCONNECT,
            b2: 0,
            b3: 0,
            b4: 0,
            i1: 0,
        }
    }
}

fn sanitize_sync1_control(control: u8) -> u8 {
    // Only bits 0,1,2,7 are valid in SYNC1 control.
    0x81 | (control & 0x06)
}

fn bgb_sync1_control_value(high_speed: bool, double_speed: bool) -> u8 {
    0x80 | 0x01 | ((high_speed as u8) << 1) | ((double_speed as u8) << 2)
}

fn bgb_sync1_control_to_dot_cycles_per_bit(control: u8) -> u32 {
    let high_speed = (control & 0x02) != 0;
    let double_speed = (control & 0x04) != 0;
    serial_dot_cycles_per_bit(high_speed, double_speed)
}

pub enum LinkCommand {
    Listen { port: u16 },
    Connect { host: String, port: u16 },
    Disconnect,
    NotifyPause,
    NotifyResume,
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum LinkEvent {
    Listening { port: u16 },
    Connected,
    Disconnected,
    RemotePaused,
    RemoteResumed,
    SlaveTransferReady,
    Error(String),
}

/// Unified state for the link connection.
#[derive(Default)]
pub struct LinkState {
    pub stream: Option<TcpStream>,
    pub connected: bool,
    pub remote_paused: bool,

    // Master transfer state (we are sending with internal clock)
    pub master_outgoing: Option<u8>,
    pub master_response: Option<u8>,
    pub master_waiting: bool,
    pub master_control: u8,

    // Slave transfer state (we are responding with external clock)
    pub slave_incoming: Option<u8>,
    pub slave_outgoing: Option<u8>,
    pub slave_complete: bool,

    slave_queue: VecDeque<u8>,
    last_remote_sync3_timestamp: Option<u32>,
    remote_supports_reconnect: bool,
}

impl LinkState {
    fn queue_slave_incoming(&mut self, byte: u8) -> bool {
        // Keep slave transfers strictly lockstepped: do not accept a new byte
        // until the previous one has been consumed by the emulation thread.
        if !self.slave_queue.is_empty() {
            return false;
        }
        self.slave_queue.push_back(byte);
        self.slave_incoming = self.slave_queue.front().copied();
        self.slave_complete = !self.slave_queue.is_empty();
        true
    }

    fn pop_slave_incoming(&mut self) -> Option<u8> {
        let incoming = self.slave_queue.pop_front();
        self.slave_incoming = self.slave_queue.front().copied();
        self.slave_complete = !self.slave_queue.is_empty();
        if !self.slave_complete {
            self.slave_outgoing = None;
        }
        incoming
    }

    fn clear_queues(&mut self) {
        self.master_outgoing = None;
        self.master_response = None;
        self.master_waiting = false;
        self.master_control = 0;
        self.slave_incoming = None;
        self.slave_outgoing = None;
        self.slave_complete = false;
        self.slave_queue.clear();
        self.last_remote_sync3_timestamp = None;
    }
}

// Legacy compatibility types (preserved for main.rs integration)
pub struct ExternalClockPending {
    pending: AtomicBool,
    dot_cycles_per_bit: AtomicU32,
}

impl Default for ExternalClockPending {
    fn default() -> Self {
        Self {
            pending: AtomicBool::new(false),
            dot_cycles_per_bit: AtomicU32::new(DEFAULT_EXTERNAL_DOT_CYCLES_PER_BIT),
        }
    }
}

impl ExternalClockPending {
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    pub fn dot_cycles_per_bit(&self) -> u32 {
        self.dot_cycles_per_bit.load(Ordering::Acquire)
    }

    pub fn mark_pending(&self, dot_cycles_per_bit: u32) {
        let dot_cycles_per_bit = dot_cycles_per_bit.max(1);
        self.dot_cycles_per_bit
            .store(dot_cycles_per_bit, Ordering::Release);
        self.pending.store(true, Ordering::Release);
    }

    pub fn clear(&self) {
        self.pending.store(false, Ordering::Release);
        self.dot_cycles_per_bit
            .store(DEFAULT_EXTERNAL_DOT_CYCLES_PER_BIT, Ordering::Release);
    }
}

pub struct PendingTimestamp(pub AtomicU32);

impl Default for PendingTimestamp {
    fn default() -> Self {
        Self(AtomicU32::new(0))
    }
}

pub struct SlaveReadyState(pub AtomicU32);

impl Default for SlaveReadyState {
    fn default() -> Self {
        Self(AtomicU32::new(Self::NOT_READY))
    }
}

impl SlaveReadyState {
    pub const NOT_READY: u32 = u32::MAX;

    pub fn set_ready(&self, outgoing_byte: u8) {
        self.0.store(outgoing_byte as u32, Ordering::Release);
    }

    pub fn set_not_ready(&self) {
        self.0.store(Self::NOT_READY, Ordering::Release);
    }

    pub fn get_ready_byte(&self) -> Option<u8> {
        let val = self.0.load(Ordering::Acquire);
        if val == Self::NOT_READY {
            None
        } else {
            Some(val as u8)
        }
    }
}

// Legacy type alias for main.rs compatibility
pub type NetworkState = LinkState;

pub struct TransferCondvar(pub Mutex<()>, pub std::sync::Condvar);

impl Default for TransferCondvar {
    fn default() -> Self {
        Self(Mutex::new(()), std::sync::Condvar::new())
    }
}

pub struct NetworkLinkPort {
    state: Arc<Mutex<LinkState>>,
    shared_timestamp: Arc<AtomicU32>,
}

impl NetworkLinkPort {
    pub fn new(
        state: Arc<Mutex<LinkState>>,
        _external_clock_pending: Arc<ExternalClockPending>,
        _transfer_condvar: Arc<TransferCondvar>,
        timestamp: Arc<AtomicU32>,
        _slave_ready: Arc<SlaveReadyState>,
    ) -> Self {
        Self {
            state,
            shared_timestamp: timestamp,
        }
    }

    pub fn new_v2(
        state: Arc<Mutex<LinkState>>,
        timestamp: Arc<AtomicU32>,
        _doublespeed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            state,
            shared_timestamp: timestamp,
        }
    }

    fn _get_timestamp(&self) -> u32 {
        self.shared_timestamp.load(Ordering::Acquire)
    }
}

impl LinkPort for NetworkLinkPort {
    fn transfer(&mut self, byte: u8) -> u8 {
        let start = Instant::now();
        loop {
            if let Some(response) = self.try_transfer(byte) {
                return response;
            }

            if start.elapsed() > LINK_TRANSFER_TIMEOUT {
                warn!("Link: transfer 0x{:02X} timeout", byte);
                if let Ok(mut s) = self.state.lock() {
                    s.master_waiting = false;
                    s.master_outgoing = None;
                    s.master_response = None;
                }
                return 0xFF;
            }

            std::thread::sleep(Duration::from_micros(100));
        }
    }

    fn try_transfer(&mut self, byte: u8) -> Option<u8> {
        self.try_transfer_with_clock(byte, SerialTransferClock::default())
    }

    fn try_transfer_with_clock(&mut self, byte: u8, clock: SerialTransferClock) -> Option<u8> {
        let mut state = self.state.lock().ok()?;

        if let Some(response) = state.master_response.take() {
            state.master_waiting = false;
            state.master_outgoing = None;
            debug!(
                "Link: master transfer complete, sent 0x{:02X} received 0x{:02X}",
                byte, response
            );
            return Some(response);
        }

        if !state.connected {
            state.master_waiting = false;
            state.master_outgoing = None;
            return Some(0xFF);
        }

        if state.master_waiting {
            return None;
        }

        let control = bgb_sync1_control_value(clock.high_speed, clock.double_speed);
        state.master_control = sanitize_sync1_control(control);
        match state.master_outgoing {
            Some(existing) if existing != byte => {
                // Restart request with the latest byte.
                state.master_outgoing = Some(byte);
            }
            None => {
                state.master_outgoing = Some(byte);
            }
            _ => {}
        }

        None
    }

    fn try_external_transfer(&mut self, _byte: u8) -> Option<u8> {
        let mut state = self.state.lock().ok()?;
        if let Some(incoming) = state.pop_slave_incoming() {
            debug!(
                "Link: external transfer received queued byte 0x{:02X}",
                incoming
            );
            return Some(incoming);
        }

        if !state.connected {
            return Some(0xFF);
        }

        None
    }
}

pub fn spawn_network_thread(
    cmd_rx: mpsc::Receiver<LinkCommand>,
    event_tx: cb::Sender<LinkEvent>,
    external_clock_pending: Arc<ExternalClockPending>,
    _pending_timestamp: Arc<PendingTimestamp>,
    local_timestamp: Arc<AtomicU32>,
    slave_ready: Arc<SlaveReadyState>,
) -> (Arc<Mutex<LinkState>>, Arc<TransferCondvar>, Arc<AtomicU32>) {
    let state = Arc::new(Mutex::new(LinkState::default()));
    let state_clone = Arc::clone(&state);
    let condvar = Arc::new(TransferCondvar::default());
    let timestamp_clone = Arc::clone(&local_timestamp);

    thread::spawn(move || {
        network_thread_main(
            cmd_rx,
            event_tx,
            state_clone,
            external_clock_pending,
            slave_ready,
            local_timestamp,
        );
    });

    (state, condvar, timestamp_clone)
}

fn send_packet(stream: &mut TcpStream, packet: &BgbPacket) -> bool {
    let bytes = packet.to_bytes();
    stream.write_all(&bytes).is_ok() && stream.flush().is_ok()
}

fn read_blocking_packet(stream: &mut TcpStream) -> io::Result<BgbPacket> {
    let mut buf = [0u8; RX_PACKET_SIZE];
    stream.read_exact(&mut buf)?;
    Ok(BgbPacket::from_bytes(&buf))
}

fn resolve_socket_addr(host: &str, port: u16) -> io::Result<SocketAddr> {
    let endpoint = format!("{host}:{port}");
    if let Ok(addr) = endpoint.parse::<SocketAddr>() {
        return Ok(addr);
    }

    let mut addrs = endpoint.to_socket_addrs()?;
    addrs.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("No address resolved for {endpoint}"),
        )
    })
}

fn do_handshake_client(stream: &mut TcpStream) -> Option<(bool, bool)> {
    if let Err(e) = stream.set_nonblocking(false) {
        warn!("Link: failed to set blocking mode for client handshake: {e}");
        return None;
    }
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;

    if !send_packet(stream, &BgbPacket::version()) {
        warn!("Link: failed to send VERSION");
        return None;
    }

    let version = match read_blocking_packet(stream) {
        Ok(packet) => packet,
        Err(e) => {
            warn!("Link: failed to read VERSION during client handshake: {e}");
            return None;
        }
    };
    if version.b1 != CMD_VERSION || version.b2 != 1 || version.b3 != 4 || version.b4 != 0 {
        warn!(
            "Link: invalid VERSION from peer (cmd={} b2={} b3={} b4={})",
            version.b1, version.b2, version.b3, version.b4
        );
        return None;
    }

    let status = match read_blocking_packet(stream) {
        Ok(packet) => packet,
        Err(e) => {
            warn!("Link: failed to read STATUS during client handshake: {e}");
            return None;
        }
    };
    if status.b1 != CMD_STATUS {
        warn!("Link: expected STATUS during handshake, got {}", status.b1);
        return None;
    }
    let remote_paused = (status.b2 & STATUS_PAUSED) != 0;
    let remote_support_reconnect = (status.b2 & STATUS_SUPPORT_RECONNECT) != 0;

    if !send_packet(stream, &BgbPacket::status(true, false, true)) {
        warn!("Link: failed to send STATUS");
        return None;
    }

    stream.set_read_timeout(None).ok()?;
    info!("Link: BGB client handshake complete");
    Some((remote_paused, remote_support_reconnect))
}

fn do_handshake_server(stream: &mut TcpStream) -> Option<(bool, bool)> {
    if let Err(e) = stream.set_nonblocking(false) {
        warn!("Link: failed to set blocking mode for server handshake: {e}");
        return None;
    }
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;

    let version = match read_blocking_packet(stream) {
        Ok(packet) => packet,
        Err(e) => {
            warn!("Link: failed to read VERSION during server handshake: {e}");
            return None;
        }
    };
    if version.b1 != CMD_VERSION || version.b2 != 1 || version.b3 != 4 || version.b4 != 0 {
        warn!(
            "Link: invalid VERSION from peer (cmd={} b2={} b3={} b4={})",
            version.b1, version.b2, version.b3, version.b4
        );
        return None;
    }

    if !send_packet(stream, &BgbPacket::version()) {
        warn!("Link: failed to send VERSION");
        return None;
    }

    // Mirror BGB behavior seen in captures: send paused+running first,
    // then running.
    if !send_packet(stream, &BgbPacket::status(true, true, true)) {
        warn!("Link: failed to send initial paused STATUS");
        return None;
    }
    if !send_packet(stream, &BgbPacket::status(true, false, true)) {
        warn!("Link: failed to send running STATUS");
        return None;
    }

    let status = match read_blocking_packet(stream) {
        Ok(packet) => packet,
        Err(e) => {
            warn!("Link: failed to read STATUS during server handshake: {e}");
            return None;
        }
    };
    if status.b1 != CMD_STATUS {
        warn!("Link: expected STATUS during handshake, got {}", status.b1);
        return None;
    }
    let remote_paused = (status.b2 & STATUS_PAUSED) != 0;
    let remote_support_reconnect = (status.b2 & STATUS_SUPPORT_RECONNECT) != 0;

    stream.set_read_timeout(None).ok()?;
    info!("Link: BGB server handshake complete");
    Some((remote_paused, remote_support_reconnect))
}

fn queue_packet(tx_queue: &mut VecDeque<u8>, packet: BgbPacket) {
    tx_queue.extend(packet.to_bytes());
}

fn flush_send_queue(stream: &mut TcpStream, tx_queue: &mut VecDeque<u8>) -> std::io::Result<()> {
    let mut chunk = [0u8; 512];

    while !tx_queue.is_empty() {
        let chunk_len = tx_queue.len().min(chunk.len());
        for (dst, src) in chunk.iter_mut().zip(tx_queue.iter().take(chunk_len)) {
            *dst = *src;
        }

        match stream.write(&chunk[..chunk_len]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "socket closed while writing",
                ));
            }
            Ok(written) => {
                for _ in 0..written {
                    let _ = tx_queue.pop_front();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }

    Ok(())
}

enum ReadState {
    Alive,
    Disconnected,
}

struct PendingConnect {
    host: String,
    port: u16,
    next_attempt_at: Instant,
    deadline: Instant,
    attempts: u32,
    last_error: Option<String>,
}

impl PendingConnect {
    fn new(host: String, port: u16) -> Self {
        let now = Instant::now();
        Self {
            host,
            port,
            next_attempt_at: now,
            deadline: now + CONNECT_RETRY_WINDOW,
            attempts: 0,
            last_error: None,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn poll_stream_read(stream: &mut TcpStream, rx_buf: &mut Vec<u8>) -> std::io::Result<ReadState> {
    let mut temp = [0u8; 1024];
    loop {
        match stream.read(&mut temp) {
            Ok(0) => return Ok(ReadState::Disconnected),
            Ok(n) => {
                rx_buf.extend_from_slice(&temp[..n]);
                if n < temp.len() {
                    return Ok(ReadState::Alive);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(ReadState::Alive),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

fn drain_packets(rx_buf: &mut Vec<u8>) -> Vec<BgbPacket> {
    let mut packets = Vec::new();
    let mut offset = 0usize;

    while rx_buf.len().saturating_sub(offset) >= RX_PACKET_SIZE {
        let mut bytes = [0u8; RX_PACKET_SIZE];
        bytes.copy_from_slice(&rx_buf[offset..offset + RX_PACKET_SIZE]);
        packets.push(BgbPacket::from_bytes(&bytes));
        offset += RX_PACKET_SIZE;
    }

    if offset != 0 {
        rx_buf.drain(..offset);
    }

    packets
}

#[allow(clippy::too_many_arguments)]
fn network_thread_main(
    cmd_rx: mpsc::Receiver<LinkCommand>,
    event_tx: cb::Sender<LinkEvent>,
    state: Arc<Mutex<LinkState>>,
    external_clock_pending: Arc<ExternalClockPending>,
    slave_ready: Arc<SlaveReadyState>,
    timestamp: Arc<AtomicU32>,
) {
    let mut listener: Option<TcpListener> = None;
    let mut stream: Option<TcpStream> = None;
    let mut pending_connect: Option<PendingConnect> = None;
    let mut rx_buf: Vec<u8> = Vec::new();
    let mut tx_queue: VecDeque<u8> = VecDeque::new();
    let mut master_retry_not_before: Option<Instant> = None;

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                LinkCommand::Listen { port } => {
                    disconnect(&state, &external_clock_pending);
                    stream = None;
                    pending_connect = None;
                    rx_buf.clear();
                    tx_queue.clear();

                    match TcpListener::bind(format!("0.0.0.0:{port}")) {
                        Ok(l) => {
                            if let Err(e) = l.set_nonblocking(true) {
                                let _ = event_tx.try_send(LinkEvent::Error(format!(
                                    "Failed to set non-blocking listener: {e}"
                                )));
                                continue;
                            }
                            info!("Link: listening on port {port}");
                            let _ = event_tx.try_send(LinkEvent::Listening { port });
                            listener = Some(l);
                        }
                        Err(e) => {
                            let _ = event_tx.try_send(LinkEvent::Error(format!(
                                "Failed to bind listener: {e}"
                            )));
                        }
                    }
                }
                LinkCommand::Connect { host, port } => {
                    disconnect(&state, &external_clock_pending);
                    stream = None;
                    listener = None;
                    pending_connect = Some(PendingConnect::new(host, port));
                    rx_buf.clear();
                    tx_queue.clear();
                    if let Some(connect) = pending_connect.as_ref() {
                        info!("Link: connecting to {}", connect.endpoint());
                    }
                }
                LinkCommand::Disconnect => {
                    if let Some(conn) = stream.as_mut() {
                        let send_wantdisconnect = state
                            .lock()
                            .map(|s| s.remote_supports_reconnect)
                            .unwrap_or(false);
                        if send_wantdisconnect {
                            let _ = send_packet(conn, &BgbPacket::want_disconnect());
                        }
                    }

                    disconnect(&state, &external_clock_pending);
                    stream = None;
                    listener = None;
                    pending_connect = None;
                    rx_buf.clear();
                    tx_queue.clear();
                    let _ = event_tx.try_send(LinkEvent::Disconnected);
                }
                LinkCommand::NotifyPause => {
                    if stream.is_some() {
                        queue_packet(&mut tx_queue, BgbPacket::status(true, true, true));
                    }
                }
                LinkCommand::NotifyResume => {
                    if stream.is_some() {
                        queue_packet(&mut tx_queue, BgbPacket::status(true, false, true));
                    }
                }
                LinkCommand::Shutdown => {
                    disconnect(&state, &external_clock_pending);
                    return;
                }
            }
        }

        if stream.is_none()
            && let Some(l) = listener.as_ref()
        {
            match l.accept() {
                Ok((mut accepted, addr)) => {
                    info!("Link: accepted connection from {addr}");
                    let _ = accepted.set_nodelay(true);
                    if let Some((remote_paused, remote_support_reconnect)) =
                        do_handshake_server(&mut accepted)
                    {
                        let _ = accepted.set_nonblocking(true);
                        if let Ok(mut s) = state.lock() {
                            s.stream = accepted.try_clone().ok();
                            s.connected = true;
                            s.remote_paused = remote_paused;
                            s.remote_supports_reconnect = remote_support_reconnect;
                        }
                        stream = Some(accepted);
                        listener = None;
                        let _ = event_tx.try_send(LinkEvent::Connected);
                    } else {
                        warn!("Link: BGB handshake failed with {addr}");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    warn!("Link: listener accept error: {e}");
                }
            }
        }

        if stream.is_none() && pending_connect.is_some() {
            let mut clear_pending = false;

            if let Some(connect) = pending_connect.as_mut() {
                let now = Instant::now();
                if now >= connect.deadline {
                    let endpoint = connect.endpoint();
                    let detail = connect
                        .last_error
                        .clone()
                        .unwrap_or_else(|| "timed out while waiting for peer".to_string());
                    let attempts = connect.attempts.max(1);
                    let _ = event_tx.try_send(LinkEvent::Error(format!(
                        "Connection to {endpoint} failed after {attempts} attempt(s): {detail}"
                    )));
                    clear_pending = true;
                } else if now >= connect.next_attempt_at {
                    connect.attempts += 1;
                    let endpoint = connect.endpoint();
                    debug!(
                        "Link: connection attempt {} to {}",
                        connect.attempts, endpoint
                    );

                    match resolve_socket_addr(&connect.host, connect.port) {
                        Ok(addr) => {
                            match TcpStream::connect_timeout(&addr, CONNECT_ATTEMPT_TIMEOUT) {
                                Ok(mut conn) => {
                                    let _ = conn.set_nodelay(true);
                                    if let Some((remote_paused, remote_support_reconnect)) =
                                        do_handshake_client(&mut conn)
                                    {
                                        let _ = conn.set_nonblocking(true);
                                        if let Ok(mut s) = state.lock() {
                                            s.stream = conn.try_clone().ok();
                                            s.connected = true;
                                            s.remote_paused = remote_paused;
                                            s.remote_supports_reconnect = remote_support_reconnect;
                                        }
                                        info!(
                                            "Link: connected to {} after {} attempt(s)",
                                            endpoint, connect.attempts
                                        );
                                        stream = Some(conn);
                                        clear_pending = true;
                                        let _ = event_tx.try_send(LinkEvent::Connected);
                                    } else {
                                        connect.last_error =
                                            Some("BGB handshake failed".to_string());
                                        connect.next_attempt_at =
                                            Instant::now() + CONNECT_RETRY_INTERVAL;
                                    }
                                }
                                Err(e) => {
                                    connect.last_error = Some(e.to_string());
                                    connect.next_attempt_at =
                                        Instant::now() + CONNECT_RETRY_INTERVAL;
                                }
                            }
                        }
                        Err(e) => {
                            connect.last_error = Some(e.to_string());
                            connect.deadline = now;
                        }
                    }
                }
            }

            if clear_pending {
                pending_connect = None;
            }
        }

        let mut should_disconnect = false;
        if let Some(conn) = stream.as_mut() {
            let now = Instant::now();
            if let Ok(mut s) = state.lock()
                && let Some(outgoing) = s.master_outgoing
                && !s.master_waiting
                && master_retry_not_before.is_none_or(|next| now >= next)
            {
                s.master_waiting = true;
                let control = if s.master_control == 0 {
                    bgb_sync1_control_value(false, false)
                } else {
                    s.master_control
                };
                let ts = timestamp.load(Ordering::Acquire) & 0x7FFF_FFFF;
                queue_packet(&mut tx_queue, BgbPacket::sync1(outgoing, control, ts));
                debug!(
                    "Link: queued SYNC1 0x{:02X} ctrl=0x{:02X} ts={}",
                    outgoing, control, ts
                );
            }

            if let Err(e) = flush_send_queue(conn, &mut tx_queue) {
                warn!("Link: write error: {e}");
                should_disconnect = true;
            }

            if !should_disconnect {
                match poll_stream_read(conn, &mut rx_buf) {
                    Ok(ReadState::Alive) => {}
                    Ok(ReadState::Disconnected) => {
                        should_disconnect = true;
                    }
                    Err(e) => {
                        warn!("Link: read error: {e}");
                        should_disconnect = true;
                    }
                }
            }

            if !should_disconnect {
                for packet in drain_packets(&mut rx_buf) {
                    trace!(
                        "Link: received cmd={} b2=0x{:02X} b3=0x{:02X} b4=0x{:02X} ts={}",
                        packet.b1, packet.b2, packet.b3, packet.b4, packet.i1
                    );
                    handle_packet(
                        packet,
                        &state,
                        &external_clock_pending,
                        &event_tx,
                        &slave_ready,
                        &mut master_retry_not_before,
                        &mut tx_queue,
                    );
                }

                if let Err(e) = flush_send_queue(conn, &mut tx_queue) {
                    warn!("Link: write error after packet handling: {e}");
                    should_disconnect = true;
                }
            }
        }

        if should_disconnect {
            disconnect(&state, &external_clock_pending);
            stream = None;
            rx_buf.clear();
            tx_queue.clear();
            let _ = event_tx.try_send(LinkEvent::Disconnected);
        }

        thread::sleep(Duration::from_micros(100));
    }
}

fn handle_packet(
    packet: BgbPacket,
    state: &Arc<Mutex<LinkState>>,
    external_clock_pending: &Arc<ExternalClockPending>,
    event_tx: &cb::Sender<LinkEvent>,
    slave_ready: &Arc<SlaveReadyState>,
    master_retry_not_before: &mut Option<Instant>,
    tx_queue: &mut VecDeque<u8>,
) {
    match packet.b1 {
        CMD_VERSION => {
            trace!("Link: received late VERSION packet");
        }
        CMD_JOYPAD => {
            // Joypad remote control is optional; ignore for now.
        }
        CMD_STATUS => {
            let paused = (packet.b2 & STATUS_PAUSED) != 0;
            let support_reconnect = (packet.b2 & STATUS_SUPPORT_RECONNECT) != 0;
            let mut emit_event = None;
            if let Ok(mut s) = state.lock() {
                if s.remote_paused != paused {
                    s.remote_paused = paused;
                    emit_event = Some(if paused {
                        LinkEvent::RemotePaused
                    } else {
                        LinkEvent::RemoteResumed
                    });
                }
                s.remote_supports_reconnect = support_reconnect;
            }
            if let Some(event) = emit_event {
                let _ = event_tx.try_send(event);
            }
        }
        CMD_SYNC1 => {
            let incoming = packet.b2;
            let dot_cycles_per_bit = bgb_sync1_control_to_dot_cycles_per_bit(packet.b3);
            let mut response = None;
            let mut slave_transfer_ready = false;
            let slave_busy = external_clock_pending.is_pending();

            if let Ok(mut s) = state.lock() {
                if s.master_waiting {
                    // Collision: both sides attempted active transfer. Resolve by
                    // using their SYNC1 byte as our response, and reply with our
                    // pending outgoing byte as SYNC2.
                    let outgoing = s.master_outgoing.take().unwrap_or(0xFF);
                    s.master_waiting = false;
                    s.master_response = Some(incoming);
                    *master_retry_not_before = None;
                    response = Some(BgbPacket::sync2(outgoing, true));
                } else if slave_busy || s.slave_complete {
                    // Keep transfers in lockstep: while a previous external
                    // transfer is still pending/in-flight, ask the master to
                    // retry later instead of queueing ahead.
                    response = Some(BgbPacket::sync3_ack());
                } else if let Some(outgoing) = slave_ready.get_ready_byte() {
                    if s.queue_slave_incoming(incoming) {
                        s.slave_outgoing = Some(outgoing);
                        response = Some(BgbPacket::sync2(outgoing, true));
                        slave_transfer_ready = true;
                    } else {
                        response = Some(BgbPacket::sync3_ack());
                    }
                } else {
                    // Passive side not ready yet; ask master to retry.
                    response = Some(BgbPacket::sync3_ack());
                }
            }

            if let Some(packet) = response {
                queue_packet(tx_queue, packet);
            }
            if slave_transfer_ready {
                external_clock_pending.mark_pending(dot_cycles_per_bit);
                let _ = event_tx.try_send(LinkEvent::SlaveTransferReady);
            }
        }
        CMD_SYNC2 => {
            let data = packet.b2;
            if let Ok(mut s) = state.lock() {
                if s.master_waiting {
                    s.master_response = Some(data);
                    s.master_outgoing = None;
                    s.master_waiting = false;
                    *master_retry_not_before = None;
                } else {
                    // SYNC2 is only valid as a response to our in-flight SYNC1.
                    // Treat anything else as stale/stray traffic and ignore it.
                    trace!("Link: ignoring unexpected SYNC2 while not waiting");
                }
            }
        }
        CMD_SYNC3 => {
            if packet.b2 == 1 {
                // Peer is not in passive transfer mode. Retry the pending master
                // transfer later with BGB-like pacing instead of busy-looping.
                if let Ok(mut s) = state.lock() {
                    if s.master_outgoing.is_some() {
                        *master_retry_not_before = Some(Instant::now() + MASTER_RETRY_INTERVAL);
                    }
                    s.master_waiting = false;
                }
            } else {
                trace!("Link: received timestamp SYNC3 ts={}", packet.i1);
                // BGB expects timestamp sync packets to be acknowledged with
                // a timestamp sync packet in return. De-duplicate by remote
                // timestamp value to avoid ping-pong loops on echoed packets.
                let should_echo = if let Ok(mut s) = state.lock() {
                    let is_new = s.last_remote_sync3_timestamp != Some(packet.i1);
                    s.last_remote_sync3_timestamp = Some(packet.i1);
                    is_new
                } else {
                    true
                };
                if should_echo {
                    queue_packet(tx_queue, BgbPacket::sync3_timestamp(packet.i1));
                }
            }
        }
        CMD_WANTDISCONNECT => {
            trace!("Link: peer sent WANTDISCONNECT");
        }
        _ => {
            trace!("Link: ignoring unknown command {}", packet.b1);
        }
    }
}

fn disconnect(state: &Arc<Mutex<LinkState>>, external_clock_pending: &Arc<ExternalClockPending>) {
    external_clock_pending.clear();
    if let Ok(mut s) = state.lock() {
        s.stream = None;
        s.connected = false;
        s.remote_paused = false;
        s.remote_supports_reconnect = false;
        s.clear_queues();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pop_packet(tx_queue: &mut VecDeque<u8>) -> BgbPacket {
        let mut bytes = [0u8; RX_PACKET_SIZE];
        for byte in &mut bytes {
            *byte = tx_queue.pop_front().expect("missing queued packet byte");
        }
        BgbPacket::from_bytes(&bytes)
    }

    #[test]
    fn sync1_packet_uses_bgb_control_value() {
        let packet = BgbPacket::sync1(0x42, 0x85, 0x1234_5678);
        assert_eq!(packet.b1, CMD_SYNC1);
        assert_eq!(packet.b2, 0x42);
        assert_eq!(packet.b3, 0x85);
        assert_eq!(packet.b4, 0);
        assert_eq!(packet.i1, 0x1234_5678);
    }

    #[test]
    fn sync1_control_bits_map_to_expected_clock_rates() {
        assert_eq!(bgb_sync1_control_to_dot_cycles_per_bit(0x81), 512);
        assert_eq!(bgb_sync1_control_to_dot_cycles_per_bit(0x85), 256);
        assert_eq!(bgb_sync1_control_to_dot_cycles_per_bit(0x83), 16);
        assert_eq!(bgb_sync1_control_to_dot_cycles_per_bit(0x87), 8);
    }

    #[test]
    fn sync1_ready_uses_control_bits_for_external_clock_pacing() {
        let state = Arc::new(Mutex::new(LinkState::default()));
        let pending = Arc::new(ExternalClockPending::default());
        let slave_ready = Arc::new(SlaveReadyState::default());
        slave_ready.set_ready(0x34);
        let (event_tx, _event_rx) = cb::bounded(4);
        let mut tx_queue = VecDeque::new();
        let mut master_retry_not_before = None;

        handle_packet(
            BgbPacket::sync1(0x12, 0x87, 99),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        assert!(pending.is_pending());
        assert_eq!(pending.dot_cycles_per_bit(), 8);
    }

    #[test]
    fn sync1_not_ready_responds_with_sync3_ack() {
        let state = Arc::new(Mutex::new(LinkState::default()));
        let pending = Arc::new(ExternalClockPending::default());
        let slave_ready = Arc::new(SlaveReadyState::default());
        let (event_tx, _event_rx) = cb::bounded(4);
        let mut tx_queue = VecDeque::new();
        let mut master_retry_not_before = None;

        handle_packet(
            BgbPacket::sync1(0x12, 0x85, 99),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        let packet = pop_packet(&mut tx_queue);
        assert_eq!(packet.b1, CMD_SYNC3);
        assert_eq!(packet.b2, 1);
        assert_eq!(packet.i1, 0);
        assert!(
            !pending.is_pending(),
            "external clock should not be armed when slave is not ready"
        );
    }

    #[test]
    fn sync1_ready_responds_sync2_and_queues_incoming() {
        let state = Arc::new(Mutex::new(LinkState::default()));
        let pending = Arc::new(ExternalClockPending::default());
        let slave_ready = Arc::new(SlaveReadyState::default());
        slave_ready.set_ready(0x34);
        let (event_tx, _event_rx) = cb::bounded(4);
        let mut tx_queue = VecDeque::new();
        let mut master_retry_not_before = None;

        handle_packet(
            BgbPacket::sync1(0x12, 0x85, 99),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        let packet = pop_packet(&mut tx_queue);
        assert_eq!(packet.b1, CMD_SYNC2);
        assert_eq!(packet.b2, 0x34);
        assert_eq!(packet.b3, 0x80);
        assert_eq!(packet.b4, 1);

        let lock = state.lock().expect("state lock");
        assert_eq!(lock.slave_incoming, Some(0x12));
        assert!(lock.slave_complete);
        assert!(pending.is_pending());
        assert_eq!(pending.dot_cycles_per_bit(), 256);
    }

    #[test]
    fn sync3_ack_rearms_master_with_retry_delay() {
        let state = Arc::new(Mutex::new(LinkState::default()));
        {
            let mut lock = state.lock().expect("state lock");
            lock.master_outgoing = Some(0x77);
            lock.master_waiting = true;
        }

        let pending = Arc::new(ExternalClockPending::default());
        let slave_ready = Arc::new(SlaveReadyState::default());
        let (event_tx, _event_rx) = cb::bounded(4);
        let mut tx_queue = VecDeque::new();
        let mut master_retry_not_before = None;

        handle_packet(
            BgbPacket::sync3_ack(),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        let lock = state.lock().expect("state lock");
        assert_eq!(lock.master_response, None);
        assert_eq!(lock.master_outgoing, Some(0x77));
        assert!(!lock.master_waiting);
        assert!(master_retry_not_before.is_some());
    }

    #[test]
    fn sync3_timestamp_echoes_once_per_unique_remote_timestamp() {
        let state = Arc::new(Mutex::new(LinkState::default()));
        let pending = Arc::new(ExternalClockPending::default());
        let slave_ready = Arc::new(SlaveReadyState::default());
        let (event_tx, _event_rx) = cb::bounded(4);
        let mut tx_queue = VecDeque::new();
        let mut master_retry_not_before = None;

        handle_packet(
            BgbPacket::sync3_timestamp(0x0012_3456),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        let first = pop_packet(&mut tx_queue);
        assert_eq!(first.b1, CMD_SYNC3);
        assert_eq!(first.b2, 0);
        assert_eq!(first.i1, 0x0012_3456);

        // Duplicate timestamp should not be echoed again.
        handle_packet(
            BgbPacket::sync3_timestamp(0x0012_3456),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );
        assert!(tx_queue.is_empty());

        // A newer timestamp should be echoed.
        handle_packet(
            BgbPacket::sync3_timestamp(0x0012_3856),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );
        let second = pop_packet(&mut tx_queue);
        assert_eq!(second.b1, CMD_SYNC3);
        assert_eq!(second.b2, 0);
        assert_eq!(second.i1, 0x0012_3856);
    }

    #[test]
    fn sync2_waiting_master_completes_transfer() {
        let state = Arc::new(Mutex::new(LinkState::default()));
        {
            let mut lock = state.lock().expect("state lock");
            lock.master_outgoing = Some(0x11);
            lock.master_waiting = true;
        }

        let pending = Arc::new(ExternalClockPending::default());
        let slave_ready = Arc::new(SlaveReadyState::default());
        let (event_tx, _event_rx) = cb::bounded(4);
        let mut tx_queue = VecDeque::new();
        let mut master_retry_not_before = None;

        handle_packet(
            BgbPacket::sync2(0x22, true),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        let lock = state.lock().expect("state lock");
        assert_eq!(lock.master_response, Some(0x22));
        assert_eq!(lock.master_outgoing, None);
        assert!(!lock.master_waiting);
    }

    #[test]
    fn sync2_not_waiting_master_is_ignored() {
        let state = Arc::new(Mutex::new(LinkState::default()));
        {
            let mut lock = state.lock().expect("state lock");
            lock.master_outgoing = Some(0x11);
            lock.master_waiting = false;
        }

        let pending = Arc::new(ExternalClockPending::default());
        let slave_ready = Arc::new(SlaveReadyState::default());
        let (event_tx, event_rx) = cb::bounded(4);
        let mut tx_queue = VecDeque::new();
        let mut master_retry_not_before = None;

        handle_packet(
            BgbPacket::sync2(0x22, true),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        let lock = state.lock().expect("state lock");
        assert_eq!(lock.master_response, None);
        assert_eq!(lock.master_outgoing, Some(0x11));
        assert!(!lock.master_waiting);
        assert!(!pending.is_pending());
        assert!(tx_queue.is_empty());
        assert!(event_rx.try_recv().is_err());
    }

    #[test]
    fn collision_followed_by_late_sync2_keeps_collision_response() {
        let state = Arc::new(Mutex::new(LinkState::default()));
        {
            let mut lock = state.lock().expect("state lock");
            lock.master_outgoing = Some(0xA1);
            lock.master_waiting = true;
        }

        let pending = Arc::new(ExternalClockPending::default());
        let slave_ready = Arc::new(SlaveReadyState::default());
        let (event_tx, _event_rx) = cb::bounded(4);
        let mut tx_queue = VecDeque::new();
        let mut master_retry_not_before = None;

        // First packet collides with our in-flight master transfer.
        handle_packet(
            BgbPacket::sync1(0xB2, 0x85, 123),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        let packet = pop_packet(&mut tx_queue);
        assert_eq!(packet.b1, CMD_SYNC2);
        assert_eq!(packet.b2, 0xA1);

        // A late SYNC2 for the abandoned transfer must be ignored.
        handle_packet(
            BgbPacket::sync2(0xC3, true),
            &state,
            &pending,
            &event_tx,
            &slave_ready,
            &mut master_retry_not_before,
            &mut tx_queue,
        );

        let lock = state.lock().expect("state lock");
        assert_eq!(lock.master_response, Some(0xB2));
        assert_eq!(lock.master_outgoing, None);
        assert!(!lock.master_waiting);
    }

    #[test]
    fn server_handshake_succeeds_when_accepted_socket_is_nonblocking() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("listener address");
        let (result_tx, result_rx) = mpsc::channel();

        let server = std::thread::spawn(move || {
            let (mut accepted, _) = listener.accept().expect("accept client");
            accepted
                .set_nonblocking(true)
                .expect("set accepted socket non-blocking");
            let ok = do_handshake_server(&mut accepted).is_some();
            result_tx.send(ok).expect("send server result");
        });

        let mut client = TcpStream::connect(addr).expect("connect client");
        std::thread::sleep(Duration::from_millis(20));
        let client_ok = do_handshake_client(&mut client).is_some();
        let server_ok = result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("receive server result");
        server.join().expect("join server thread");

        assert!(client_ok, "client handshake should complete");
        assert!(server_ok, "server handshake should complete");
    }

    #[test]
    fn resolve_socket_addr_accepts_numeric_host() {
        let addr = resolve_socket_addr("127.0.0.1", 5000).expect("numeric host should parse");
        assert_eq!(addr.port(), 5000);
        assert!(addr.ip().is_ipv4());
    }
}
