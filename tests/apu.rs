use vibeEmu::apu::Apu;
use vibeEmu::mmu::Mmu;

fn tick_machine(apu: &mut Apu, div: &mut u16, cycles: u16) {
    let prev = *div;
    *div = div.wrapping_add(cycles);
    apu.tick(prev, *div, false);
    apu.step(cycles);
}

#[test]
#[ignore]
fn frame_sequencer_tick() {
    let mut apu = Apu::new();
    let mut div = 0u16;
    assert_eq!(apu.sequencer_step(), 0);
    for _ in 0..(8192 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    }
    assert_eq!(apu.sequencer_step(), 0);
    for _ in 0..(8192 * 7 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    }
    assert_eq!(apu.sequencer_step(), 0);
}

#[test]
#[ignore]
fn sample_generation() {
    let mut apu = Apu::new();
    // enable sound and channel 2 with simple settings
    apu.write_reg(0xFF26, 0x80); // master enable
    apu.write_reg(0xFF24, 0x77); // max volume
    apu.write_reg(0xFF25, 0x22); // ch2 left+right
    apu.write_reg(0xFF16, 0); // length
    apu.write_reg(0xFF17, 0xF0); // envelope
    apu.write_reg(0xFF18, 0); // freq low
    apu.write_reg(0xFF19, 0x80); // trigger
    // step enough cycles for a few samples
    let mut div = 0u16;
    for _ in 0..10 {
        for _ in 0..(95 / 4) {
            tick_machine(&mut apu, &mut div, 4);
        }
    }
    assert!(apu.pop_sample().is_some());
}
#[test]
#[ignore]
fn writes_ignored_when_disabled() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x00); // disable
    apu.write_reg(0xFF12, 0xF0);
    assert_eq!(apu.read_reg(0xFF12), 0xF0);
    apu.write_reg(0xFF26, 0x80); // enable
    apu.write_reg(0xFF12, 0xF0);
    assert_eq!(apu.read_reg(0xFF12) & 0xF0, 0xF0);
}

#[test]
#[ignore]
fn read_mask_unused_bits() {
    let apu = Apu::new();
    assert_eq!(apu.read_reg(0xFF11), 0xBF);
}

#[test]
#[ignore]
fn register_write_read_fidelity() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // enable APU
    apu.write_reg(0xFF10, 0x07);
    apu.write_reg(0xFF11, 0xA2);
    assert_eq!(apu.read_reg(0xFF10), 0x87);
    assert_eq!(apu.read_reg(0xFF11), 0xBF);
}

#[test]
#[ignore]
fn wave_ram_access() {
    let mut apu = Apu::new();
    // write while channel 3 inactive
    apu.write_reg(0xFF30, 0x12);
    assert_eq!(apu.read_reg(0xFF30), 0x12);

    // start channel 3
    apu.write_reg(0xFF1A, 0x80); // DAC on
    apu.write_reg(0xFF1E, 0x80); // trigger
    apu.write_reg(0xFF30, 0x34); // should be ignored
    assert_eq!(apu.read_reg(0xFF30), 0xFF);

    // disable DAC while length counter still running
    apu.write_reg(0xFF1A, 0x00);
    apu.write_reg(0xFF30, 0x56);
    assert_eq!(apu.read_reg(0xFF30), 0x56);

    // power cycle should not clear wave RAM
    apu.write_reg(0xFF26, 0x00);
    apu.write_reg(0xFF26, 0x80);
    assert_eq!(apu.read_reg(0xFF30), 0x56);
}

#[test]
#[ignore]
fn dac_off_disables_channel() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // enable
    apu.write_reg(0xFF12, 0xF0); // envelope with volume
    apu.write_reg(0xFF14, 0x80); // trigger channel 1
    assert_eq!(apu.read_reg(0xFF26) & 0x01, 0x01);
    apu.write_reg(0xFF12, 0x00); // turn DAC off
    assert_eq!(apu.read_reg(0xFF26) & 0x01, 0x00);
}

#[test]
#[ignore]
fn sweep_trigger_and_step() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // master enable
    apu.write_reg(0xFF10, 0x11); // period=1, shift=1
    apu.write_reg(0xFF12, 0xF0); // envelope (DAC on)
    // set frequency 0x200
    apu.write_reg(0xFF13, 0x00);
    apu.write_reg(0xFF14, 0x82); // high bits=2, trigger
    // immediately applied sweep -> freq should be 0x300
    assert_eq!(apu.ch1_frequency(), 0x300);
    // advance until the sequencer clocks sweep (step 2)
    let mut div = 0u16;
    for _ in 0..(8192 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    } // step 1
    for _ in 0..(8192 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    } // step 2
    for _ in 0..(8192 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    } // step 3 (sweep clocked on previous step)
    assert_eq!(apu.ch1_frequency(), 0x6C0);
}

