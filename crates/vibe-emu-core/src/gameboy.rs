use crate::{
    cpu::Cpu,
    hardware::{CgbRevision, DmgRevision},
    mmu::Mmu,
};

pub struct GameBoy {
    pub cpu: Cpu,
    pub mmu: Mmu,
    pub cgb: bool,
    pub dmg_revision: DmgRevision,
    pub cgb_revision: CgbRevision,
}

impl GameBoy {
    pub fn new() -> Self {
        Self::new_with_mode(false)
    }

    pub fn new_with_mode(cgb: bool) -> Self {
        Self::new_with_revisions(cgb, DmgRevision::default(), CgbRevision::default())
    }

    pub fn new_with_revision(cgb: bool, revision: CgbRevision) -> Self {
        Self::new_with_revisions(cgb, DmgRevision::default(), revision)
    }

    pub fn new_with_revisions(
        cgb: bool,
        dmg_revision: DmgRevision,
        cgb_revision: CgbRevision,
    ) -> Self {
        Self {
            cpu: Cpu::new_with_mode_and_revision(cgb, dmg_revision),
            mmu: Mmu::new_with_revisions(cgb, dmg_revision, cgb_revision),
            cgb,
            dmg_revision,
            cgb_revision,
        }
    }

    /// Create a Game Boy initialized to an approximate power-on state suitable
    /// for executing a boot ROM.
    pub fn new_power_on_with_revisions(
        cgb: bool,
        dmg_revision: DmgRevision,
        cgb_revision: CgbRevision,
    ) -> Self {
        Self {
            cpu: Cpu::new_power_on_with_revision(cgb, dmg_revision),
            mmu: Mmu::new_power_on_with_revisions(cgb, dmg_revision, cgb_revision),
            cgb,
            dmg_revision,
            cgb_revision,
        }
    }

    pub fn new_power_on_with_revision(cgb: bool, revision: CgbRevision) -> Self {
        Self::new_power_on_with_revisions(cgb, DmgRevision::default(), revision)
    }

    /// Reset the Game Boy to its initial power-on state while
    /// preserving the loaded cartridge and boot ROM.
    pub fn reset(&mut self) {
        let cart = self.mmu.cart.take();
        let boot = self.mmu.boot_rom.take();
        self.cpu = Cpu::new_with_mode_and_revision(self.cgb, self.dmg_revision);
        self.mmu = Mmu::new_with_revisions(self.cgb, self.dmg_revision, self.cgb_revision);
        if let Some(c) = cart {
            self.mmu.load_cart(c);
        }
        if let Some(b) = boot {
            self.mmu.load_boot_rom(b);
        }
    }

    /// Reset to a power-on state (instead of the post-boot state used when
    /// skipping the boot ROM), while preserving loaded cartridge and boot ROM.
    pub fn reset_power_on(&mut self) {
        let cart = self.mmu.cart.take();
        let boot = self.mmu.boot_rom.take();
        self.cpu = Cpu::new_power_on_with_revision(self.cgb, self.dmg_revision);
        self.mmu = Mmu::new_power_on_with_revisions(self.cgb, self.dmg_revision, self.cgb_revision);
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
