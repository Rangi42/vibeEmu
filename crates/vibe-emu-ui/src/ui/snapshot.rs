use vibe_emu_core::gameboy::GameBoy;

#[derive(Clone, Copy, Debug, Default)]
pub struct CpuSnapshot {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
    pub ime: bool,
    pub cycles: u64,
}

#[derive(Clone, Debug, Default)]
pub struct PpuSnapshot {
    pub frame_counter: u64,
    pub cgb: bool,

    pub lcdc: u8,
    pub scy: u8,
    pub scx: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,

    pub vram0: Vec<u8>,
    pub vram1: Vec<u8>,
    pub oam: Vec<u8>,

    /// CGB BG palette colors as 0x00RRGGBB.
    pub cgb_bg_colors: [[u32; 4]; 8],
    /// CGB OBJ palette colors as 0x00RRGGBB.
    pub cgb_ob_colors: [[u32; 4]; 8],
}

impl PpuSnapshot {
    pub fn vram_bank(&self, bank: usize) -> &[u8] {
        match bank {
            0 => &self.vram0,
            _ => &self.vram1,
        }
    }

    pub fn obp(&self, which: u8) -> u8 {
        match which {
            0 => self.obp0,
            _ => self.obp1,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UiSnapshot {
    pub cpu: CpuSnapshot,
    pub ppu: PpuSnapshot,
}

impl UiSnapshot {
    pub fn from_gb(gb: &mut GameBoy) -> Self {
        let cpu = CpuSnapshot {
            a: gb.cpu.a,
            f: gb.cpu.f,
            b: gb.cpu.b,
            c: gb.cpu.c,
            d: gb.cpu.d,
            e: gb.cpu.e,
            h: gb.cpu.h,
            l: gb.cpu.l,
            sp: gb.cpu.sp,
            pc: gb.cpu.pc,
            ime: gb.cpu.ime,
            cycles: gb.cpu.cycles,
        };

        let ppu = &mut gb.mmu.ppu;
        let mut cgb_bg_colors = [[0u32; 4]; 8];
        let mut cgb_ob_colors = [[0u32; 4]; 8];
        if ppu.is_cgb() {
            for pal in 0..8 {
                for col in 0..4 {
                    cgb_bg_colors[pal][col] = ppu.bg_palette_color(pal, col);
                    cgb_ob_colors[pal][col] = ppu.ob_palette_color(pal, col);
                }
            }
        }

        let ppu_snap = PpuSnapshot {
            frame_counter: ppu.frames(),
            cgb: ppu.is_cgb(),
            lcdc: ppu.read_reg(0xFF40),
            scy: ppu.read_reg(0xFF42),
            scx: ppu.read_reg(0xFF43),
            bgp: ppu.read_reg(0xFF47),
            obp0: ppu.read_reg(0xFF48),
            obp1: ppu.read_reg(0xFF49),
            vram0: ppu.vram[0].to_vec(),
            vram1: ppu.vram[1].to_vec(),
            oam: ppu.oam.to_vec(),
            cgb_bg_colors,
            cgb_ob_colors,
        };

        Self { cpu, ppu: ppu_snap }
    }
}