#[test]
fn sweep_disabled_when_period_zero() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // enable
    apu.write_reg(0xFF10, 0x11); // period=1, shift=1
    apu.write_reg(0xFF12, 0xF0); // DAC on
    apu.write_reg(0xFF13, 0x00);
    apu.write_reg(0xFF14, 0x82); // trigger with freq=0x200
    assert_eq!(apu.ch1_frequency(), 0x300);
    // disable sweep by setting period to 0
    apu.write_reg(0xFF10, 0x01);
    let mut div = 0u16;
    for _ in 0..64 {
        tick_machine(&mut apu, &mut div, 4);
    }
    assert_eq!(apu.ch1_frequency(), 0x300);
}

#[test]
fn sweep_subtraction_mode() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80);
    apu.write_reg(0xFF10, 0x19); // period=1, negate, shift=1
    apu.write_reg(0xFF12, 0xF0);
    apu.write_reg(0xFF13, 0x00);
    apu.write_reg(0xFF14, 0x82); // freq=0x200, trigger
    assert_eq!(apu.ch1_frequency(), 0x100);
    let mut div = 0u16;
    for _ in 0..8 {
        tick_machine(&mut apu, &mut div, 4);
    }
    for _ in 0..8 {
        tick_machine(&mut apu, &mut div, 4);
    }
    for _ in 0..8 {
        tick_machine(&mut apu, &mut div, 4);
    }
    assert_eq!(apu.ch1_frequency(), 0x80);
}

#[test]
fn sweep_overflow_with_period_zero_disables_channel() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80);
    apu.write_reg(0xFF10, 0x01); // period=0, shift=1 (addition)
    apu.write_reg(0xFF12, 0xF0);
    apu.write_reg(0xFF13, 0xF8); // freq high to overflow
    apu.write_reg(0xFF14, 0x87); // high bits=7, trigger
    // overflow should disable channel immediately
    assert_eq!(apu.read_reg(0xFF26) & 0x01, 0x00);
}

#[test]
fn sweep_updates_frequency_registers() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80);
    apu.write_reg(0xFF10, 0x11); // period=1, shift=1
    apu.write_reg(0xFF12, 0xF0);
    apu.write_reg(0xFF13, 0x00);
    apu.write_reg(0xFF14, 0x82); // trigger
    let mut div = 0u16;
    for _ in 0..8 {
        tick_machine(&mut apu, &mut div, 4);
    }
    for _ in 0..8 {
        tick_machine(&mut apu, &mut div, 4);
    }
    for _ in 0..8 {
        tick_machine(&mut apu, &mut div, 4);
    }
    assert_eq!(apu.ch1_frequency(), 0x480);
}

#[test]
#[ignore]
fn pcm_register_open_bus() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x00); // power off
    assert_eq!(apu.read_pcm(0xFF76), 0xFF);
    assert_eq!(apu.read_pcm(0xFF77), 0xFF);
}

