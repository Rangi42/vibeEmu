use crate::{
    apu::Apu,
    cartridge::Cartridge,
    hardware::{CgbRevision, DmgRevision},
    input::Input,
    ppu::Ppu,
    serial::Serial,
    timer::Timer,
};

const WRAM_BANK_SIZE: usize = 0x1000;

/// Transfer mode for CGB DMA operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmaMode {
    /// General DMA (immediate)
    Gdma,
    /// HBlank DMA
    Hdma,
}

#[derive(Debug)]
struct HdmaState {
    /// 16-bit source pointer (upper 12 bits writable)
    src: u16,
    /// Destination in VRAM (0x8000 | (dst & 0x1FF0))
    dst: u16,
    /// Remaining 0x10-byte blocks (0-7F means 1-128 blocks)
    blocks: u8,
    /// Current DMA mode
    mode: DmaMode,
    /// HDMA active flag
    active: bool,
    /// Whether the previous transfer was explicitly cancelled (FF55 <- 0)
    cancelled: bool,
}

pub struct Mmu {
    pub wram: [[u8; WRAM_BANK_SIZE]; 8],
    pub wram_bank: usize,
    pub hram: [u8; 0x7F],
    pub cart: Option<Cartridge>,
    pub boot_rom: Option<Vec<u8>>,
    pub boot_mapped: bool,
    pub if_reg: u8,
    pub ie_reg: u8,
    pub serial: Serial,
    pub ppu: Ppu,
    pub apu: Apu,
    pub timer: Timer,
    pub input: Input,
    hdma: HdmaState,
    pub key1: u8,
    pub rp: u8,
    pub dma_cycles: u16,
    dma_source: u16,
    pending_dma: Option<u16>,
    pending_delay: u16,
    /// Remaining stall cycles after a General DMA
    gdma_cycles: u32,
    cgb_mode: bool,
    cgb_revision: CgbRevision,
    dmg_revision: DmgRevision,
    /// Last CPU program counter observed when the CPU performed a memory
    /// operation. This is set by the `Cpu` helpers before calling into the
    /// MMU so logs can attribute blocked accesses to the originating PC.
    pub last_cpu_pc: Option<u16>,
}

impl Mmu {
    pub fn new_with_mode(cgb: bool) -> Self {
        Self::new_with_revisions(cgb, DmgRevision::default(), CgbRevision::default())
    }

    pub fn new_with_config(cgb: bool, revision: CgbRevision) -> Self {
        Self::new_with_revisions(cgb, DmgRevision::default(), revision)
    }

    pub fn new_with_revisions(
        cgb: bool,
        dmg_revision: DmgRevision,
        cgb_revision: CgbRevision,
    ) -> Self {
        let mut timer = Timer::new();
        // Power-on DIV phase differs between DMG revisions. These values match
        // the phases measured by mooneye's boot_div acceptance tests so the
        // first post-boot instruction sequence observes the expected timing.
        timer.div = match dmg_revision {
            DmgRevision::Rev0 => 0x1830,
            DmgRevision::RevA | DmgRevision::RevB | DmgRevision::RevC => 0xABCC,
        };

        let mut ppu = Ppu::new_with_mode(cgb);
        ppu.apply_boot_state(if cgb { None } else { Some(dmg_revision) });

        Self {
            wram: [[0; WRAM_BANK_SIZE]; 8],
            wram_bank: 1,
            hram: [0; 0x7F],
            cart: None,
            boot_rom: None,
            boot_mapped: false,
            if_reg: 0xE1,
            ie_reg: 0,
            serial: Serial::new(cgb, dmg_revision),
            ppu,
            apu: Apu::new_with_revisions(cgb, dmg_revision, cgb_revision),
            timer,
            input: Input::new(),
            hdma: HdmaState {
                src: 0,
                dst: Self::sanitize_vram_dma_dest(0),
                blocks: 0,
                mode: DmaMode::Gdma,
                active: false,
                cancelled: false,
            },
            key1: if cgb { 0x7E } else { 0 },
            rp: 0,
            dma_cycles: 0,
            dma_source: 0,
            pending_dma: None,
            pending_delay: 0,
            gdma_cycles: 0,
            cgb_mode: cgb,
            cgb_revision,
            dmg_revision,
            last_cpu_pc: None,
        }
    }

