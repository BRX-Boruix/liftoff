//! 启动诊断输出：通过平台抽象写字节。
//!
//! 边界：只做“把字节按顺序送到诊断通道”，通道由 `Platform::write_byte` 决定。

use arch::platform::Platform;

/// 通过平台把整段字节按顺序写出。
pub fn write_all<P: Platform>(bytes: &[u8]) {
    for byte in bytes {
        P::write_byte(*byte);
    }
}

/// 输出启动诊断行：`[liftoff] gen2 up, platform=<name>`。
pub fn report_startup<P: Platform>() {
    write_all::<P>(b"[liftoff] gen2 up, platform=");
    write_all::<P>(P::name().as_bytes());
    P::write_byte(b'\n');
}

#[cfg(test)]
mod tests {
    use super::{report_startup, write_all};
    use arch::platform::{InterruptState, Platform};
    use std::vec::Vec;

    /// 测试替身：把写入的字节记下来。
    struct Recorder;

    // 记录缓冲：测试内单线程使用。
    static mut SINK: Option<Vec<u8>> = None;

    impl Platform for Recorder {
        fn init() {}

        fn name() -> &'static str {
            "recorder"
        }

        unsafe fn jump_to(_entry: u64) -> ! {
        // 测试替身不应被调用：万一有测试走到跳转，就响亮失败，而不是静默通过。
        panic!("测试替身不应被调用")
    }

    fn halt() -> ! {
            loop {
                core::hint::spin_loop();
            }
        }

        fn write_byte(byte: u8) {
            // SAFETY: 测试内单线程；不创建对静态的引用。
            unsafe {
                let slot = (&raw mut SINK).as_mut().expect("SINK 地址有效");
                slot.as_mut().expect("SINK 已初始化").push(byte);
            }
        }

        fn disable_interrupts() -> InterruptState {
            InterruptState::from_enabled(true)
        }

        fn restore_interrupts(_state: InterruptState) {}
    }

    fn reset() {
        // SAFETY: 测试内单线程。
        unsafe { *(&raw mut SINK) = Some(Vec::new()) };
    }

    fn taken() -> Vec<u8> {
        // SAFETY: 测试内单线程。
        unsafe { (*&raw mut SINK).take().unwrap_or_default() }
    }

    #[test]
    fn write_all_writes_every_byte_in_order() {
        reset();
        write_all::<Recorder>(b"abc");
        assert_eq!(taken(), b"abc".to_vec());
    }

    #[test]
    fn report_startup_names_the_platform() {
        reset();
        report_startup::<Recorder>();
        let line = std::string::String::from_utf8(taken()).expect("UTF-8");
        assert_eq!(line, "[liftoff] gen2 up, platform=recorder\n");
    }
}