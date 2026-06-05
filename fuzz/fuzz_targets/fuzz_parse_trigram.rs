#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut pos = 0;
    let _ = ix::varint::decode(data, &mut pos);
    if data.len() >= 3 {
        let _ = ix::trigram::from_bytes(data[0], data[1], data[2]);
    }
});