    pub fn new() -> Self {
        Self::new_with_mode(false)
    }

    pub fn load_cart(&mut self, cart: Cartridge) {
        let is_dmg = !cart.cgb;
        self.cart = Some(cart);
        if self.cgb_mode && is_dmg {
            self.ppu.apply_dmg_compatibility_palettes();
        }
    }

    pub fn save_cart_ram(&mut self) {
        if let Some(cart) = &mut self.cart
            && let Err(e) = cart.save_ram()
        {
            eprintln!("Failed to save RAM: {e}");
        }
    }

    pub fn load_boot_rom(&mut self, data: Vec<u8>) {
        self.boot_rom = Some(data);
        self.boot_mapped = true;
    }

    fn read_byte_inner(&mut self, addr: u16, allow_dma: bool) -> u8 {
        if !allow_dma && self.dma_cycles > 0 {
            match addr {
                // Allow ROM, WRAM/Echo and all I/O/HRAM accesses during the
                // transfer. These regions remain readable/writable because
                // they reside on buses that stay available even while OAM DMA
                // monopolizes the VRAM/OAM bus.
                0x0000..=0x7FFF | 0xC000..=0xFDFF | 0xFF00..=0xFFFF => {}
                0xFE00..=0xFEFF => {
                    #[cfg(feature = "ppu-trace")]
                    {
                        let pc_str = self
                            .last_cpu_pc
                            .map(|p| format!("{:04X}", p))
                            .unwrap_or_else(|| "<none>".to_string());
                        eprintln!(
                            "[DMA] read blocked (OAM) addr={:04X} dma_cycles={} pc={}",
                            addr, self.dma_cycles, pc_str
                        );
                    }
                    return 0xFF;
                }
                _ => {
                    #[cfg(feature = "ppu-trace")]
                    {
                        let region = if (0x8000..=0x9FFF).contains(&addr) {
                            "VRAM"
                        } else {
                            "OTHER"
                        };
                        let pc_str = self
                            .last_cpu_pc
                            .map(|p| format!("{:04X}", p))
                            .unwrap_or_else(|| "<none>".to_string());
                        eprintln!(
                            "[DMA] read blocked ({}) addr={:04X} dma_cycles={} dma_src={:04X} pc={}",
                            region, addr, self.dma_cycles, self.dma_source, pc_str
                        );
                    }
                    return 0xFF;
                }
            }
        }
        match addr {
            // When the boot ROM is mapped, overlay it on the lower
            // portion of the address space. On DMG this covers
            // 0x0000-0x00FF. On CGB the internal boot ROM also maps
            // 0x0200-0x08FF while leaving the cartridge header at
            // 0x0100-0x01FF visible.
            0x0000..=0x00FF if self.boot_mapped => self
                .boot_rom
                .as_ref()
                .and_then(|b| b.get(addr as usize).copied())
                .unwrap_or(0xFF),
            0x0200..=0x08FF if self.boot_mapped && self.cgb_mode => self
                .boot_rom
                .as_ref()
                .and_then(|b| b.get(addr as usize).copied())
                .unwrap_or(0xFF),
            0x0000..=0x7FFF => self.cart.as_mut().map(|c| c.read(addr)).unwrap_or(0xFF),
            0x8000..=0x9FFF => {
                let accessible = self.ppu.vram_accessible();
                if accessible {
                    let value = self.ppu.vram[self.ppu.vram_bank][(addr - 0x8000) as usize];
                    #[cfg(feature = "ppu-trace")]
                    {
                        let (stage, cycle, mode, mode_clock) = self.ppu.debug_startup_snapshot();
                        let pc_str = self
                            .last_cpu_pc
                            .map(|p| format!("{:04X}", p))
                            .unwrap_or_else(|| "<none>".to_string());
                        eprintln!(
                            "[PPU] VRAM read allow addr={:04X} val={:02X} bank={} pc={} stage={:?} cycle={:?} mode={} mode_clock={}",
                            addr, value, self.ppu.vram_bank, pc_str, stage, cycle, mode, mode_clock
                        );
                    }
                    value
                } else {
                    #[cfg(feature = "ppu-trace")]
                    {
                        let (stage, cycle, mode, mode_clock) = self.ppu.debug_startup_snapshot();
                        let pc_str = self
                            .last_cpu_pc
                            .map(|p| format!("{:04X}", p))
                            .unwrap_or_else(|| "<none>".to_string());
                        eprintln!(
                            "[PPU] VRAM read blocked addr={:04X} bank={} pc={} stage={:?} cycle={:?} mode={} mode_clock={}",
                            addr, self.ppu.vram_bank, pc_str, stage, cycle, mode, mode_clock
                        );
                    }
                    0xFF
                }
            }
            0xA000..=0xBFFF => self.cart.as_mut().map(|c| c.read(addr)).unwrap_or(0xFF),
            0xC000..=0xCFFF => self.wram[0][(addr - 0xC000) as usize],
            0xD000..=0xDFFF => self.wram[self.wram_bank][(addr - 0xD000) as usize],
            0xE000..=0xEFFF => self.wram[0][(addr - 0xE000) as usize],
            0xF000..=0xFDFF => self.wram[self.wram_bank][(addr - 0xF000) as usize],
            0xFE00..=0xFE9F => {
                if self.ppu.oam_accessible() {
                    self.ppu.oam[(addr - 0xFE00) as usize]
                } else {
                    0xFF
                }
            }
            0xFEA0..=0xFEFF => 0xFF,
            0xFF00 => self.input.read(),
            0xFF01 | 0xFF02 => self.serial.read(addr),
            0xFF04..=0xFF07 => self.timer.read(addr),
            0xFF0F => self.if_reg,
            0xFF10..=0xFF3F => self.apu.read_reg(addr),
            0xFF40..=0xFF45 | 0xFF47..=0xFF4B | 0xFF68..=0xFF6B => self.ppu.read_reg(addr),
            0xFF46 => self.ppu.dma,
            0xFF51 => {
                if self.cgb_mode {
                    (self.hdma.src >> 8) as u8
                } else {
                    0xFF
                }
            }
            0xFF52 => {
                if self.cgb_mode {
                    (self.hdma.src & 0x00F0) as u8
                } else {
                    0xFF
                }
            }
            0xFF53 => {
                if self.cgb_mode {
                    ((self.hdma.dst & 0x1F00) >> 8) as u8
                } else {
                    0xFF
                }
            }
            0xFF54 => {
                if self.cgb_mode {
                    (self.hdma.dst & 0x00F0) as u8
                } else {
                    0xFF
                }
            }
            0xFF55 => {
                if !self.cgb_mode {
                    0xFF
                } else if self.hdma.active {
                    // Busy flag (bit 7) is cleared while the DMA is running.
                    self.hdma.blocks.saturating_sub(1) & 0x7F
                } else if self.hdma.cancelled {
                    // After cancellation the hardware reports bit 7 set with the lower bits cleared.
                    0x80
                } else {
                    // Hardware returns 0xFF once HDMA/GDMA has completed or no transfer is pending.
                    0xFF
                }
            }
            0xFF4D => {
                if self.cgb_mode {
                    (self.key1 & 0x81) | 0x7E
                } else {
                    0xFF
                }
            }
            0xFF56 => {
                if self.cgb_mode {
                    self.rp | 0xC0
                } else {
                    0xFF
                }
            }
            0xFF4F => {
                if self.cgb_mode {
                    self.ppu.vram_bank as u8
                } else {
                    0xFF
                }
            }
            0xFF70 => {
                if self.cgb_mode {
                    self.wram_bank as u8
                } else {
                    0xFF
                }
            }
            0xFF76 | 0xFF77 => {
                if self.cgb_mode {
                    self.apu.read_pcm(addr)
                } else {
                    0xFF
                }
            }
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.ie_reg,
            _ => 0xFF,
        }
    }