#[test]
#[ignore]
fn pcm_register_sample_values() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // enable
    // ch1 low, ch2 high so PCM12 should be 0xC0
    apu.write_reg(0xFF11, 0x00); // duty 12.5%
    apu.write_reg(0xFF12, 0xF0); // max volume
    apu.write_reg(0xFF14, 0x80); // trigger

    apu.write_reg(0xFF16, 0xC0); // duty 75%
    apu.write_reg(0xFF17, 0xF0);
    apu.write_reg(0xFF19, 0x80); // trigger

    let mut div = 0u16;
    for _ in 0..(8300 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    }

    assert_eq!(apu.read_pcm(0xFF76), 0x0C);
}
#[test]
#[ignore]
fn pcm_mmu_mapping() {
    let mut mmu = Mmu::new_with_mode(true);
    mmu.write_byte(0xFF26, 0x80);
    mmu.write_byte(0xFF11, 0x00); // duty 12.5%
    mmu.write_byte(0xFF12, 0xF0);
    mmu.write_byte(0xFF14, 0x80);
    mmu.write_byte(0xFF16, 0xC0);
    mmu.write_byte(0xFF17, 0xF0);
    mmu.write_byte(0xFF19, 0x80);
    {
        let mut apu = mmu.apu.lock().unwrap();
        let mut div = 0u16;
        for _ in 0..(8300 / 4) {
            tick_machine(&mut apu, &mut div, 4);
        }
    }
    assert_eq!(mmu.read_byte(0xFF76), 0x0C);
    let mut dmg = Mmu::new();
    assert_eq!(dmg.read_byte(0xFF76), 0xFF);
}
#[test]
fn nr52_power_toggle() {
    let mut apu = Apu::new();
    // default power state should be on
    assert_eq!(apu.read_reg(0xFF26) & 0x80, 0x80);
    // power off
    apu.write_reg(0xFF26, 0x00);
    assert_eq!(apu.read_reg(0xFF26), 0x70);
    // power back on
    apu.write_reg(0xFF26, 0x80);
    assert_eq!(apu.read_reg(0xFF26), 0xF0);
    // writing channel bits should not change status
    apu.write_reg(0xFF26, 0x8F);
    assert_eq!(apu.read_reg(0xFF26), 0xF0);
}

#[test]
fn nr52_clears_registers_when_off() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // ensure enabled
    apu.write_reg(0xFF12, 0xF0);
    assert_eq!(apu.read_reg(0xFF12) & 0xF0, 0xF0);
    // power off clears registers
    apu.write_reg(0xFF26, 0x00);
    assert_eq!(apu.read_reg(0xFF12), 0x00);
    // writes ignored while off
    apu.write_reg(0xFF12, 0xF0);
    assert_eq!(apu.read_reg(0xFF12), 0x00);
    // power on again keeps cleared value
    apu.write_reg(0xFF26, 0x80);
    assert_eq!(apu.read_reg(0xFF12), 0x00);
}

#[test]
fn nr52_channel_status_bits() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80);
    assert_eq!(apu.read_reg(0xFF26) & 0x0F, 0x00);
    // trigger channel 1
    apu.write_reg(0xFF12, 0xF0);
    apu.write_reg(0xFF14, 0x80);
    assert_eq!(apu.read_reg(0xFF26) & 0x01, 0x01);
    // trigger channel 2
    apu.write_reg(0xFF17, 0xF0);
    apu.write_reg(0xFF19, 0x80);
    assert_eq!(apu.read_reg(0xFF26) & 0x03, 0x03);
    // trigger channel 3
    apu.write_reg(0xFF1A, 0x80);
    apu.write_reg(0xFF1E, 0x80);
    assert_eq!(apu.read_reg(0xFF26) & 0x07, 0x07);
    // trigger channel 4
    apu.write_reg(0xFF21, 0xF0);
    apu.write_reg(0xFF23, 0x80);
    assert_eq!(apu.read_reg(0xFF26) & 0x0F, 0x0F);
}

#[test]
fn nr52_wave_ram_persist() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF30, 0x12);
    apu.write_reg(0xFF26, 0x00);
    assert_eq!(apu.read_reg(0xFF30), 0x12);
    apu.write_reg(0xFF30, 0x34);
    apu.write_reg(0xFF26, 0x80);
    assert_eq!(apu.read_reg(0xFF30), 0x34);
}

fn run_ch2_sample(pan: u8) -> (i16, i16) {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // enable
    apu.write_reg(0xFF24, 0x77); // max volume
    apu.write_reg(0xFF25, pan); // panning
    apu.write_reg(0xFF16, 0); // length
    apu.write_reg(0xFF17, 0xF0); // envelope
    apu.write_reg(0xFF18, 0); // freq low
    apu.write_reg(0xFF19, 0x80); // trigger
    let mut div = 0u16;
    for _ in 0..25 {
        tick_machine(&mut apu, &mut div, 4);
    }
    let left = apu.pop_sample().unwrap();
    let right = apu.pop_sample().unwrap();
    (left, right)
}

fn run_ch2_sample_with_nr50(pan: u8, nr50: u8) -> (i16, i16) {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // enable
    apu.write_reg(0xFF24, nr50); // master volume
    apu.write_reg(0xFF25, pan); // panning
    apu.write_reg(0xFF16, 0); // length
    apu.write_reg(0xFF17, 0xF0); // envelope
    apu.write_reg(0xFF18, 0); // freq low
    apu.write_reg(0xFF19, 0x80); // trigger
    let mut div = 0u16;
    for _ in 0..25 {
        tick_machine(&mut apu, &mut div, 4);
    }
    let left = apu.pop_sample().unwrap();
    let right = apu.pop_sample().unwrap();
    (left, right)
}

