#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut pos = 0;
    let _ = ix::varint::decode(data, &mut pos);
});
