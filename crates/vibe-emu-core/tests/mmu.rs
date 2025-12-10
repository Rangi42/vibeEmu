use vibe_emu_core::{cartridge::Cartridge, mmu::Mmu};

#[test]
fn hdma_wait_loop_observes_idle_ff55() {
    let mut mmu = Mmu::new_with_mode(true);
    // Ensure the LCD is considered enabled so HDMA enters H-Blank mode.
    mmu.write_byte(0xFF40, 0x80);

    // Populate the HDMA source region with a known pattern.
    for (i, byte) in (0xC000..0xC010).enumerate() {
        mmu.write_byte(byte, i as u8);
    }

    // Program source (0xC000) and destination (0x8000).
    mmu.write_byte(0xFF51, 0xC0);
    mmu.write_byte(0xFF52, 0x00);
    mmu.write_byte(0xFF53, 0x80);
    mmu.write_byte(0xFF54, 0x00);

    // Kick off a single 16-byte H-Blank DMA block (value 0 => 1 block).
    mmu.write_byte(0xFF55, 0x80);
    assert_ne!(
        mmu.read_byte(0xFF55),
        0xFF,
        "HDMA should report busy immediately after start"
    );

    // Deliver one H-Blank slot and ensure the transfer completes.
    mmu.hdma_hblank_transfer();

    // Hardware returns 0xFF after HDMA completes, which the legacy wait loop relies on.
    assert_eq!(mmu.read_byte(0xFF55), 0xFF);
}

#[test]
fn wram_echo_and_bank_switch() {
    let mut mmu = Mmu::new_with_mode(true);
    mmu.write_byte(0xC000, 0xAA);
    assert_eq!(mmu.read_byte(0xC000), 0xAA);
    mmu.write_byte(0xE000, 0xBB);
    assert_eq!(mmu.read_byte(0xC000), 0xBB);

    mmu.write_byte(0xFF70, 0x02);
    mmu.write_byte(0xD000, 0xCC);
    assert_eq!(mmu.read_byte(0xD000), 0xCC);

    mmu.write_byte(0xFF70, 0x03);
    assert_eq!(mmu.read_byte(0xD000), 0x00);
    mmu.write_byte(0xD000, 0xDD);
    assert_eq!(mmu.read_byte(0xD000), 0xDD);

    mmu.write_byte(0xFF70, 0x02);
    assert_eq!(mmu.read_byte(0xD000), 0xCC);
}

#[test]
fn vram_bank_switch() {
    let mut mmu = Mmu::new_with_mode(true);
    mmu.write_byte(0x8000, 0x11);
    assert_eq!(mmu.read_byte(0x8000), 0x11);

    mmu.write_byte(0xFF4F, 0x01);
    assert_eq!(mmu.read_byte(0x8000), 0x00);
    mmu.write_byte(0x8000, 0x22);
    assert_eq!(mmu.read_byte(0x8000), 0x22);

    mmu.write_byte(0xFF4F, 0x00);
    assert_eq!(mmu.read_byte(0x8000), 0x11);
}

#[test]
fn boot_rom_disable() {
    let mut mmu = Mmu::new();
    mmu.load_boot_rom(vec![0xAA; 0x100]);
    mmu.load_cart(Cartridge::from_bytes_with_ram(vec![0xBB; 0x200], 0x2000));
    assert_eq!(mmu.read_byte(0x00), 0xAA);
    mmu.write_byte(0xFF50, 1);
    assert_eq!(mmu.read_byte(0x00), 0xBB);
}

#[test]
fn cgb_boot_rom_mapping() {
    // CGB mode MMU with a cartridge and a synthetic 0x900-byte boot ROM.
    let mut rom = vec![0u8; 0x8000];
    rom[0x0000] = 0xC0;
    rom[0x00FF] = 0xC1;
    rom[0x0100] = 0xC2;
    rom[0x01FF] = 0xC3;
    rom[0x0200] = 0xC4;
    rom[0x08FF] = 0xC5;
    let cart = Cartridge::from_bytes_with_ram(rom, 0);

    let mut mmu = Mmu::new_with_mode(true);
    mmu.load_cart(cart);

    let mut boot = vec![0u8; 0x900];
    boot[0x0000] = 0xA0;
    boot[0x00FF] = 0xA1;
    boot[0x0100] = 0xA2; // should never be visible while boot ROM is mapped
    boot[0x01FF] = 0xA3; // should never be visible while boot ROM is mapped
    boot[0x0200] = 0xA4;
    boot[0x08FF] = 0xA5;
    mmu.load_boot_rom(boot);

    // While boot ROM is mapped, DMG-compatible 0x0000-0x00FF region comes from boot ROM.
    assert_eq!(mmu.read_byte(0x0000), 0xA0);
    assert_eq!(mmu.read_byte(0x00FF), 0xA1);

    // On CGB, 0x0100-0x01FF remains mapped to the cartridge header.
    assert_eq!(mmu.read_byte(0x0100), 0xC2);
    assert_eq!(mmu.read_byte(0x01FF), 0xC3);

    // CGB-only extension: 0x0200-0x08FF should also be served from boot ROM.
    assert_eq!(mmu.read_byte(0x0200), 0xA4);
    assert_eq!(mmu.read_byte(0x08FF), 0xA5);

    // After disabling the boot ROM via FF50, all addresses should revert to the cartridge.
    mmu.write_byte(0xFF50, 1);
    assert_eq!(mmu.read_byte(0x0000), 0xC0);
    assert_eq!(mmu.read_byte(0x00FF), 0xC1);
    assert_eq!(mmu.read_byte(0x0100), 0xC2);
    assert_eq!(mmu.read_byte(0x01FF), 0xC3);
    assert_eq!(mmu.read_byte(0x0200), 0xC4);
    assert_eq!(mmu.read_byte(0x08FF), 0xC5);
}

