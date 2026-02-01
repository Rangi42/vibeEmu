/// Decode an SM83 instruction from the given memory slice.
/// `mem` should be a slice starting at the instruction to decode.
/// `addr` is the absolute address (used for relative jump target display).
/// Returns (mnemonic, instruction_length, optional_target_address).
/// The target address is set for JP, JR, CALL, LD with address operands.
pub fn decode_sm83(mem: &[u8], addr: u16) -> (String, u16, Option<u16>) {
    let get = |offset: usize| -> u8 { mem.get(offset).copied().unwrap_or(0) };
    let op = get(0);
    let imm8 = || get(1);
    let imm16 = || {
        let lo = get(1) as u16;
        let hi = get(2) as u16;
        (hi << 8) | lo
    };

    if op == 0xCB {
        let (s, len) = decode_cb(get(1));
        return (s, len, None);
    }

    decode_base(addr, op, imm8, imm16)
}

fn decode_base<F8, F16>(addr: u16, op: u8, imm8: F8, imm16: F16) -> (String, u16, Option<u16>)
where
    F8: Fn() -> u8,
    F16: Fn() -> u16,
{
    let x = op >> 6;
    let y = (op >> 3) & 0x07;
    let z = op & 0x07;
    let p = y >> 1;
    let q = y & 0x01;

    let r = |idx: u8| -> &'static str {
        match idx {
            0 => "B",
            1 => "C",
            2 => "D",
            3 => "E",
            4 => "H",
            5 => "L",
            6 => "(HL)",
            7 => "A",
            _ => "?",
        }
    };

    let rp = |idx: u8| -> &'static str {
        match idx {
            0 => "BC",
            1 => "DE",
            2 => "HL",
            3 => "SP",
            _ => "?",
        }
    };

    let rp2 = |idx: u8| -> &'static str {
        match idx {
            0 => "BC",
            1 => "DE",
            2 => "HL",
            3 => "AF",
            _ => "?",
        }
    };

    let alu = |idx: u8| -> &'static str {
        match idx {
            0 => "ADD",
            1 => "ADC",
            2 => "SUB",
            3 => "SBC",
            4 => "AND",
            5 => "XOR",
            6 => "OR",
            7 => "CP",
            _ => "?",
        }
    };

    // Relative jump: returns (mnemonic_with_placeholder, len, target)
    let rel = |mn: &str| -> (String, u16, Option<u16>) {
        let e = imm8() as i8;
        let dest = addr.wrapping_add(2).wrapping_add(e as u16);
        (format!("{mn} ${dest:04X}"), 2, Some(dest))
    };

    match x {
        0 => match z {
            0 => match y {
                0 => ("NOP".to_string(), 1, None),
                1 => (format!("LD (${:04X}),SP", imm16()), 3, None),
                2 => ("STOP".to_string(), 2, None),
                3 => rel("JR"),
                4 => rel("JR NZ,"),
                5 => rel("JR Z,"),
                6 => rel("JR NC,"),
                7 => rel("JR C,"),
                _ => unreachable!(),
            },
            1 => {
                let rp_name = rp(p);
                if q == 0 {
                    (format!("LD {rp_name},${:04X}", imm16()), 3, None)
                } else {
                    (format!("ADD HL,{rp_name}"), 1, None)
                }
            }
            2 => {
                let s = match (q, p) {
                    (0, 0) => "LD (BC),A".to_string(),
                    (0, 1) => "LD (DE),A".to_string(),
                    (0, 2) => "LD (HL+),A".to_string(),
                    (0, 3) => "LD (HL-),A".to_string(),
                    (1, 0) => "LD A,(BC)".to_string(),
                    (1, 1) => "LD A,(DE)".to_string(),
                    (1, 2) => "LD A,(HL+)".to_string(),
                    (1, 3) => "LD A,(HL-)".to_string(),
                    _ => format!("DB ${op:02X}"),
                };
                (s, 1, None)
            }
            3 => {
                let rp_name = rp(p);
                if q == 0 {
                    (format!("INC {rp_name}"), 1, None)
                } else {
                    (format!("DEC {rp_name}"), 1, None)
                }
            }
            4 => (format!("INC {}", r(y)), 1, None),
            5 => (format!("DEC {}", r(y)), 1, None),
            6 => (format!("LD {},${:02X}", r(y), imm8()), 2, None),
            7 => match y {
                0 => ("RLCA".to_string(), 1, None),
                1 => ("RRCA".to_string(), 1, None),
                2 => ("RLA".to_string(), 1, None),
                3 => ("RRA".to_string(), 1, None),
                4 => ("DAA".to_string(), 1, None),
                5 => ("CPL".to_string(), 1, None),
                6 => ("SCF".to_string(), 1, None),
                7 => ("CCF".to_string(), 1, None),
                _ => (format!("DB ${op:02X}"), 1, None),
            },
            _ => (format!("DB ${op:02X}"), 1, None),
        },
        1 => {
            if op == 0x76 {
                return ("HALT".to_string(), 1, None);
            }
            (format!("LD {},{}", r(y), r(z)), 1, None)
        }
        2 => (format!("{} {}", alu(y), r(z)), 1, None),
        3 => match z {
            0 => match y {
                0 => ("RET NZ".to_string(), 1, None),
                1 => ("RET Z".to_string(), 1, None),
                2 => ("RET NC".to_string(), 1, None),
                3 => ("RET C".to_string(), 1, None),
                4 => {
                    let offset = imm8();
                    let target = 0xFF00 | (offset as u16);
                    (format!("LDH (${target:04X}),A"), 2, Some(target))
                }
                5 => {
                    let e = imm8() as i8;
                    (format!("ADD SP,{e}"), 2, None)
                }
                6 => {
                    let offset = imm8();
                    let target = 0xFF00 | (offset as u16);
                    (format!("LDH A,(${target:04X})"), 2, Some(target))
                }
                7 => {
                    let e = imm8() as i8;
                    (format!("LD HL,SP+{e}"), 2, None)
                }
                _ => (format!("DB ${op:02X}"), 1, None),
            },
            1 => {
                if q == 0 {
                    (format!("POP {}", rp2(p)), 1, None)
                } else {
                    match p {
                        0 => ("RET".to_string(), 1, None),
                        1 => ("RETI".to_string(), 1, None),
                        2 => ("JP (HL)".to_string(), 1, None),
                        3 => ("LD SP,HL".to_string(), 1, None),
                        _ => (format!("DB ${op:02X}"), 1, None),
                    }
                }
            }
            2 => match y {
                0 => {
                    let target = imm16();
                    (format!("JP NZ,${target:04X}"), 3, Some(target))
                }
                1 => {
                    let target = imm16();
                    (format!("JP Z,${target:04X}"), 3, Some(target))
                }
                2 => {
                    let target = imm16();
                    (format!("JP NC,${target:04X}"), 3, Some(target))
                }
                3 => {
                    let target = imm16();
                    (format!("JP C,${target:04X}"), 3, Some(target))
                }
                4 => ("LDH (C),A".to_string(), 1, None),
                5 => (format!("LD (${:04X}),A", imm16()), 3, None),
                6 => ("LDH A,(C)".to_string(), 1, None),
                7 => (format!("LD A,(${:04X})", imm16()), 3, None),
                _ => (format!("DB ${op:02X}"), 1, None),
            },
            3 => match y {
                0 => {
                    let target = imm16();
                    (format!("JP ${target:04X}"), 3, Some(target))
                }
                1 => ("PREFIX CB".to_string(), 1, None),
                6 => ("DI".to_string(), 1, None),
                7 => ("EI".to_string(), 1, None),
                _ => (format!("DB ${op:02X}"), 1, None),
            },
            4 => match y {
                0 => {
                    let target = imm16();
                    (format!("CALL NZ,${target:04X}"), 3, Some(target))
                }
                1 => {
                    let target = imm16();
                    (format!("CALL Z,${target:04X}"), 3, Some(target))
                }
                2 => {
                    let target = imm16();
                    (format!("CALL NC,${target:04X}"), 3, Some(target))
                }
                3 => {
                    let target = imm16();
                    (format!("CALL C,${target:04X}"), 3, Some(target))
                }
                _ => (format!("DB ${op:02X}"), 1, None),
            },
            5 => {
                if q == 0 {
                    (format!("PUSH {}", rp2(p)), 1, None)
                } else if p == 0 {
                    let target = imm16();
                    (format!("CALL ${target:04X}"), 3, Some(target))
                } else {
                    (format!("DB ${op:02X}"), 1, None)
                }
            }
            6 => (format!("{} ${:02X}", alu(y), imm8()), 2, None),
            7 => (format!("RST ${:02X}", y * 8), 1, None),
            _ => (format!("DB ${op:02X}"), 1, None),
        },
        _ => (format!("DB ${op:02X}"), 1, None),
    }
}