    pub fn read_byte(&mut self, addr: u16) -> u8 {
        self.read_byte_inner(addr, false)
    }

    fn dma_read_byte(&mut self, addr: u16) -> u8 {
        let addr = if !self.cgb_mode && (0xFE00..=0xFF9F).contains(&addr) {
            addr.wrapping_sub(0x2000)
        } else {
            addr
        };

        self.read_byte_inner(addr, true)
    }

    pub fn write_byte(&mut self, addr: u16, val: u8) {
        if self.dma_cycles > 0 {
            match addr {
                0x0000..=0x7FFF | 0xC000..=0xFDFF | 0xFF00..=0xFFFF => {}
                0xFE00..=0xFEFF => {
                    #[cfg(feature = "ppu-trace")]
                    {
                        let pc_str = self
                            .last_cpu_pc
                            .map(|p| format!("{:04X}", p))
                            .unwrap_or_else(|| "<none>".to_string());
                        eprintln!(
                            "[DMA] write blocked (OAM) addr={:04X} val={:02X} dma_cycles={} pc={}",
                            addr, val, self.dma_cycles, pc_str
                        );
                    }
                    return;
                }
                _ => {
                    #[cfg(feature = "ppu-trace")]
                    {
                        let region = if (0x8000..=0x9FFF).contains(&addr) {
                            "VRAM"
                        } else {
                            "OTHER"
                        };
                        let pc_str = self
                            .last_cpu_pc
                            .map(|p| format!("{:04X}", p))
                            .unwrap_or_else(|| "<none>".to_string());
                        eprintln!(
                            "[DMA] write blocked ({}) addr={:04X} val={:02X} dma_cycles={} dma_src={:04X} pc={}",
                            region, addr, val, self.dma_cycles, self.dma_source, pc_str
                        );
                    }
                    return;
                }
            }
        }

        match addr {
            0x8000..=0x9FFF => {
                if self.ppu.vram_accessible() {
                    self.ppu.vram[self.ppu.vram_bank][(addr - 0x8000) as usize] = val;
                } else {
                    #[cfg(feature = "ppu-trace")]
                    {
                        let pc_str = self
                            .last_cpu_pc
                            .map(|p| format!("{:04X}", p))
                            .unwrap_or_else(|| "<none>".to_string());
                        eprintln!(
                            "[PPU] VRAM write blocked addr={:04X} val={:02X} bank={} pc={}",
                            addr, val, self.ppu.vram_bank, pc_str
                        );
                    }
                }
            }
            0x0000..=0x7FFF | 0xA000..=0xBFFF => {
                if let Some(cart) = self.cart.as_mut() {
                    cart.write(addr, val);
                }
            }
            0xC000..=0xCFFF => self.wram[0][(addr - 0xC000) as usize] = val,
            0xD000..=0xDFFF => self.wram[self.wram_bank][(addr - 0xD000) as usize] = val,
            0xE000..=0xEFFF => self.wram[0][(addr - 0xE000) as usize] = val,
            0xF000..=0xFDFF => self.wram[self.wram_bank][(addr - 0xF000) as usize] = val,
            0xFE00..=0xFE9F => {
                if self.ppu.oam_accessible() {
                    self.ppu.oam[(addr - 0xFE00) as usize] = val;
                }
            }
            0xFEA0..=0xFEFF => {}
            0xFF00 => self.input.write(val),
            0xFF01 | 0xFF02 => self.serial.write(addr, val),
            0xFF04 => {
                self.reset_div();
            }
            0xFF05..=0xFF07 => self.timer.write(addr, val, &mut self.if_reg),
            0xFF0F => self.if_reg = (val & 0x1F) | (self.if_reg & 0xE0),
            0xFF10..=0xFF3F => self.apu.write_reg(addr, val),
            0xFF40 => {
                let lcd_was_on = self.ppu.lcd_enabled();
                self.ppu.write_reg(addr, val);
                if lcd_was_on && !self.ppu.lcd_enabled() {
                    self.complete_active_hdma();
                }
            }
            0xFF41..=0xFF45 | 0xFF47..=0xFF4B | 0xFF68..=0xFF6B => self.ppu.write_reg(addr, val),
            0xFF51 => {
                if self.cgb_mode && !self.hdma.active {
                    self.hdma.src = (val as u16) << 8 | (self.hdma.src & 0x00FF);
                }
            }
            0xFF52 => {
                if self.cgb_mode && !self.hdma.active {
                    self.hdma.src = (self.hdma.src & 0xFF00) | (val & 0xF0) as u16;
                }
            }
            0xFF53 => {
                if self.cgb_mode && !self.hdma.active {
                    let vram_hi = (val & 0x1F) as u16;
                    let raw = (vram_hi << 8) | (self.hdma.dst & 0x00F0);
                    self.hdma.dst = Self::sanitize_vram_dma_dest(raw);
                }
            }
            0xFF54 => {
                if self.cgb_mode && !self.hdma.active {
                    let raw = (self.hdma.dst & 0x1F00) | (val as u16 & 0x00F0);
                    self.hdma.dst = Self::sanitize_vram_dma_dest(raw);
                }
            }
            0xFF55 => {
                if !self.cgb_mode {
                    return;
                }
                self.hdma.dst = Self::sanitize_vram_dma_dest(self.hdma.dst);
                let requested_blocks = (val & 0x7F) + 1;
                if self.hdma.active && (val & 0x80) == 0 {
                    // Abort ongoing HDMA. Hardware reports remaining blocks in FF55 when
                    // polled after cancellation, so keep the current block count.
                    self.hdma.active = false;
                    self.hdma.blocks = 0;
                    self.hdma.cancelled = true;
                } else if val & 0x80 == 0 {
                    self.start_gdma(requested_blocks);
                } else {
                    self.hdma.mode = DmaMode::Hdma;
                    self.hdma.blocks = requested_blocks;
                    self.hdma.active = true;
                    self.hdma.cancelled = false;
                    if !self.ppu.lcd_enabled() || self.ppu.in_hblank() {
                        self.hdma_hblank_transfer();
                    }
                }
            }
            0xFF4D => {
                if self.cgb_mode {
                    self.key1 = (self.key1 & 0x80) | (val & 0x01);
                }
            }
            0xFF56 => {
                if self.cgb_mode {
                    self.rp = val & 0xC1;
                }
            }
            0xFF4F => {
                if self.cgb_mode {
                    self.ppu.vram_bank = (val & 0x01) as usize;
                }
            }
            0xFF46 => {
                self.ppu.dma = val;
                let src = (val as u16) << 8;
                self.pending_dma = Some(src);
                // DMA starts after two M-cycles. `pending_delay` is tracked
                // in T-cycles (dots). In double-speed mode an M-cycle is 2
                // T-cycles instead of 4, so halve the delay there.
                self.pending_delay = if self.key1 & 0x80 != 0 { 4 } else { 8 };
                #[cfg(feature = "ppu-trace")]
                {
                    let region = if (0x0000..=0x7FFF).contains(&src) {
                        "ROM"
                    } else if (0xA000..=0xBFFF).contains(&src) {
                        "CARTRAM"
                    } else if (0xC000..=0xDFFF).contains(&src) || (0xE000..=0xFDFF).contains(&src) {
                        "WRAM"
                    } else {
                        "OTHER"
                    };
                    let pc_str = self
                        .last_cpu_pc
                        .map(|p| format!("{:04X}", p))
                        .unwrap_or_else(|| "<none>".to_string());
                    eprintln!(
                        "[DMA] pending OAM DMA scheduled src={:04X} region={} pending_delay=8 pc={}",
                        src, region, pc_str
                    );
                }
            }
            0xFF50 => self.boot_mapped = false,
            0xFF70 => {
                if self.cgb_mode {
                    let bank = (val & 0x07) as usize;
                    self.wram_bank = if bank == 0 { 1 } else { bank };
                }
            }
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = val,
            0xFFFF => self.ie_reg = val,
            _ => {}
        }
    }

