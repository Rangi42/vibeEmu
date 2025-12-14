use crate::hardware::DmgRevision;

pub trait LinkPort: Send {
    /// Transfer a byte over the link. Returns the byte received from the
    /// partner. Implementations may perform the transfer immediately.
    fn transfer(&mut self, byte: u8) -> u8;
}

/// A stub link port used when no cable is attached.
/// By default it emulates a "line dead" scenario where incoming bits are all 1,
/// so any transfer receives 0xFF. When `loopback` is true the sent byte is
/// echoed back instead.
#[derive(Default)]
pub struct NullLinkPort {
    loopback: bool,
}

impl NullLinkPort {
    pub fn new(loopback: bool) -> Self {
        Self { loopback }
    }
}

impl LinkPort for NullLinkPort {
    fn transfer(&mut self, byte: u8) -> u8 {
        if self.loopback { byte } else { 0xFF }
    }
}

/// Represents the Game Boy serial registers.
/// This struct handles SB/SC behavior and raises the serial interrupt
/// when a transfer completes.
pub struct Serial {
    sb: u8,
    sc: u8,
    pub(crate) out_buf: Vec<u8>,
    port: Box<dyn LinkPort + Send>,
    transfer: Option<TransferState>,
    cgb_mode: bool,
    dmg_revision: DmgRevision,
}

struct TransferState {
    remaining_bits: u8,
    outgoing: u8,
    incoming: Option<u8>,
    pending_in: u8,
    internal_clock: bool,
    fast_clock: bool,
}

impl TransferState {
    fn new(outgoing: u8, internal_clock: bool, fast_clock: bool) -> Self {
        Self {
            remaining_bits: 8,
            outgoing,
            incoming: None,
            pending_in: 0,
            internal_clock,
            fast_clock,
        }
    }

    fn latch_incoming(&mut self, incoming: u8) {
        if self.incoming.is_some() {
            return;
        }
        self.incoming = Some(incoming);
        self.pending_in = incoming;
    }

    fn shift(&mut self, sb: &mut u8) -> bool {
        if self.remaining_bits == 0 {
            return true;
        }

        let incoming_bit = (self.pending_in & 0x80) != 0;
        self.pending_in <<= 1;
        *sb = (*sb << 1) | incoming_bit as u8;
        self.remaining_bits -= 1;
        self.remaining_bits == 0
    }
}

impl Serial {
    pub fn new(cgb: bool, dmg_revision: DmgRevision) -> Self {
        Self {
            sb: 0,
            sc: if cgb { 0x7F } else { 0x7E },
            out_buf: Vec::new(),
            port: Box::new(NullLinkPort::default()),
            transfer: None,
            cgb_mode: cgb,
            dmg_revision,
        }
    }

