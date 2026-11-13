// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Probes every 7-bit I2C address on each candidate SDA/SCL pair and prints
//! what answers, to find a display without guessing its wiring.

use esp_idf_svc::sys;

/// SDA/SCL pairs used by common ESP32-S3 boards: XIAO (5/6), DevKitC and
/// many shields (8/9), Heltec (17/18), and a few others.
const PAIRS: &[(i32, i32)] = &[
    (5, 6),
    (8, 9),
    (17, 18),
    (1, 2),
    (4, 5),
    (41, 42),
    (39, 40),
    (21, 47),
    (10, 11),
];

fn main() {
    sys::link_patches();
    std::thread::sleep(std::time::Duration::from_millis(500));
    for &(sda, scl) in PAIRS {
        let mut found = Vec::new();
        // SAFETY: plain ESP-IDF I2C driver calls on pins this firmware owns.
        unsafe {
            let mut bus: sys::i2c_master_bus_handle_t = core::ptr::null_mut();
            let mut cfg: sys::i2c_master_bus_config_t = core::mem::zeroed();
            cfg.i2c_port = 0;
            cfg.sda_io_num = sda;
            cfg.scl_io_num = scl;
            cfg.__bindgen_anon_1.clk_source = sys::soc_periph_i2c_clk_src_t_I2C_CLK_SRC_DEFAULT;
            cfg.glitch_ignore_cnt = 7;
            cfg.flags.set_enable_internal_pullup(1);
            if sys::i2c_new_master_bus(&cfg, &mut bus) != 0 {
                println!("SCAN sda={sda} scl={scl} bus-init-failed");
                continue;
            }
            for addr in 0x08u16..0x78 {
                if sys::i2c_master_probe(bus, addr, 20) == 0 {
                    found.push(format!("0x{addr:02x}"));
                }
            }
            sys::i2c_del_master_bus(bus);
        }
        println!("SCAN sda={sda} scl={scl} found=[{}]", found.join(","));
    }
    println!("SCAN done");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
