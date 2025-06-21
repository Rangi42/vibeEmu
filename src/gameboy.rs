use crate::{cpu::Cpu, mmu::Mmu};

pub struct GameBoy {
    pub cpu: Cpu,
    pub mmu: Mmu,
    pub cgb: bool,
}

impl GameBoy {
    pub fn new() -> Self {
        Self::new_with_mode(false)
    }

    pub fn new_with_mode(cgb: bool) -> Self {
        Self {
            cpu: Cpu::new_with_mode(cgb),
            mmu: Mmu::new_with_mode(cgb),
            cgb,
        }
    }

    /// Reset the Game Boy to its initial power-on state while
    /// preserving the loaded cartridge and boot ROM.
    pub fn reset(&mut self) {
        let cart = self.mmu.cart.take();
        let boot = self.mmu.boot_rom.take();
        self.cpu = Cpu::new_with_mode(self.cgb);
        self.mmu = Mmu::new_with_mode(self.cgb);
        if let Some(c) = cart {
            self.mmu.load_cart(c);
        }
        if let Some(b) = boot {
            self.mmu.load_boot_rom(b);
        }
    }
}

impl Default for GameBoy {
    fn default() -> Self {
        Self::new()
    }
}