    pub fn connect(&mut self, port: Box<dyn LinkPort + Send>) {
        self.port = port;
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF01 => self.sb,
            0xFF02 => {
                if self.cgb_mode {
                    self.sc
                } else {
                    self.sc | 0x7E
                }
            }
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0xFF01 => self.sb = val,
            0xFF02 => {
                if let Some(state) = self.transfer.as_mut() {
                    // Mid-transfer SC writes:
                    // - If bit7 is cleared, cancel the transfer.
                    // - If bit7 remains set, treat the write as a (re)start
                    //   request: restart the transfer using the current SB
                    //   value, and apply clock mode bits.
                    if val & 0x80 == 0 {
                        self.sc = val;
                        self.transfer = None;
                        return;
                    }

                    self.sc = val;
                    state.remaining_bits = 8;
                    state.outgoing = self.sb;
                    state.incoming = None;
                    state.pending_in = 0;
                    state.internal_clock = (val & 0x01) != 0;
                    state.fast_clock = (val & 0x02) != 0;
                    return;
                }

                self.sc = val;
                if val & 0x80 != 0 {
                    let internal_clock = val & 0x01 != 0;
                    let fast_clock = val & 0x02 != 0;
                    self.transfer = Some(TransferState::new(self.sb, internal_clock, fast_clock));
                    // When using an external clock the transfer will only
                    // complete if the link partner supplies the necessary
                    // pulses, so we simply keep SC bit 7 asserted until the
                    // clock edges arrive.
                }
            }
            _ => {}
        }
    }

    /// Deliver external clock pulses to the serial unit.
    ///
    /// Each pulse clocks one bit. This is only meaningful when the transfer
    /// is in external clock mode (SC bit0 = 0).
    pub fn external_clock_pulse(&mut self, count: u8, if_reg: &mut u8) {
        if self.transfer.is_none() {
            return;
        }

        let mut complete = false;
        {
            let state = self.transfer.as_mut().unwrap();
            if state.internal_clock {
                return;
            }

            if state.incoming.is_none() {
                let incoming = self.port.transfer(state.outgoing);
                state.latch_incoming(incoming);
            }

            for _ in 0..count {
                if state.shift(&mut self.sb) {
                    complete = true;
                    break;
                }
            }
        }

        if complete {
            let state = self.transfer.take().unwrap();
            let incoming = state.incoming.unwrap_or(0xFF);
            self.sb = incoming;
            self.out_buf.push(state.outgoing);
            self.sc &= 0x7F;
            *if_reg |= 0x08;
        }
    }

    pub fn step(&mut self, prev_div: u16, curr_div: u16, double_speed: bool, if_reg: &mut u8) {
        if self.transfer.is_none() {
            return;
        }

        let (clock_bit, phase) = if let Some(state) = self.transfer.as_ref() {
            (
                clock_bit_index(self.cgb_mode, double_speed, state.fast_clock),
                self.phase_adjust(double_speed, state.fast_clock),
            )
        } else {
            return;
        };

        let mut complete = false;
        {
            let state = self.transfer.as_mut().unwrap();
            let mut div = prev_div;
            let steps = curr_div.wrapping_sub(prev_div);
            let mut prev_clock = ((div.wrapping_sub(phase) >> clock_bit) & 1) != 0;

            if state.internal_clock && state.incoming.is_none() {
                // Defer the link exchange until the transfer actually clocks,
                // so external-clock transfers don't consume bytes when no
                // clock edges arrive.
                //
                // For internal clock mode, latch the partner byte before the
                // first shifted bit.
                let incoming = self.port.transfer(state.outgoing);
                state.latch_incoming(incoming);
            }

            for _ in 0..steps {
                div = div.wrapping_add(1);
                let clock = ((div.wrapping_sub(phase) >> clock_bit) & 1) != 0;
                if state.internal_clock && prev_clock && !clock && state.shift(&mut self.sb) {
                    complete = true;
                    break;
                }
                prev_clock = clock;
            }
        }

        if complete {
            let state = self.transfer.take().unwrap();
            let incoming = state.incoming.unwrap_or(0xFF);
            self.sb = incoming;
            self.out_buf.push(state.outgoing);
            self.sc &= 0x7F;
            *if_reg |= 0x08;
        }
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        let out = self.out_buf.clone();
        self.out_buf.clear();
        out
    }

    pub fn peek_output(&self) -> &[u8] {
        &self.out_buf
    }

    fn phase_adjust(&self, double_speed: bool, fast_clock: bool) -> u16 {
        if self.cgb_mode {
            return 0;
        }

        if double_speed || fast_clock {
            return 0;
        }

        match self.dmg_revision {
            DmgRevision::RevA | DmgRevision::RevB | DmgRevision::RevC => 0,
            DmgRevision::Rev0 => 0,
        }
    }
}

