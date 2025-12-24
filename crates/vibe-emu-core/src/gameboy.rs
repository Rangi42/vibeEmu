use crate::{
    cpu::Cpu,
    hardware::{CgbRevision, DmgRevision},
    mmu::Mmu,
};

/// High-level emulator facade representing a single Game Boy / Game Boy Color.
///
/// `GameBoy` owns the CPU and MMU and provides constructors for common initial
/// states (post-boot vs. power-on) across DMG/CGB modes and hardware revisions.
pub struct GameBoy {
    /// CPU core.
    pub cpu: Cpu,
    /// Memory map and attached devices (PPU/APU/timer/cartridge/etc).
    pub mmu: Mmu,
    /// Whether the machine is running in CGB mode.
    pub cgb: bool,
    /// DMG CPU/board revision used for revision-specific quirks.
    pub dmg_revision: DmgRevision,
    /// CGB revision used for revision-specific quirks.
    pub cgb_revision: CgbRevision,
}

impl GameBoy {
    /// Creates a DMG-mode machine in the post-boot state.
    pub fn new() -> Self {
        Self::new_with_mode(false)
    }

    /// Creates a machine in the post-boot state.
    ///
    /// When `cgb` is `true`, the machine runs in CGB mode with default revisions.
    pub fn new_with_mode(cgb: bool) -> Self {
        Self::new_with_revisions(cgb, DmgRevision::default(), CgbRevision::default())
    }

    /// Creates a machine in the post-boot state with an explicit CGB revision.
    pub fn new_with_revision(cgb: bool, revision: CgbRevision) -> Self {
        Self::new_with_revisions(cgb, DmgRevision::default(), revision)
    }

    /// Creates a machine in the post-boot state with explicit DMG + CGB revisions.
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

    /// Creates a machine initialized to an approximate power-on state.
    ///
    /// This is intended for executing a boot ROM. If you are skipping the boot
    /// ROM, prefer [`Self::new_with_revisions`].
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

    /// Creates a power-on machine with an explicit CGB revision.
    pub fn new_power_on_with_revision(cgb: bool, revision: CgbRevision) -> Self {
        Self::new_power_on_with_revisions(cgb, DmgRevision::default(), revision)
    }

    /// Resets to the post-boot state, preserving cartridge and boot ROM.
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

    /// Resets to the power-on state, preserving cartridge and boot ROM.
    ///
    /// This is useful when you want to re-run the boot ROM sequence.
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