    /// Write to VRAM bypassing mode checks (used by DMA transfers)
    fn vram_dma_write(&mut self, addr: u16, val: u8) {
        self.ppu.vram[self.ppu.vram_bank][(addr - 0x8000) as usize] = val;
    }

    pub fn take_serial(&mut self) -> Vec<u8> {
        self.serial.take_output()
    }

    /// Advance the ongoing OAM DMA transfer if active.
    pub fn dma_step(&mut self, cycles: u16) {
        for _ in 0..cycles {
            if self.pending_delay > 0 {
                self.pending_delay -= 1;
                if self.pending_delay == 0
                    && let Some(src) = self.pending_dma.take()
                {
                    self.dma_source = src;
                    // Number of T-cycles for the OAM DMA transfer: 160 M-cycles.
                    // In normal speed M-cycle = 4 T-cycles => 160 * 4 = 640.
                    // In double-speed M-cycle = 2 T-cycles => 160 * 2 = 320.
                    self.dma_cycles = if self.key1 & 0x80 != 0 { 320 } else { 640 };
                    #[cfg(feature = "ppu-trace")]
                    {
                        let region = if (0x0000..=0x7FFF).contains(&src) {
                            "ROM"
                        } else if (0xA000..=0xBFFF).contains(&src) {
                            "CARTRAM"
                        } else if (0xC000..=0xDFFF).contains(&src)
                            || (0xE000..=0xFDFF).contains(&src)
                        {
                            "WRAM"
                        } else {
                            "OTHER"
                        };
                        eprintln!(
                            "[DMA] OAM DMA started src={:04X} region={} dma_cycles={}",
                            src, region, self.dma_cycles
                        );
                    }
                }
            }

            if self.dma_cycles == 0 {
                continue;
            }

            // Determine per-byte cadence based on double-speed. In normal
            // speed one byte is transferred every 4 T-cycles; in double-speed
            // it's every 2 T-cycles.
            let per_byte = if self.key1 & 0x80 != 0 { 2 } else { 4 };
            let initial = if self.key1 & 0x80 != 0 { 320 } else { 640 };
            let elapsed = initial - self.dma_cycles;
            if elapsed.is_multiple_of(per_byte) {
                let idx: u16 = elapsed / per_byte;
                if idx < 0xA0 {
                    // Clear `last_cpu_pc` to avoid attributing DMA-originated reads
                    // to the last CPU instruction that happened to run. DMA
                    // engine operations are independent of the CPU and should
                    // not surface a CPU PC in logs.
                    self.last_cpu_pc = None;
                    let byte = self.dma_read_byte(self.dma_source.wrapping_add(idx));
                    self.ppu.oam[idx as usize] = byte;
                }
            }

            self.dma_cycles -= 1;
        }
    }