fn clock_bit_index(cgb_mode: bool, double_speed: bool, fast_clock: bool) -> u32 {
    if !cgb_mode {
        if double_speed { 7 } else { 8 }
    } else {
        match (fast_clock, double_speed) {
            (false, false) => 8,
            (false, true) => 7,
            (true, false) => 3,
            (true, true) => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkPort, Serial};
    use crate::hardware::DmgRevision;

    struct FixedInLinkPort {
        ret: u8,
        calls: usize,
        last_out: Option<u8>,
    }

    impl FixedInLinkPort {
        fn new(ret: u8) -> Self {
            Self {
                ret,
                calls: 0,
                last_out: None,
            }
        }
    }

    impl LinkPort for FixedInLinkPort {
        fn transfer(&mut self, byte: u8) -> u8 {
            self.calls += 1;
            self.last_out = Some(byte);
            self.ret
        }
    }

    #[test]
    fn sc_write_during_active_transfer_does_not_cancel() {
        let mut serial = Serial::new(false, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        serial.write(0xFF02, 0x80 | 0x01);

        // Attempt to clear SC mid-transfer. The transfer should be cancelled
        // and must not complete later.
        serial.write(0xFF02, 0x00);
        assert_eq!(serial.read(0xFF02) & 0x80, 0);

        let mut if_reg = 0u8;
        serial.step(0, 4096, false, &mut if_reg);
        assert_eq!(serial.read(0xFF02) & 0x80, 0);
        assert_eq!(if_reg & 0x08, 0);
    }

    #[test]
    fn internal_clock_transfer_completes_and_requests_irq() {
        let mut serial = Serial::new(false, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        serial.write(0xFF02, 0x80 | 0x01);

        let mut if_reg = 0u8;
        // For DMG internal clock, we clock on DIV bit 8 falling edges.
        // 8 bits = 8 falling edges = 8 * 512 DIV increments.
        serial.step(0, 4096, false, &mut if_reg);

        assert_eq!(serial.read(0xFF02) & 0x80, 0);
        assert_ne!(if_reg & 0x08, 0);
        assert_eq!(serial.read(0xFF01), 0x34);
    }

    #[test]
    fn external_clock_stalls_without_pulses() {
        let mut serial = Serial::new(false, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        serial.write(0xFF02, 0x80);

        let mut if_reg = 0u8;
        serial.step(0, 60000, false, &mut if_reg);

        assert_ne!(serial.read(0xFF02) & 0x80, 0);
        assert_eq!(if_reg & 0x08, 0);
    }

    #[test]
    fn external_clock_completes_with_pulses() {
        let mut serial = Serial::new(false, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        serial.write(0xFF02, 0x80);

        let mut if_reg = 0u8;
        serial.external_clock_pulse(7, &mut if_reg);
        assert_ne!(serial.read(0xFF02) & 0x80, 0);
        assert_eq!(if_reg & 0x08, 0);

        serial.external_clock_pulse(1, &mut if_reg);
        assert_eq!(serial.read(0xFF02) & 0x80, 0);
        assert_ne!(if_reg & 0x08, 0);
        assert_eq!(serial.read(0xFF01), 0x34);
    }

    #[test]
    fn internal_clock_irq_only_on_final_bit_dmg() {
        let mut serial = Serial::new(false, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        serial.write(0xFF02, 0x80 | 0x01);

        let mut if_reg = 0u8;
        // 7 bits worth of falling edges: 7 * 512 DIV increments.
        serial.step(0, 3584, false, &mut if_reg);
        assert_ne!(serial.read(0xFF02) & 0x80, 0);
        assert_eq!(if_reg & 0x08, 0);

        // Final bit.
        serial.step(3584, 4096, false, &mut if_reg);
        assert_eq!(serial.read(0xFF02) & 0x80, 0);
        assert_ne!(if_reg & 0x08, 0);
        assert_eq!(serial.read(0xFF01), 0x34);
    }

    #[test]
    fn internal_clock_rate_cgb_normal_speed() {
        let mut serial = Serial::new(true, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        // CGB internal, normal clock.
        serial.write(0xFF02, 0x80 | 0x01);

        let mut if_reg = 0u8;
        // CGB normal clock uses DIV bit 8: 8 bits -> 8 * 512 increments.
        serial.step(0, 4095, false, &mut if_reg);
        assert_ne!(serial.read(0xFF02) & 0x80, 0);
        assert_eq!(if_reg & 0x08, 0);

        serial.step(4095, 4096, false, &mut if_reg);
        assert_eq!(serial.read(0xFF02) & 0x80, 0);
        assert_ne!(if_reg & 0x08, 0);
        assert_eq!(serial.read(0xFF01), 0x34);
    }

    #[test]
    fn internal_clock_rate_cgb_double_speed() {
        let mut serial = Serial::new(true, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        // CGB internal, normal clock (fast bit clear) in double-speed mode.
        serial.write(0xFF02, 0x80 | 0x01);

        let mut if_reg = 0u8;
        // In double-speed, CGB normal clock uses DIV bit 7: 8 bits -> 8 * 256.
        serial.step(0, 2047, true, &mut if_reg);
        assert_ne!(serial.read(0xFF02) & 0x80, 0);
        assert_eq!(if_reg & 0x08, 0);

        serial.step(2047, 2048, true, &mut if_reg);
        assert_eq!(serial.read(0xFF02) & 0x80, 0);
        assert_ne!(if_reg & 0x08, 0);
        assert_eq!(serial.read(0xFF01), 0x34);
    }

    #[test]
    fn internal_clock_rate_cgb_fast_clock() {
        let mut serial = Serial::new(true, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        // CGB internal + fast clock (SC bit1).
        serial.write(0xFF02, 0x80 | 0x01 | 0x02);

        let mut if_reg = 0u8;
        // Fast clock uses DIV bit 3 in normal speed: falling edges every 16.
        // 8 bits -> 8 * 16 = 128 increments.
        serial.step(0, 127, false, &mut if_reg);
        assert_ne!(serial.read(0xFF02) & 0x80, 0);
        assert_eq!(if_reg & 0x08, 0);

        serial.step(127, 128, false, &mut if_reg);
        assert_eq!(serial.read(0xFF02) & 0x80, 0);
        assert_ne!(if_reg & 0x08, 0);
        assert_eq!(serial.read(0xFF01), 0x34);
    }

    #[test]
    fn open_bus_no_partner_internal_clock_receives_ff() {
        let mut serial = Serial::new(false, DmgRevision::default());
        // No connect(): uses NullLinkPort which shifts in 1s.
        serial.write(0xFF01, 0x12);
        serial.write(0xFF02, 0x80 | 0x01);

        let mut if_reg = 0u8;
        serial.step(0, 4096, false, &mut if_reg);

        assert_ne!(if_reg & 0x08, 0);
        assert_eq!(serial.read(0xFF01), 0xFF);
    }

    #[test]
    fn open_bus_no_partner_external_clock_receives_ff() {
        let mut serial = Serial::new(false, DmgRevision::default());
        // No connect(): uses NullLinkPort which shifts in 1s.
        serial.write(0xFF01, 0x12);
        serial.write(0xFF02, 0x80);

        let mut if_reg = 0u8;
        serial.external_clock_pulse(8, &mut if_reg);

        assert_ne!(if_reg & 0x08, 0);
        assert_eq!(serial.read(0xFF01), 0xFF);
    }

    #[test]
    fn sc_write_with_bit7_restarts_transfer_using_current_sb() {
        let mut serial = Serial::new(false, DmgRevision::default());
        serial.connect(Box::new(FixedInLinkPort::new(0x34)));

        serial.write(0xFF01, 0x12);
        serial.write(0xFF02, 0x80 | 0x01);

        let mut if_reg = 0u8;
        // Advance one bit.
        serial.step(0, 512, false, &mut if_reg);
        assert_eq!(if_reg & 0x08, 0);

        // Update SB and restart.
        serial.write(0xFF01, 0x55);
        serial.write(0xFF02, 0x80 | 0x01);

        // Complete transfer.
        serial.step(512, 512 + 4096, false, &mut if_reg);
        assert_ne!(if_reg & 0x08, 0);
        // Output buffer records the outgoing byte that was actually used.
        assert_eq!(serial.peek_output().last().copied(), Some(0x55));
    }
}