fn decode_cb(op: u8) -> (String, u16) {
    let x = op >> 6;
    let y = (op >> 3) & 0x07;
    let z = op & 0x07;

    let r = |idx: u8| -> &'static str {
        match idx {
            0 => "B",
            1 => "C",
            2 => "D",
            3 => "E",
            4 => "H",
            5 => "L",
            6 => "(HL)",
            7 => "A",
            _ => "?",
        }
    };

    let rot = |idx: u8| -> &'static str {
        match idx {
            0 => "RLC",
            1 => "RRC",
            2 => "RL",
            3 => "RR",
            4 => "SLA",
            5 => "SRA",
            6 => "SWAP",
            7 => "SRL",
            _ => "?",
        }
    };

    let s = match x {
        0 => format!("{} {}", rot(y), r(z)),
        1 => format!("BIT {y},{}", r(z)),
        2 => format!("RES {y},{}", r(z)),
        3 => format!("SET {y},{}", r(z)),
        _ => format!("DB $CB{op:02X}"),
    };

    (s, 2)
}

pub fn format_bytes(mem: &[u8], addr: u16, len: u16) -> String {
    let mut s = String::with_capacity(len as usize * 3);
    for i in 0..len {
        if i > 0 {
            s.push(' ');
        }
        let b = mem.get(addr as usize + i as usize).copied().unwrap_or(0);
        s.push_str(&format!("{b:02X}"));
    }
    s
}
