#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = ix::varint::decode(data);
    let _ = ix::trigram::Trigram::from_bytes(data);
});