#[test]
fn nr51_ch2_left_only() {
    let (left, right) = run_ch2_sample(0x20);
    assert_ne!(left, 0);
    assert_eq!(right, 0);
}

#[test]
fn nr51_ch2_right_only() {
    let (left, right) = run_ch2_sample(0x02);
    assert_eq!(left, 0);
    assert_ne!(right, 0);
}

#[test]
fn nr51_ch2_center() {
    let (left, right) = run_ch2_sample(0x22);
    assert_ne!(left, 0);
    assert_eq!(left, right);
}

#[test]
fn nr51_ch2_off() {
    let (left, right) = run_ch2_sample(0x00);
    assert_eq!(left, 0);
    assert_eq!(right, 0);
}

#[test]
fn nr50_volume_zero_not_muted() {
    let (left, right) = run_ch2_sample_with_nr50(0x22, 0x00);
    assert_ne!(left, 0);
    assert_ne!(right, 0);
}

#[test]
fn nr50_left_vs_right_volume() {
    let (left, right) = run_ch2_sample_with_nr50(0x22, 0x70);
    assert!(left.abs() > right.abs());
    assert_ne!(right, 0);
}

#[test]
fn nr50_vin_bits_ignored() {
    let (l1, r1) = run_ch2_sample_with_nr50(0x22, 0x77);
    let (l2, r2) = run_ch2_sample_with_nr50(0x22, 0xF7);
    assert_eq!(l1, l2);
    assert_eq!(r1, r2);
}

#[test]
fn nr11_write_sets_duty_and_length() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80);
    let val = 0xCA; // duty 3, length 0x0A
    apu.write_reg(0xFF11, val);
    assert_eq!(apu.ch1_duty(), 3);
    assert_eq!(apu.ch1_length(), 64 - (val & 0x3F));
    assert_eq!(apu.read_reg(0xFF11), val | 0x3F);
}

#[test]
fn nr11_length_counter_expires() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80);
    apu.write_reg(0xFF11, 0x3F); // length = 1
    apu.write_reg(0xFF12, 0xF0); // DAC on
    apu.write_reg(0xFF14, 0x80); // trigger, length disabled
    assert_eq!(apu.read_reg(0xFF26) & 0x01, 0x01);

    let mut div = 0u16;
    for _ in 0..(8192 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    }

    apu.write_reg(0xFF14, 0x40); // enable length
    for _ in 0..(8192 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    }
    assert_eq!(apu.read_reg(0xFF26) & 0x01, 0x00);
}

#[test]
fn nr12_zero_turns_off_dac() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80); // enable APU
    apu.write_reg(0xFF12, 0xF0); // DAC on
    apu.write_reg(0xFF14, 0x80); // trigger
    assert_eq!(apu.read_reg(0xFF26) & 0x01, 0x01);
    apu.write_reg(0xFF12, 0x00); // writing zero should disable DAC
    assert_eq!(apu.read_reg(0xFF26) & 0x01, 0x00);
}

#[test]
fn nr12_write_requires_retrigger() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80);
    apu.write_reg(0xFF12, 0xF0); // initial volume 15
    apu.write_reg(0xFF14, 0x80); // trigger
    assert_eq!(apu.ch1_volume(), 0xF);
    // write new envelope while channel active
    apu.write_reg(0xFF12, 0x50); // initial volume 5
    // volume should remain unchanged until retrigger
    assert_eq!(apu.ch1_volume(), 0xF);
    apu.write_reg(0xFF14, 0x80); // retrigger
    assert_eq!(apu.ch1_volume(), 0x5);
}

#[test]
fn nr12_register_unchanged_after_envelope() {
    let mut apu = Apu::new();
    apu.write_reg(0xFF26, 0x80);
    apu.write_reg(0xFF11, 0x00);
    apu.write_reg(0xFF12, 0x8A); // init 8, increase, pace=2
    apu.write_reg(0xFF14, 0x80); // trigger
    let mut div = 0u16;
    for _ in 0..(65536 / 4) {
        tick_machine(&mut apu, &mut div, 4);
    }
    assert_eq!(apu.read_reg(0xFF12), 0x8A);
    assert_ne!(apu.ch1_volume(), 8);
}
