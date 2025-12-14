#![allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::upper_case_acronyms,
    clippy::missing_safety_doc
)]

use core::ffi::c_void;
use std::os::raw::{c_char, c_int, c_uint};

// Constants from libmobile's public header (mobile.h).
// These are part of the public ABI surface and are safe to mirror here.

pub const MOBILE_MAX_CONNECTIONS: usize = 2;
pub const MOBILE_MAX_TIMERS: usize = 4;
pub const MOBILE_MAX_TRANSFER_SIZE: usize = 0xFE;
pub const MOBILE_MAX_NUMBER_SIZE: usize = 0x20;
pub const MOBILE_CONFIG_SIZE: usize = 0x200;
pub const MOBILE_RELAY_TOKEN_SIZE: usize = 0x10;

pub const MOBILE_SERIAL_IDLE_BYTE: u8 = 0xD2;
pub const MOBILE_SERIAL_IDLE_WORD: u32 = 0xD2D2D2D2;

#[repr(C)]
pub struct mobile_adapter {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum mobile_adapter_device {
    MOBILE_ADAPTER_GAMEBOY = 0,
    MOBILE_ADAPTER_GAMEBOY_ADVANCE = 1,

    MOBILE_ADAPTER_BLUE = 8,
    MOBILE_ADAPTER_YELLOW = 9,
    MOBILE_ADAPTER_GREEN = 10,
    MOBILE_ADAPTER_RED = 11,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum mobile_action {
    MOBILE_ACTION_NONE = 0,
    MOBILE_ACTION_PROCESS_COMMAND = 1 << 0,
    MOBILE_ACTION_DROP_CONNECTION = 1 << 1,
    MOBILE_ACTION_RESET = 1 << 2,
    MOBILE_ACTION_RESET_SERIAL = 1 << 3,
    MOBILE_ACTION_CHANGE_32BIT_MODE = 1 << 4,
    MOBILE_ACTION_WRITE_CONFIG = 1 << 5,
    MOBILE_ACTION_INIT_NUMBER = 1 << 6,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum mobile_socktype {
    MOBILE_SOCKTYPE_TCP = 0,
    MOBILE_SOCKTYPE_UDP = 1,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum mobile_addrtype {
    MOBILE_ADDRTYPE_NONE = 0,
    MOBILE_ADDRTYPE_IPV4 = 1,
    MOBILE_ADDRTYPE_IPV6 = 2,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum mobile_number {
    MOBILE_NUMBER_USER = 0,
    MOBILE_NUMBER_PEER = 1,
}

pub const MOBILE_HOSTLEN_IPV4: usize = 4;
pub const MOBILE_HOSTLEN_IPV6: usize = 16;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct mobile_addr4 {
    pub type_: mobile_addrtype,
    pub port: c_uint,
    pub host: [u8; MOBILE_HOSTLEN_IPV4],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct mobile_addr6 {
    pub type_: mobile_addrtype,
    pub port: c_uint,
    pub host: [u8; MOBILE_HOSTLEN_IPV6],
}

#[repr(C)]
pub union mobile_addr {
    pub type_: mobile_addrtype,
    pub _addr4: mobile_addr4,
    pub _addr6: mobile_addr6,
}

pub type mobile_func_debug_log =
    Option<unsafe extern "C" fn(user: *mut c_void, line: *const c_char)>;
pub type mobile_func_serial_disable = Option<unsafe extern "C" fn(user: *mut c_void)>;
pub type mobile_func_serial_enable =
    Option<unsafe extern "C" fn(user: *mut c_void, mode_32bit: bool)>;

pub type mobile_func_config_read = Option<
    unsafe extern "C" fn(user: *mut c_void, dest: *mut c_void, offset: usize, size: usize) -> bool,
>;
pub type mobile_func_config_write = Option<
    unsafe extern "C" fn(user: *mut c_void, src: *const c_void, offset: usize, size: usize) -> bool,
>;

pub type mobile_func_time_latch = Option<unsafe extern "C" fn(user: *mut c_void, timer: c_uint)>;
pub type mobile_func_time_check_ms =
    Option<unsafe extern "C" fn(user: *mut c_void, timer: c_uint, ms: c_uint) -> bool>;

pub type mobile_func_sock_open = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        conn: c_uint,
        socktype: mobile_socktype,
        addrtype: mobile_addrtype,
        bindport: c_uint,
    ) -> bool,
>;

pub type mobile_func_sock_close = Option<unsafe extern "C" fn(user: *mut c_void, conn: c_uint)>;

pub type mobile_func_sock_connect = Option<
    unsafe extern "C" fn(user: *mut c_void, conn: c_uint, addr: *const mobile_addr) -> c_int,
>;

pub type mobile_func_sock_listen =
    Option<unsafe extern "C" fn(user: *mut c_void, conn: c_uint) -> bool>;

pub type mobile_func_sock_accept =
    Option<unsafe extern "C" fn(user: *mut c_void, conn: c_uint) -> bool>;

pub type mobile_func_sock_send = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        conn: c_uint,
        data: *const c_void,
        size: c_uint,
        addr: *const mobile_addr,
    ) -> c_int,
>;

pub type mobile_func_sock_recv = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        conn: c_uint,
        data: *mut c_void,
        size: c_uint,
        addr: *mut mobile_addr,
    ) -> c_int,
>;

pub type mobile_func_update_number = Option<
    unsafe extern "C" fn(user: *mut c_void, number_type: mobile_number, number: *const c_char),
>;

unsafe extern "C" {
    pub static mobile_version: u32;
    pub static mobile_sizeof: usize;

    pub fn mobile_def_debug_log(adapter: *mut mobile_adapter, func: mobile_func_debug_log);
    pub fn mobile_def_serial_disable(
        adapter: *mut mobile_adapter,
        func: mobile_func_serial_disable,
    );
    pub fn mobile_def_serial_enable(adapter: *mut mobile_adapter, func: mobile_func_serial_enable);
    pub fn mobile_def_config_read(adapter: *mut mobile_adapter, func: mobile_func_config_read);
    pub fn mobile_def_config_write(adapter: *mut mobile_adapter, func: mobile_func_config_write);
    pub fn mobile_def_time_latch(adapter: *mut mobile_adapter, func: mobile_func_time_latch);
    pub fn mobile_def_time_check_ms(adapter: *mut mobile_adapter, func: mobile_func_time_check_ms);
    pub fn mobile_def_sock_open(adapter: *mut mobile_adapter, func: mobile_func_sock_open);
    pub fn mobile_def_sock_close(adapter: *mut mobile_adapter, func: mobile_func_sock_close);
    pub fn mobile_def_sock_connect(adapter: *mut mobile_adapter, func: mobile_func_sock_connect);
    pub fn mobile_def_sock_listen(adapter: *mut mobile_adapter, func: mobile_func_sock_listen);
    pub fn mobile_def_sock_accept(adapter: *mut mobile_adapter, func: mobile_func_sock_accept);
    pub fn mobile_def_sock_send(adapter: *mut mobile_adapter, func: mobile_func_sock_send);
    pub fn mobile_def_sock_recv(adapter: *mut mobile_adapter, func: mobile_func_sock_recv);
    pub fn mobile_def_update_number(adapter: *mut mobile_adapter, func: mobile_func_update_number);

    pub fn mobile_config_load(adapter: *mut mobile_adapter);
    pub fn mobile_config_save(adapter: *mut mobile_adapter);

    pub fn mobile_config_set_device(
        adapter: *mut mobile_adapter,
        device: mobile_adapter_device,
        unmetered: bool,
    );
    pub fn mobile_config_set_dns(
        adapter: *mut mobile_adapter,
        dns1: *const mobile_addr,
        dns2: *const mobile_addr,
    );
    pub fn mobile_config_set_p2p_port(adapter: *mut mobile_adapter, p2p_port: c_uint);
    pub fn mobile_config_set_relay(adapter: *mut mobile_adapter, relay: *const mobile_addr);
    pub fn mobile_config_set_relay_token(adapter: *mut mobile_adapter, token: *const u8);

    pub fn mobile_actions_get(adapter: *mut mobile_adapter) -> mobile_action;
    pub fn mobile_actions_process(adapter: *mut mobile_adapter, actions: mobile_action);

    pub fn mobile_loop(adapter: *mut mobile_adapter);

    pub fn mobile_transfer(adapter: *mut mobile_adapter, c: u8) -> u8;
    pub fn mobile_transfer_32bit(adapter: *mut mobile_adapter, c: u32) -> u32;

    pub fn mobile_start(adapter: *mut mobile_adapter);
    pub fn mobile_stop(adapter: *mut mobile_adapter);

    pub fn mobile_init(adapter: *mut mobile_adapter, user: *mut c_void);
    pub fn mobile_new(user: *mut c_void) -> *mut mobile_adapter;
}