    /// Return true if a DMA transfer is in progress.
    pub fn dma_active(&self) -> bool {
        self.dma_cycles > 0 || self.pending_delay > 0
    }

    /// Return true if a General or HBlank DMA stall is in progress.
    pub fn gdma_active(&self) -> bool {
        self.gdma_cycles > 0
    }

    /// Decrement the GDMA stall counter by the given number of m-cycles.
    pub fn gdma_step(&mut self, cycles: u16) {
        if self.gdma_cycles > 0 {
            self.gdma_cycles = self.gdma_cycles.saturating_sub(cycles as u32);
        }
    }

    #[inline]
    fn sanitize_vram_dma_dest(addr: u16) -> u16 {
        0x8000 | (addr & 0x1FF0)
    }

    /// Perform a General DMA transfer immediately, consuming CPU cycles.
    fn start_gdma(&mut self, blocks: u8) {
        let total_bytes = blocks as usize * 0x10;
        let mut src = self.hdma.src;
        let mut dst = Self::sanitize_vram_dma_dest(self.hdma.dst);

        // Clear last_cpu_pc so these DMA-driven reads/writes are not
        // misattributed to the last executing CPU instruction in logs.
        self.last_cpu_pc = None;
        for _ in 0..total_bytes {
            // Read source using the DMA-aware reader so this GDMA operation
            // can proceed even if an OAM DMA (`dma_cycles`) is active.
            let byte = self.dma_read_byte(src);
            self.vram_dma_write(dst, byte);
            src = src.wrapping_add(1);
            dst = 0x8000 | ((dst.wrapping_add(1)) & 0x1FFF);
        }

        self.hdma.src = src;
        self.hdma.dst = Self::sanitize_vram_dma_dest(dst);
        self.hdma.active = false;
        self.hdma.blocks = 0;
        self.hdma.cancelled = false;
        self.gdma_cycles = blocks as u32 * self.hdma_block_cycle_cost();
    }