#[test]
fn cartridge_ram_access() {
    let mut mmu = Mmu::new();
    mmu.load_cart(Cartridge::from_bytes_with_ram(vec![0; 0x200], 0x2000));

    mmu.write_byte(0xA000, 0x55);
    assert_eq!(mmu.read_byte(0xA000), 0x55);

    mmu.write_byte(0xBFFF, 0xAA);
    assert_eq!(mmu.read_byte(0xBFFF), 0xAA);
}

#[test]
fn mbc1_rom_bank_switching() {
    let mut rom = vec![0u8; 35 * 0x4000];
    rom[0x0147] = 0x01; // MBC1
    for i in 0..35 {
        rom[i * 0x4000] = i as u8;
    }

    let cart = Cartridge::load(rom);
    let mut mmu = Mmu::new();
    mmu.load_cart(cart);

    // default bank 1 at 0x4000
    assert_eq!(mmu.read_byte(0x4000), 1);

    mmu.write_byte(0x2000, 0x02); // select bank 2
    assert_eq!(mmu.read_byte(0x4000), 2);

    mmu.write_byte(0x4000, 0x01); // high bits 1 -> bank 0x22
    assert_eq!(mmu.read_byte(0x4000), 34);

    mmu.write_byte(0x6000, 0x01); // mode 1
    assert_eq!(mmu.read_byte(0x0000), 32);
}

#[test]
fn mbc1_ram_enable() {
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x03; // MBC1 + RAM + Battery
    rom[0x0149] = 0x03; // 32KB RAM
    let cart = Cartridge::from_bytes_with_ram(rom, 0x8000);

    let mut mmu = Mmu::new();
    mmu.load_cart(cart);

    mmu.write_byte(0xA000, 0x55);
    assert_eq!(mmu.read_byte(0xA000), 0xFF);

    mmu.write_byte(0x0000, 0x0A); // enable RAM
    mmu.write_byte(0xA000, 0x55);
    assert_eq!(mmu.read_byte(0xA000), 0x55);

    mmu.write_byte(0x0000, 0x00); // disable RAM
    assert_eq!(mmu.read_byte(0xA000), 0xFF);
}

#[test]
fn oam_dma_transfer() {
    let mut mmu = Mmu::new();
    for i in 0..0xA0u16 {
        mmu.write_byte(0x8000 + i, i as u8);
    }
    mmu.write_byte(0xFF46, 0x80); // copy from 0x8000
    mmu.dma_step(644);
    assert_eq!(mmu.ppu.oam[0], 0x00);
    assert_eq!(mmu.ppu.oam[0x9F], 0x9F);
}

#[test]
fn oam_dma_initial_delay() {
    let mut mmu = Mmu::new();
    for i in 0..0xA0u16 {
        mmu.write_byte(0x8000 + i, i as u8);
    }
    mmu.write_byte(0xFF46, 0x80);
    // First 4 cycles should be idle
    mmu.dma_step(4);
    assert_eq!(mmu.ppu.oam[0], 0x00);
    assert_eq!(mmu.ppu.oam[0x9F], 0x00);
    // Remaining cycles copy the data
    mmu.dma_step(640);
    assert_eq!(mmu.ppu.oam[0], 0x00);
    assert_eq!(mmu.ppu.oam[0x9F], 0x9F);
}

#[test]
fn oam_dma_restart_timing() {
    let mut mmu = Mmu::new();
    for i in 0..0xA0u16 {
        mmu.write_byte(0x8000 + i, i as u8);
        mmu.write_byte(0x9000 + i, (i + 0x10) as u8);
    }

    mmu.write_byte(0xFF46, 0x80);
    // Start DMA and copy first two bytes
    mmu.dma_step(8);
    assert_eq!(mmu.ppu.oam[0], 0x00);

    // Restart DMA while previous one is running
    mmu.write_byte(0xFF46, 0x90);
    // 1 M-cycle later, previous DMA still active
    mmu.dma_step(4);
    assert_eq!(mmu.ppu.oam[0], 0x00);
    // After another M-cycle, new DMA begins and overwrites first byte
    mmu.dma_step(4);
    assert_eq!(mmu.ppu.oam[0], 0x10);
}

#[test]
fn vram_oam_access_blocking() {
    let mut mmu = Mmu::new();
    mmu.ppu.mode = 3;
    mmu.write_byte(0x8000, 0x12);
    assert_eq!(mmu.read_byte(0x8000), 0xFF);
    mmu.ppu.mode = 0;
    mmu.write_byte(0x8000, 0x34);
    assert_eq!(mmu.read_byte(0x8000), 0x34);

    mmu.ppu.mode = 2;
    mmu.write_byte(0xFE00, 0x56);
    assert_eq!(mmu.read_byte(0xFE00), 0xFF);
    mmu.ppu.mode = 0;
    mmu.write_byte(0xFE00, 0x56);
    assert_eq!(mmu.read_byte(0xFE00), 0x56);
}
