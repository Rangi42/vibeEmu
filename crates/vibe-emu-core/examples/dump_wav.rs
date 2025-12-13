use std::env;
use std::fs;
use std::path::Path;
use vibe_emu_core::{
    cartridge::Cartridge,
    gameboy::GameBoy,
    hardware::{CgbRevision, DmgRevision},
};

const DEFAULT_SECONDS: f64 = 3.0;
const SAMPLE_RATE: u32 = 44_100;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let rom_path = args
        .next()
        .ok_or("expected <rom> <output wav> [--seconds=N] [--cgb] [--cgb-rev=REV]")?;
    let out_path = args
        .next()
        .ok_or("expected <rom> <output wav> [--seconds=N] [--cgb] [--cgb-rev=REV]")?;

    let mut seconds = DEFAULT_SECONDS;
    let mut override_cgb = None;
    let mut cgb_revision = CgbRevision::RevE;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--seconds=") {
            seconds = value.parse()?;
        } else if arg == "--cgb" {
            override_cgb = Some(true);
        } else if arg == "--dmg" {
            override_cgb = Some(false);
        } else if let Some(value) = arg.strip_prefix("--cgb-rev=") {
            cgb_revision = parse_cgb_revision(value)?;
            override_cgb = Some(true);
        } else {
            return Err(format!("unrecognised flag: {arg}").into());
        }
    }

    if seconds <= 0.0 {
        return Err("seconds must be positive".into());
    }

    let rom = fs::read(&rom_path)?;
    let cart = Cartridge::load(rom);

    let force_cgb = override_cgb.unwrap_or(cart.cgb);
    let mut gb = GameBoy::new_with_revisions(force_cgb, DmgRevision::default(), cgb_revision);
    gb.mmu.load_cart(cart);

    gb.mmu.apu.set_speed(1.0);
    let consumer = gb.mmu.apu.enable_output(SAMPLE_RATE);

    let total_frames = (seconds * SAMPLE_RATE as f64).ceil() as usize;
    let mut frames_written = 0usize;

    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let out_dir = Path::new(&out_path);
    if let Some(parent) = out_dir
        .parent()
        .and_then(|p| (!p.as_os_str().is_empty()).then_some(p))
    {
        fs::create_dir_all(parent)?;
    }
    let mut writer = hound::WavWriter::create(out_dir, spec)?;

    // Allow ample CPU cycles so ROMs with long silences have time to ramp.
    let cycle_budget = (total_frames as u64 * 512).max(1_000_000);

    while frames_written < total_frames && gb.cpu.cycles <= cycle_budget {
        gb.cpu.step(&mut gb.mmu);
        while frames_written < total_frames {
            let Some((left, right)) = consumer.pop_stereo() else {
                break;
            };
            writer.write_sample(left)?;
            writer.write_sample(right)?;
            frames_written += 1;
        }
    }

    // Drain any leftover samples once the main loop exits.
    while frames_written < total_frames {
        let Some((left, right)) = consumer.pop_stereo() else {
            break;
        };
        writer.write_sample(left)?;
        writer.write_sample(right)?;
        frames_written += 1;
    }

    writer.finalize()?;

    if frames_written < total_frames {
        eprintln!(
            "warning: only captured {frames_written} of {total_frames} frames before hitting the CPU cycle guard"
        );
    } else {
        println!(
            "wrote {frames_written} stereo frames ({seconds:.2}s) to {}",
            out_dir.display()
        );
    }

    Ok(())
}

fn parse_cgb_revision(value: &str) -> Result<CgbRevision, Box<dyn std::error::Error>> {
    match value.to_ascii_uppercase().as_str() {
        "0" | "REV0" => Ok(CgbRevision::Rev0),
        "A" | "REVA" => Ok(CgbRevision::RevA),
        "B" | "REVB" => Ok(CgbRevision::RevB),
        "C" | "REVC" => Ok(CgbRevision::RevC),
        "D" | "REVD" => Ok(CgbRevision::RevD),
        "E" | "REVE" => Ok(CgbRevision::RevE),
        other => Err(format!("unknown CGB revision: {other}").into()),
    }
}