    /// Execute a single 0x10-byte HDMA burst during H-Blank.
    pub fn hdma_hblank_transfer(&mut self) {
        if !(self.hdma.active && self.hdma.mode == DmaMode::Hdma) {
            return;
        }
        self.perform_hdma_block();
    }

    fn perform_hdma_block(&mut self) {
        self.hdma.dst = Self::sanitize_vram_dma_dest(self.hdma.dst);
        // Clear last_cpu_pc so HDMA transfers don't get logged with the
        // previously executing CPU PC.
        self.last_cpu_pc = None;
        for _ in 0..0x10 {
            // HDMA source reads should also bypass the DMA blocking checks
            // so HDMA can transfer data even if an OAM DMA is currently
            // active.
            let byte = self.dma_read_byte(self.hdma.src);
            self.vram_dma_write(self.hdma.dst, byte);
            self.hdma.src = self.hdma.src.wrapping_add(1);
            self.hdma.dst = 0x8000 | ((self.hdma.dst.wrapping_add(1)) & 0x1FFF);
        }

        self.hdma.blocks = self.hdma.blocks.saturating_sub(1);
        if self.hdma.blocks == 0 {
            self.hdma.active = false;
            self.hdma.cancelled = false;
        }

        self.hdma.dst = Self::sanitize_vram_dma_dest(self.hdma.dst);
        self.gdma_cycles += self.hdma_block_cycle_cost();
    }

    fn complete_active_hdma(&mut self) {
        while self.hdma.active && self.hdma.mode == DmaMode::Hdma {
            self.perform_hdma_block();
        }
    }

    fn hdma_block_cycle_cost(&self) -> u32 {
        if self.key1 & 0x80 != 0 { 16 } else { 8 }
    }

    pub fn reset_div(&mut self) {
        let prev_div = self.timer.div;
        self.timer.reset_div(&mut self.if_reg);
        let double_speed = self.key1 & 0x80 != 0;
        self.apu.on_div_reset(prev_div, double_speed);
    }

    fn tick(&mut self, m_cycles: u32) {
        let hw_cycles = if self.key1 & 0x80 != 0 {
            2 * m_cycles as u16
        } else {
            4 * m_cycles as u16
        };
        let prev_div = self.timer.div;
        self.timer.step(hw_cycles, &mut self.if_reg);
        let curr_div = self.timer.div;
        // Advance 2 MHz domain before 1 MHz staging to match APU internal ordering
        self.apu.step(hw_cycles);
        self.apu.tick(prev_div, curr_div, self.key1 & 0x80 != 0);
        let _ = self.ppu.step(hw_cycles, &mut self.if_reg);
    }
}

impl Default for Mmu {
    fn default() -> Self {
        Self::new()
    }
}
